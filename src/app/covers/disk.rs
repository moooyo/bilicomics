use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, Metadata, OpenOptions},
    io::Read,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use image::ImageEncoder;
#[cfg(windows)]
use std::fs;

pub(super) const COVER_DISK_BUDGET: u64 = 128 * 1024 * 1024;
pub(super) const COVER_DISK_FILE_LIMIT: usize = 1024;
const MAX_COVER_BYTES: u64 = 8 * 1024 * 1024;
const CANDIDATE_LIMIT: usize = 256;
const EVICTION_BATCH: usize = 128;
const TEMPORARY_BATCH: usize = 128;
static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
pub(super) struct MaintenanceRequest {
    pub touches: BTreeMap<String, SystemTime>,
    pub protected: BTreeSet<String>,
}

pub(super) struct MaintenanceResult {
    pub evicted: Vec<String>,
    pub needs_more: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProcessLiveness {
    Alive,
    Dead,
    Unknown,
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct FileIdentity {
    volume: u64,
    index: u64,
}

struct Candidate {
    name: String,
    bytes: u64,
    modified: SystemTime,
    identity: FileIdentity,
}

fn managed_name(name: &str) -> bool {
    name.len() == 68 && name.ends_with(".png")
        && name.as_bytes()[..64].iter().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

fn temporary_owner(name: &str) -> Option<u32> {
    let body = name.strip_prefix(".cover-")?.strip_suffix(".pending")?;
    let (owner, nonce) = body.split_once('-')?;
    if owner.is_empty() || owner.starts_with('0') || owner.len() > 10 || !owner.bytes().all(|byte| byte.is_ascii_digit())
        || nonce.len() != 32 || !nonce.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) {
        return None;
    }
    let owner = owner.parse::<u32>().ok()?;
    (owner > 0).then_some(owner)
}

fn absolute_data_directory(data_dir: &Path) -> Result<PathBuf> {
    let path = if data_dir.is_absolute() { data_dir.to_owned() }
        else { std::env::current_dir().context("Resolve the cover data directory")?.join(data_dir) };
    ensure!(path.components().all(|part| !matches!(part, Component::ParentDir)), "Cover data directory traversal is not allowed");
    Ok(path)
}

fn regular_metadata(metadata: &Metadata) -> Result<()> {
    ensure!(metadata.is_file() && !metadata.file_type().is_symlink(), "A cached cover must be a regular file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(metadata.uid() == unsafe { libc::geteuid() }, "A cached cover has a different owner");
        ensure!(metadata.nlink() == 1, "Hard-linked cached covers are not allowed");
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(metadata.file_attributes() & 0x400 == 0, "Cached cover reparse points are not allowed");
    }
    Ok(())
}

pub(super) fn read_cover(data_dir: &Path, name: &str) -> Result<Option<(Vec<u8>, u32, u32)>> {
    read_cover_bytes(data_dir, name)?.map(super::super::startup::decode_cover_bytes).transpose()
}

/// Read a managed object without imposing the thumbnail decoder's geometry.
pub(super) fn read_cover_bytes(data_dir: &Path, name: &str) -> Result<Option<Vec<u8>>> {
    ensure!(managed_name(name), "Invalid cached cover name");
    let Some(directory) = CacheDirectory::open(data_dir, false)? else { return Ok(None); };
    let Some(file) = directory.open_regular(name, false)? else { return Ok(None); };
    ensure!(file.metadata()?.len() <= MAX_COVER_BYTES, "Cached cover exceeds its byte limit");
    let mut encoded = Vec::new();
    file.take(MAX_COVER_BYTES + 1).read_to_end(&mut encoded).context("Read a cached cover")?;
    ensure!(encoded.len() as u64 <= MAX_COVER_BYTES, "Cached cover exceeds its byte limit");
    Ok(Some(encoded))
}

pub(super) fn publish_cover(data_dir: &Path, name: &str, rgba: &[u8], width: u32, height: u32) -> Result<PathBuf> {
    ensure!(managed_name(name), "Invalid cached cover name");
    ensure!(width > 0 && height > 0 && width <= 480 && height <= 640, "Invalid prepared cover dimensions");
    ensure!((width as usize).checked_mul(height as usize).and_then(|pixels| pixels.checked_mul(4)) == Some(rgba.len()),
        "Invalid prepared cover pixel count");
    publish_png(data_dir, name, rgba, width, height, image::ExtendedColorType::Rgba8)
}

/// Share the bounded managed cache while retaining a screen-sized L8 image.
pub(super) fn publish_sleep_cover(data_dir: &Path, name: &str, luma: &[u8], width: u32, height: u32) -> Result<PathBuf> {
    ensure!(managed_name(name), "Invalid cached cover name");
    ensure!(width > 0 && height > 0 && width <= 1536 && height <= 2048, "Invalid prepared sleep cover dimensions");
    ensure!((width as usize).checked_mul(height as usize) == Some(luma.len()),
        "Invalid prepared sleep cover pixel count");
    publish_png(data_dir, name, luma, width, height, image::ExtendedColorType::L8)
}

fn publish_png(data_dir: &Path, name: &str, pixels: &[u8], width: u32, height: u32, color: image::ExtendedColorType) -> Result<PathBuf> {
    let directory = CacheDirectory::open(data_dir, true)?.context("Create the cached cover directory")?;
    let mut temporary = OwnedTemporary::create(&directory)?;
    image::codecs::png::PngEncoder::new(temporary.file_mut())
        .write_image(pixels, width, height, color)
        .context("Encode a prepared cover")?;
    ensure!(temporary.file_mut().metadata()?.len() <= MAX_COVER_BYTES, "Prepared cover exceeds its byte limit");
    temporary.publish(name)?;
    Ok(directory.path.join(name))
}

pub(super) fn maintain(data_dir: &Path, request: MaintenanceRequest) -> Result<MaintenanceResult> {
    maintain_with_limits(data_dir, request, COVER_DISK_BUDGET, COVER_DISK_FILE_LIMIT, process_liveness)
}

fn maintain_with_limits(
    data_dir: &Path,
    request: MaintenanceRequest,
    maximum_bytes: u64,
    maximum_files: usize,
    liveness: fn(u32) -> ProcessLiveness,
) -> Result<MaintenanceResult> {
    let Some(directory) = CacheDirectory::open(data_dir, false)? else {
        return Ok(MaintenanceResult { evicted: Vec::new(), needs_more: false });
    };
    // Access timestamps are applied in a background batch before choosing LRU victims.
    for (name, touched) in request.touches {
        if !managed_name(&name) { continue; }
        if let Ok(Some(file)) = directory.open_regular(&name, true) {
            if file.metadata()?.modified().map_or(true, |modified| touched > modified) {
                file.set_modified(touched).context("Update a cached cover access time")?;
            }
        }
    }
    let mut total_bytes = 0_u64;
    let mut total_files = 0_usize;
    let mut candidates = Vec::<Candidate>::with_capacity(CANDIDATE_LIMIT);
    let mut temporaries = Vec::with_capacity(TEMPORARY_BATCH);
    let mut temporary_more = false;
    for entry in directory.entries()? {
        let name = entry?;
        if let Some(owner) = temporary_owner(&name) {
            if liveness(owner) != ProcessLiveness::Dead { continue; }
            if temporaries.len() >= TEMPORARY_BATCH { temporary_more = true; continue; }
            let Ok(Some(file)) = directory.open_regular(&name, false) else { continue; };
            temporaries.push((name, file_identity(&file)?));
            continue;
        }
        if !managed_name(&name) { continue; }
        let Ok(Some(file)) = directory.open_regular(&name, false) else { continue; };
        let metadata = file.metadata()?;
        total_bytes = total_bytes.saturating_add(metadata.len());
        total_files = total_files.saturating_add(1);
        if request.protected.contains(&name) { continue; }
        let candidate = Candidate {
            name, bytes: metadata.len(), modified: metadata.modified().unwrap_or(UNIX_EPOCH),
            identity: file_identity(&file)?,
        };
        if candidates.len() < CANDIDATE_LIMIT {
            candidates.push(candidate);
        } else {
            let newest = candidates.iter().enumerate().max_by(|(_, left), (_, right)|
                left.modified.cmp(&right.modified).then_with(|| left.name.cmp(&right.name))).map(|(index, _)| index).unwrap();
            let previous = &candidates[newest];
            if candidate.modified.cmp(&previous.modified).then_with(|| candidate.name.cmp(&previous.name)).is_lt() {
                candidates[newest] = candidate;
            }
        }
    }
    // Do not mutate directory entries while their streaming enumeration is active.
    let mut temporary_removed = 0_usize;
    for (name, identity) in temporaries {
        if directory.remove_if_same(&name, identity)? { temporary_removed += 1; }
    }
    candidates.sort_by(|left, right| left.modified.cmp(&right.modified).then_with(|| left.name.cmp(&right.name)));
    let mut evicted = Vec::new();
    for candidate in candidates {
        if total_bytes <= maximum_bytes && total_files <= maximum_files || evicted.len() >= EVICTION_BATCH { break; }
        let Ok(Some(file)) = directory.open_regular(&candidate.name, false) else { continue; };
        let metadata = file.metadata()?;
        // A newly published or newly touched object gets a fresh decision in the next batch.
        if file_identity(&file)? != candidate.identity || metadata.len() != candidate.bytes
            || metadata.modified().unwrap_or(UNIX_EPOCH) != candidate.modified {
            continue;
        }
        if directory.remove_if_same(&candidate.name, candidate.identity)? {
            total_bytes = total_bytes.saturating_sub(candidate.bytes);
            total_files = total_files.saturating_sub(1);
            evicted.push(candidate.name);
        }
    }
    if !evicted.is_empty() || temporary_removed > 0 { directory.sync()?; }
    Ok(MaintenanceResult {
        evicted,
        needs_more: temporary_more || total_bytes > maximum_bytes || total_files > maximum_files,
    })
}

struct OwnedTemporary<'a> {
    directory: &'a CacheDirectory,
    name: String,
    file: Option<File>,
    identity: FileIdentity,
    published: bool,
}

impl<'a> OwnedTemporary<'a> {
    fn create(directory: &'a CacheDirectory) -> Result<Self> {
        for _ in 0..16 {
            let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64;
            let sequence = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
            let name = format!(".cover-{}-{stamp:016x}{sequence:016x}.pending", std::process::id());
            match Self::create_named(directory, name) {
                Ok(temporary) => return Ok(temporary),
                Err(error) if error.downcast_ref::<std::io::Error>().is_some_and(|error| error.kind() == std::io::ErrorKind::AlreadyExists) => {}
                Err(error) => return Err(error),
            }
        }
        anyhow::bail!("Could not allocate a private cover temporary file")
    }

    fn create_named(directory: &'a CacheDirectory, name: String) -> Result<Self> {
        ensure!(temporary_owner(&name) == Some(std::process::id()), "A cover temporary file needs the current process owner");
        let file = directory.create_new(&name).context("Create a private cover temporary file")?;
        let identity = file_identity(&file)?;
        Ok(Self { directory, name, file: Some(file), identity, published: false })
    }

    fn file_mut(&mut self) -> &mut File { self.file.as_mut().expect("The cover temporary file is open") }

    fn publish(mut self, target: &str) -> Result<()> {
        self.file_mut().sync_all().context("Sync a prepared cover")?;
        let current = self.directory.open_regular(&self.name, false)?.context("The prepared cover temporary file disappeared")?;
        ensure!(file_identity(&current)? == self.identity, "The prepared cover temporary file was replaced");
        self.directory.replace(&self.name, target)?;
        self.published = true;
        drop(self.file.take());
        self.directory.sync()
    }
}

impl Drop for OwnedTemporary<'_> {
    fn drop(&mut self) {
        // Keep the original inode pinned until the identity-checked cleanup completes.
        if !self.published {
            let _ = self.directory.remove_if_same(&self.name, self.identity);
        }
        drop(self.file.take());
    }
}

#[cfg(unix)]
struct CacheDirectory {
    path: PathBuf,
    file: File,
}

#[cfg(unix)]
impl CacheDirectory {
    fn open(data_dir: &Path, create: bool) -> Result<Option<Self>> {
        use std::os::{fd::{AsRawFd, FromRawFd}, unix::{ffi::OsStrExt, fs::{MetadataExt, OpenOptionsExt}}};
        let absolute = absolute_data_directory(data_dir)?;
        let mut current = OpenOptions::new().read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC).open("/")
            .context("Open the cover filesystem root")?;
        for component in absolute.components() {
            let Component::Normal(name) = component else { continue; };
            let name = std::ffi::CString::new(name.as_bytes()).context("Invalid cover directory component")?;
            let descriptor = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
            if descriptor < 0 {
                let error = std::io::Error::last_os_error();
                if !create && error.kind() == std::io::ErrorKind::NotFound { return Ok(None); }
                return Err(error).context("Open a cover data directory component");
            }
            current = unsafe { File::from_raw_fd(descriptor) };
        }
        let name = c"covers";
        let mut descriptor = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        if descriptor < 0 && create && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
            if unsafe { libc::mkdirat(current.as_raw_fd(), name.as_ptr(), 0o700) } != 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists {
                return Err(std::io::Error::last_os_error()).context("Create the cover cache directory");
            }
            descriptor = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        }
        if descriptor < 0 {
            let error = std::io::Error::last_os_error();
            if !create && error.kind() == std::io::ErrorKind::NotFound { return Ok(None); }
            return Err(error).context("Open the cover cache directory");
        }
        let file = unsafe { File::from_raw_fd(descriptor) };
        ensure!(file.metadata()?.uid() == unsafe { libc::geteuid() }, "The cover cache directory has a different owner");
        Ok(Some(Self { path: absolute.join("covers"), file }))
    }

    fn open_regular(&self, name: &str, writable: bool) -> Result<Option<File>> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let name = std::ffi::CString::new(name).context("Invalid cached cover component")?;
        let descriptor = unsafe { libc::openat(self.file.as_raw_fd(), name.as_ptr(),
            (if writable { libc::O_RDWR } else { libc::O_RDONLY }) | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK) };
        if descriptor < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound { return Ok(None); }
            return Err(error).context("Open a cached cover object");
        }
        let file = unsafe { File::from_raw_fd(descriptor) };
        regular_metadata(&file.metadata()?)?;
        let _ = file_identity(&file)?;
        Ok(Some(file))
    }

    fn create_new(&self, name: &str) -> Result<File> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let name = std::ffi::CString::new(name).context("Invalid cached cover temporary component")?;
        let descriptor = unsafe { libc::openat(self.file.as_raw_fd(), name.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0o600) };
        if descriptor < 0 { return Err(std::io::Error::last_os_error()).context("Create a cached cover object"); }
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }

    fn remove_if_same(&self, name: &str, identity: FileIdentity) -> Result<bool> {
        use std::os::fd::AsRawFd;
        let Some(file) = self.open_regular(name, false)? else { return Ok(false); };
        if file_identity(&file)? != identity { return Ok(false); }
        let name = std::ffi::CString::new(name).context("Invalid cached cover component")?;
        if unsafe { libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound { return Ok(false); }
            return Err(error).context("Remove a cached cover object");
        }
        Ok(true)
    }

    fn replace(&self, source: &str, target: &str) -> Result<()> {
        use std::os::fd::AsRawFd;
        let _ = self.open_regular(target, false)?;
        let source = std::ffi::CString::new(source).context("Invalid cached cover temporary component")?;
        let target = std::ffi::CString::new(target).context("Invalid cached cover component")?;
        if unsafe { libc::renameat(self.file.as_raw_fd(), source.as_ptr(), self.file.as_raw_fd(), target.as_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error()).context("Publish a cached cover");
        }
        Ok(())
    }

    fn entries(&self) -> Result<DirectoryEntries> {
        use std::os::fd::AsRawFd;
        let descriptor = unsafe { libc::openat(self.file.as_raw_fd(), c".".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        if descriptor < 0 { return Err(std::io::Error::last_os_error()).context("Open the cover cache directory stream"); }
        let stream = unsafe { libc::fdopendir(descriptor) };
        if stream.is_null() {
            let error = std::io::Error::last_os_error();
            unsafe { libc::close(descriptor); }
            return Err(error).context("Read the cover cache directory stream");
        }
        Ok(DirectoryEntries { stream, finished: false })
    }

    fn sync(&self) -> Result<()> { self.file.sync_all().context("Sync the cover cache directory") }
}

#[cfg(unix)]
struct DirectoryEntries {
    stream: *mut libc::DIR,
    finished: bool,
}

#[cfg(unix)]
impl Iterator for DirectoryEntries {
    type Item = Result<String>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.finished { return None; }
        loop {
            clear_directory_errno();
            let entry = unsafe { libc::readdir(self.stream) };
            if entry.is_null() {
                self.finished = true;
                let error = std::io::Error::last_os_error();
                return if error.raw_os_error() == Some(0) { None }
                    else { Some(Err(error).context("Enumerate the cover cache directory")) };
            }
            let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
            let Ok(name) = name.to_str() else { continue; };
            if name == "." || name == ".." { continue; }
            return Some(Ok(name.to_owned()));
        }
    }
}

#[cfg(unix)]
impl Drop for DirectoryEntries {
    fn drop(&mut self) { unsafe { libc::closedir(self.stream); } }
}

#[cfg(unix)]
fn clear_directory_errno() {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    unsafe { *libc::__errno_location() = 0; }
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd", target_os = "dragonfly"))]
    unsafe { *libc::__error() = 0; }
    #[cfg(any(target_os = "openbsd", target_os = "netbsd"))]
    unsafe { *libc::__errno() = 0; }
}

#[cfg(unix)]
fn file_identity(file: &File) -> Result<FileIdentity> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok(FileIdentity { volume: metadata.dev(), index: metadata.ino() })
}

#[cfg(unix)]
fn process_liveness(pid: u32) -> ProcessLiveness {
    let Ok(pid) = i32::try_from(pid) else { return ProcessLiveness::Unknown; };
    if pid <= 0 { return ProcessLiveness::Unknown; }
    if unsafe { libc::kill(pid, 0) } == 0 { return ProcessLiveness::Alive; }
    if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) { ProcessLiveness::Dead }
    else { ProcessLiveness::Unknown }
}

#[cfg(windows)]
struct CacheDirectory {
    path: PathBuf,
    _pins: Vec<File>,
}

#[cfg(windows)]
impl CacheDirectory {
    fn open(data_dir: &Path, create: bool) -> Result<Option<Self>> {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        let absolute = absolute_data_directory(data_dir)?;
        let mut prefix = PathBuf::new();
        let mut pins = Vec::new();
        for component in absolute.components() {
            prefix.push(component.as_os_str());
            if matches!(component, Component::Prefix(_) | Component::CurDir) { continue; }
            let file = match OpenOptions::new().read(true).share_mode(0x1 | 0x2)
                .custom_flags(0x02000000 | 0x00200000).open(&prefix) {
                Ok(file) => file,
                Err(error) if !create && error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error).context("Pin a cover data directory component"),
            };
            let metadata = file.metadata()?;
            ensure!(metadata.is_dir() && !metadata.file_type().is_symlink() && metadata.file_attributes() & 0x400 == 0,
                "Cover directory links and reparse points are not allowed");
            pins.push(file);
        }
        let path = absolute.join("covers");
        if create {
            match fs::create_dir(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error).context("Create the cover cache directory"),
            }
        }
        let file = match OpenOptions::new().read(true).share_mode(0x1 | 0x2)
            .custom_flags(0x02000000 | 0x00200000).open(&path) {
            Ok(file) => file,
            Err(error) if !create && error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("Pin the cover cache directory"),
        };
        let metadata = file.metadata()?;
        ensure!(metadata.is_dir() && !metadata.file_type().is_symlink() && metadata.file_attributes() & 0x400 == 0,
            "Cover directory links and reparse points are not allowed");
        // Omitting FILE_SHARE_DELETE pins every ancestor against path replacement.
        pins.push(file);
        Ok(Some(Self { path, _pins: pins }))
    }

    fn open_regular(&self, name: &str, writable: bool) -> Result<Option<File>> {
        use std::os::windows::fs::OpenOptionsExt;
        let file = match OpenOptions::new().read(true).write(writable).share_mode(0x1 | 0x2 | 0x4)
            .custom_flags(0x00200000).open(self.path.join(name)) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("Open a cached cover object"),
        };
        regular_metadata(&file.metadata()?)?;
        let _ = file_identity(&file)?;
        Ok(Some(file))
    }

    fn create_new(&self, name: &str) -> Result<File> {
        use std::os::windows::fs::OpenOptionsExt;
        OpenOptions::new().read(true).write(true).create_new(true).share_mode(0x1 | 0x4)
            .custom_flags(0x00200000).open(self.path.join(name)).context("Create a private cached cover object")
    }

    fn remove_if_same(&self, name: &str, identity: FileIdentity) -> Result<bool> {
        let Some(file) = self.open_regular(name, false)? else { return Ok(false); };
        if file_identity(&file)? != identity { return Ok(false); }
        match fs::remove_file(self.path.join(name)) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error).context("Remove a cached cover object"),
        }
    }

    fn replace(&self, source: &str, target: &str) -> Result<()> {
        use std::os::windows::ffi::OsStrExt;
        let _ = self.open_regular(target, false)?;
        let source: Vec<u16> = self.path.join(source).as_os_str().encode_wide().chain(Some(0)).collect();
        let target: Vec<u16> = self.path.join(target).as_os_str().encode_wide().chain(Some(0)).collect();
        #[link(name = "kernel32")]
        unsafe extern "system" { fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32; }
        if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 0x1 | 0x8) } == 0 {
            return Err(std::io::Error::last_os_error()).context("Publish a cached cover");
        }
        Ok(())
    }

    fn entries(&self) -> Result<DirectoryEntries> {
        Ok(DirectoryEntries { inner: fs::read_dir(&self.path).context("Read the pinned cover cache directory")? })
    }

    fn sync(&self) -> Result<()> {
        // MOVEFILE_WRITE_THROUGH synchronizes publication; Windows directory handles do not support sync_all.
        Ok(())
    }
}

#[cfg(windows)]
struct DirectoryEntries {
    inner: fs::ReadDir,
}

#[cfg(windows)]
impl Iterator for DirectoryEntries {
    type Item = Result<String>;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.inner.next()? {
                Ok(entry) => match entry.file_name().into_string() {
                    Ok(name) => return Some(Ok(name)),
                    Err(_) => continue,
                },
                Err(error) => return Some(Err(error).context("Enumerate the pinned cover cache directory")),
            }
        }
    }
}

#[cfg(windows)]
fn file_identity(file: &File) -> Result<FileIdentity> {
    use std::{ffi::c_void, os::windows::io::AsRawHandle};
    #[repr(C)]
    #[derive(Default)]
    struct FileTime { low: u32, high: u32 }
    #[repr(C)]
    #[derive(Default)]
    struct ByHandleFileInformation {
        attributes: u32, creation: FileTime, access: FileTime, write: FileTime,
        volume: u32, size_high: u32, size_low: u32, links: u32, index_high: u32, index_low: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" { fn GetFileInformationByHandle(file: *mut c_void, information: *mut ByHandleFileInformation) -> i32; }
    let mut information = ByHandleFileInformation::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(std::io::Error::last_os_error()).context("Inspect a cached cover file identity");
    }
    ensure!(information.links == 1, "Hard-linked cached covers are not allowed");
    Ok(FileIdentity { volume: information.volume as u64,
        index: ((information.index_high as u64) << 32) | information.index_low as u64 })
}

#[cfg(windows)]
fn process_liveness(pid: u32) -> ProcessLiveness {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    if pid == 0 { return ProcessLiveness::Unknown; }
    let process = unsafe { OpenProcess(0x00100000, 0, pid) };
    if process.is_null() {
        return if std::io::Error::last_os_error().raw_os_error() == Some(87) { ProcessLiveness::Dead }
            else { ProcessLiveness::Unknown };
    }
    let state = match unsafe { WaitForSingleObject(process, 0) } {
        0 => ProcessLiveness::Dead,
        258 => ProcessLiveness::Alive,
        _ => ProcessLiveness::Unknown,
    };
    unsafe { CloseHandle(process); }
    state
}

#[cfg(test)]
#[path = "disk_tests.rs"]
mod tests;
