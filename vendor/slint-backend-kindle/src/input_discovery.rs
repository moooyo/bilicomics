//! Bounded kernel metadata hints for evdev discovery.
//! Adapted from AidokuForKindle commit 4ce1fd156a7a4a75ca2fda5f4324991d6b02ce92.
//!
//! Sysfs only narrows the search. Callers still validate the opened descriptor
//! with evdev ioctls and discard the fast path if the device snapshot changes.

use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Read},
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MetadataKind {
    Name,
    AbsoluteAxes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DeviceIdentity {
    filesystem: u64,
    inode: u64,
    device: u64,
}

impl DeviceIdentity {
    fn from_metadata(metadata: &Metadata) -> io::Result<Self> {
        if !metadata.file_type().is_char_device() {
            return Err(invalid_metadata());
        }
        Ok(Self {
            filesystem: metadata.dev(),
            inode: metadata.ino(),
            device: metadata.rdev(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Hint {
    Name(Vec<u8>),
    AbsoluteAxes(u64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EventCandidate {
    path: PathBuf,
    identity: DeviceIdentity,
    hint: Hint,
}

impl EventCandidate {
    pub(crate) fn name(&self) -> Option<&[u8]> {
        match &self.hint {
            Hint::Name(name) => Some(name),
            Hint::AbsoluteAxes(_) => None,
        }
    }

    pub(crate) fn axes(&self) -> Option<u64> {
        match self.hint {
            Hint::AbsoluteAxes(axes) => Some(axes),
            Hint::Name(_) => None,
        }
    }

    /// A vanished or replaced node requires discovery again, not a stale fd.
    pub(crate) fn open_checked(&self) -> io::Result<Option<File>> {
        let file = match open_path(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let observed = DeviceIdentity::from_metadata(&file.metadata()?)?;
        Ok((observed == self.identity).then_some(file))
    }
}

pub(crate) struct EventSnapshot {
    device_root: PathBuf,
    sysfs_root: PathBuf,
    kind: MetadataKind,
    maximum: usize,
    entries: Vec<EventCandidate>,
}

impl EventSnapshot {
    /// Missing, malformed, or unreadable hints preserve the legacy scan.
    pub(crate) fn capture(kind: MetadataKind, maximum: usize) -> Option<Self> {
        Self::capture_with(
            Path::new("/dev/input"),
            Path::new("/sys/class/input"),
            kind,
            maximum,
            device_identity,
        )
        .ok()
    }

    fn capture_with(
        device_root: &Path,
        sysfs_root: &Path,
        kind: MetadataKind,
        maximum: usize,
        mut lookup: impl FnMut(&Path) -> io::Result<Option<DeviceIdentity>>,
    ) -> io::Result<Self> {
        let mut entries = Vec::new();
        for index in 0..=maximum {
            let leaf = format!("event{index}");
            let path = device_root.join(&leaf);
            let Some(identity) = lookup(&path)? else {
                continue;
            };
            let sysfs = sysfs_root.join(&leaf);
            let (major, minor) = parse_device_number(&read_bounded(&sysfs.join("dev"), 64)?)?;
            if u64::from(libc::major(identity.device as libc::dev_t)) != major
                || u64::from(libc::minor(identity.device as libc::dev_t)) != minor
            {
                return Err(invalid_metadata());
            }
            let hint = match kind {
                MetadataKind::Name => {
                    let name = read_bounded(&sysfs.join("device/name"), 128)?;
                    let name = name.strip_suffix(b"\n").unwrap_or(&name);
                    if name.len() > 127 || name.iter().any(|byte| matches!(byte, 0 | b'\n' | b'\r'))
                    {
                        return Err(invalid_metadata());
                    }
                    Hint::Name(name.to_vec())
                }
                MetadataKind::AbsoluteAxes => Hint::AbsoluteAxes(parse_absolute_axes(
                    &read_bounded(&sysfs.join("device/capabilities/abs"), 128)?,
                )?),
            };
            entries.push(EventCandidate {
                path,
                identity,
                hint,
            });
        }
        Ok(Self {
            device_root: device_root.into(),
            sysfs_root: sysfs_root.into(),
            kind,
            maximum,
            entries,
        })
    }

    pub(crate) fn entries(&self) -> impl Iterator<Item = &EventCandidate> {
        self.entries.iter()
    }

    /// Recheck the complete bounded roster, including previously skipped nodes.
    pub(crate) fn is_current(&self) -> bool {
        Self::capture_with(
            &self.device_root,
            &self.sysfs_root,
            self.kind,
            self.maximum,
            device_identity,
        )
        .is_ok_and(|current| current.entries == self.entries)
    }
}

pub(crate) fn open_event(index: usize) -> io::Result<File> {
    open_path(&PathBuf::from(format!("/dev/input/event{index}")))
}

fn open_path(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
}

fn device_identity(path: &Path) -> io::Result<Option<DeviceIdentity>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => DeviceIdentity::from_metadata(&metadata).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn read_bounded(path: &Path, maximum: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open_path(path)?
        .take(maximum as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err(invalid_metadata());
    }
    Ok(bytes)
}

fn parse_device_number(bytes: &[u8]) -> io::Result<(u64, u64)> {
    let value = std::str::from_utf8(bytes)
        .map_err(|_| invalid_metadata())?
        .trim_ascii();
    let (major, minor) = value.split_once(':').ok_or_else(invalid_metadata)?;
    let parse = |value: &str| {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid_metadata());
        }
        value.parse::<u64>().map_err(|_| invalid_metadata())
    };
    Ok((parse(major)?, parse(minor)?))
}

fn parse_absolute_axes(bytes: &[u8]) -> io::Result<u64> {
    let value = std::str::from_utf8(bytes)
        .map_err(|_| invalid_metadata())?
        .trim_ascii();
    if value.contains(['\n', '\r']) {
        return Err(invalid_metadata());
    }
    let words = value.split_ascii_whitespace().collect::<Vec<_>>();
    let limit = match words.len() {
        1 => 16,
        2 => 8,
        _ => return Err(invalid_metadata()),
    };
    let mut bitmap = 0;
    for word in words {
        if word.is_empty()
            || word.len() > limit
            || !word.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(invalid_metadata());
        }
        let part = u64::from_str_radix(word, 16).map_err(|_| invalid_metadata())?;
        bitmap = if limit == 8 {
            (bitmap << 32) | part
        } else {
            part
        };
    }
    Ok(bitmap)
}

fn invalid_metadata() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "input discovery metadata is unavailable or inconsistent",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::fd::AsRawFd,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "bilicomics-input-discovery-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn node(&self, index: usize, name: &str, axes: &str) -> DeviceIdentity {
            let directory = self.0.join(format!("event{index}"));
            fs::create_dir_all(directory.join("device/capabilities")).unwrap();
            fs::write(directory.join("dev"), format!("13:{}\n", 64 + index)).unwrap();
            fs::write(directory.join("device/name"), format!("{name}\n")).unwrap();
            fs::write(directory.join("device/capabilities/abs"), axes).unwrap();
            DeviceIdentity {
                filesystem: 7,
                inode: index as u64 + 100,
                device: libc::makedev(13, 64 + index as u32) as u64,
            }
        }

        fn snapshot(
            &self,
            kind: MetadataKind,
            identities: &[(usize, DeviceIdentity)],
        ) -> io::Result<EventSnapshot> {
            EventSnapshot::capture_with(
                Path::new("/fixture/dev/input"),
                &self.0,
                kind,
                31,
                |path| {
                    let index = path.file_name().unwrap().to_str().unwrap()[5..]
                        .parse::<usize>()
                        .unwrap();
                    Ok(identities
                        .iter()
                        .find(|row| row.0 == index)
                        .map(|row| row.1))
                },
            )
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn both_kernel_bitmap_word_sizes_preserve_the_same_axes() {
        let expected = (1u64 << 53) | (1 << 24);
        assert_eq!(parse_absolute_axes(b"20000001000000\n").unwrap(), expected);
        assert_eq!(parse_absolute_axes(b"200000 01000000\n").unwrap(), expected);
        assert_eq!(parse_absolute_axes(b"0\n").unwrap(), 0);
        assert_eq!(
            parse_absolute_axes(b"ffffffff ffffffff\n").unwrap(),
            u64::MAX
        );
        assert_eq!(
            parse_absolute_axes(b"ffffffffffffffff\n").unwrap(),
            u64::MAX
        );
    }

    #[test]
    fn malformed_bitmaps_cannot_silently_hide_a_device() {
        for value in [
            "",
            "0x20000000000000",
            "+1",
            "-1",
            "1 0 0",
            "100000000 0",
            "10000000000000000",
            "1\n2",
            "1\0",
            "not-hex",
        ] {
            assert!(parse_absolute_axes(value.as_bytes()).is_err(), "{value:?}");
        }
    }

    #[test]
    fn device_numbers_are_bounded_numeric_pairs() {
        assert_eq!(parse_device_number(b"13:64\n").unwrap(), (13, 64));
        for value in [
            "",
            "13",
            "13:",
            ":64",
            "13:64:1",
            "+13:64",
            "13:6x",
            "18446744073709551616:1",
        ] {
            assert!(parse_device_number(value.as_bytes()).is_err());
        }
    }

    #[test]
    fn roster_keeps_numeric_order_and_uses_matching_kernel_device_numbers() {
        let fixture = Fixture::new();
        let first = fixture.node(2, "bma4xy_feature", "1000000\n");
        let second = fixture.node(10, "touch", "20000000000000\n");
        let identities = [(10, second), (2, first)];
        let snapshot = fixture.snapshot(MetadataKind::Name, &identities).unwrap();
        assert!(snapshot.entries[0].path.ends_with("event2"));
        assert!(snapshot.entries[1].path.ends_with("event10"));
        assert_eq!(
            snapshot.entries[0].name(),
            Some(b"bma4xy_feature".as_slice())
        );
        fs::write(fixture.0.join("event2/dev"), "13:99\n").unwrap();
        assert!(fixture.snapshot(MetadataKind::Name, &identities).is_err());
    }

    #[test]
    fn incomplete_metadata_for_a_skipped_node_requires_fallback() {
        let fixture = Fixture::new();
        let unrelated = fixture.node(0, "button", "0\n");
        let sensor = fixture.node(1, "bma4xy_feature", "1000000\n");
        let identities = [(0, unrelated), (1, sensor)];
        fixture.snapshot(MetadataKind::Name, &identities).unwrap();
        fs::remove_file(fixture.0.join("event0/device/name")).unwrap();
        assert!(fixture.snapshot(MetadataKind::Name, &identities).is_err());
    }

    #[test]
    fn topology_and_metadata_changes_invalidate_the_complete_roster() {
        let fixture = Fixture::new();
        let other = fixture.node(0, "button", "0\n");
        let sensor = fixture.node(1, "bma4xy_feature", "1000000\n");
        let before = fixture
            .snapshot(MetadataKind::Name, &[(0, other), (1, sensor)])
            .unwrap();
        let unchanged = fixture
            .snapshot(MetadataKind::Name, &[(0, other), (1, sensor)])
            .unwrap();
        assert_eq!(before.entries, unchanged.entries);
        let removed = fixture
            .snapshot(MetadataKind::Name, &[(1, sensor)])
            .unwrap();
        assert_ne!(before.entries, removed.entries);
        let new_sensor = fixture.node(2, "KX132 accelerometer", "1000000\n");
        let added = fixture
            .snapshot(
                MetadataKind::Name,
                &[(0, other), (1, sensor), (2, new_sensor)],
            )
            .unwrap();
        assert_ne!(before.entries, added.entries);
        fs::write(fixture.0.join("event0/device/name"), "bma4xy_feature\n").unwrap();
        let relabeled = fixture
            .snapshot(MetadataKind::Name, &[(0, other), (1, sensor)])
            .unwrap();
        assert_ne!(before.entries, relabeled.entries);
        let replaced = DeviceIdentity {
            inode: sensor.inode + 1,
            ..sensor
        };
        let current = fixture
            .snapshot(MetadataKind::Name, &[(0, other), (1, replaced)])
            .unwrap();
        assert_ne!(relabeled.entries, current.entries);
    }

    #[test]
    fn oversized_and_multiline_names_require_legacy_inspection() {
        let fixture = Fixture::new();
        let sensor = fixture.node(0, "bma4xy_feature", "1000000\n");
        for name in [
            vec![b'a'; 129],
            b"bma4xy_feature\0ignored\n".to_vec(),
            b"bma4xy_feature\nextra\n".to_vec(),
        ] {
            fs::write(fixture.0.join("event0/device/name"), name).unwrap();
            assert!(
                fixture
                    .snapshot(MetadataKind::Name, &[(0, sensor)])
                    .is_err()
            );
        }
    }

    #[test]
    fn actual_character_descriptor_identity_and_open_flags_are_checked() {
        let path = PathBuf::from("/dev/null");
        let identity = device_identity(&path).unwrap().unwrap();
        let candidate = EventCandidate {
            path,
            identity,
            hint: Hint::AbsoluteAxes(1 << 53),
        };
        let file = candidate.open_checked().unwrap().unwrap();
        assert_eq!(
            DeviceIdentity::from_metadata(&file.metadata().unwrap()).unwrap(),
            identity
        );
        // SAFETY: These fcntl operations only inspect our owned descriptor.
        let descriptor_flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) };
        let status_flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
        assert_ne!(descriptor_flags & libc::FD_CLOEXEC, 0);
        assert_ne!(status_flags & libc::O_NONBLOCK, 0);
        let mut stale = candidate.clone();
        stale.identity.inode += 1;
        assert!(stale.open_checked().unwrap().is_none());
    }

    #[test]
    fn valid_touch_axes_are_selected_from_the_kernel_bitmap() {
        let fixture = Fixture::new();
        let button = fixture.node(0, "button", "0\n");
        let pen = fixture.node(1, "pen", "1000003\n");
        let touch = fixture.node(2, "touch", "200000 00000000\n");
        let snapshot = fixture
            .snapshot(
                MetadataKind::AbsoluteAxes,
                &[(0, button), (1, pen), (2, touch)],
            )
            .unwrap();
        let paths = snapshot
            .entries()
            .filter(|entry| entry.axes().unwrap() & (1 << 53) != 0)
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        assert_eq!(paths, [PathBuf::from("/fixture/dev/input/event2")]);
        fs::write(
            fixture.0.join("event0/device/capabilities/abs"),
            "unsupported\n",
        )
        .unwrap();
        assert!(
            fixture
                .snapshot(
                    MetadataKind::AbsoluteAxes,
                    &[(0, button), (1, pen), (2, touch)]
                )
                .is_err()
        );
    }
}
