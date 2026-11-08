//! Read the current portrait pose from a recognized Kindle accelerometer.
//!
//! This module uses only read-only evdev metadata ioctls. It does not grab a
//! device, consume its event stream, or change any input or display property.
//! Its validated opener is shared with the separate native event observer.
//! The kx132 values were reviewed in KOReader commit
//! 9040116701c888889bd35233b23b83283be66745, KindleGyroTransform in
//! frontend/device/kindle/device.lua. The implementation here is independent.
//! That driver contract does not establish that a sample reflects a physical
//! direction change at a particular point in the native wake sequence.
//! The scoped Scribe 5.19.5 device reports bma4xy_feature; read-only physical
//! observations confirmed value 15 with native U and value 16 with native D.

use std::{
    fs::{File, OpenOptions},
    io,
    mem::size_of,
    os::{
        fd::AsRawFd,
        unix::fs::{FileTypeExt, OpenOptionsExt},
    },
};

use crate::NativePortraitOrientation;

const MAX_EVENT_INDEX: usize = 31;
const EV_KEY: u8 = 1;
const EV_ABS: u8 = 3;
const ABS_PRESSURE: usize = 0x18;
const WORD_BITS: usize = size_of::<libc::c_ulong>() * 8;
const ABS_WORDS: usize = 64 / WORD_BITS;
const KEY_WORDS: usize = 768 / WORD_BITS;
const NAME_BYTES: usize = 128;

type AbsoluteBits = [libc::c_ulong; ABS_WORDS];
type KeyBits = [libc::c_ulong; KEY_WORDS];

// These expand Linux's _IOR('E', number, size) on the supported Linux ABIs.
const fn read_request(number: u8, bytes: usize) -> libc::c_ulong {
    (2 << 30)
        | ((bytes as libc::c_ulong) << 16)
        | ((b'E' as libc::c_ulong) << 8)
        | number as libc::c_ulong
}

fn has_bit(bits: &[libc::c_ulong], bit: usize) -> bool {
    bits.get(bit / WORD_BITS)
        .is_some_and(|word| *word & (1 << (bit % WORD_BITS)) != 0)
}

fn pressure_only_axes(bits: &AbsoluteBits) -> bool {
    bits.iter().enumerate().all(|(index, word)| {
        let expected = if index == ABS_PRESSURE / WORD_BITS {
            1 << (ABS_PRESSURE % WORD_BITS)
        } else {
            0
        };
        *word == expected
    })
}

fn has_digitizer_keys(bits: &KeyBits) -> bool {
    // Linux's BTN_DIGI group includes touch contacts, stylus buttons and tools.
    (0x140..=0x14f).any(|key| has_bit(bits, key))
}

fn known_portrait_sensor_name(name: &[u8]) -> bool {
    let end = name.iter().position(|byte| *byte == 0).unwrap_or(name.len());
    name[..end].eq_ignore_ascii_case(b"bma4xy_feature") || name[..end]
        .windows(5)
        .any(|part| part.eq_ignore_ascii_case(b"kx132"))
}

fn normalize_pose(name: &[u8], value: i32) -> io::Result<Option<NativePortraitOrientation>> {
    if !known_portrait_sensor_name(name) {
        return Ok(None);
    }
    decode_native_portrait_value(value).map(Some).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown native portrait direction value: {value}"),
        )
    })
}

/// Decode only the reviewed portrait states of an already validated source.
/// Non-pose states are ignored by the active observer without changing its UI.
pub(crate) fn decode_native_portrait_value(value: i32) -> Option<NativePortraitOrientation> {
    match value {
        // Scribe keeps the reviewed U/L and D/R portrait grouping.
        15 | 17 => Some(NativePortraitOrientation::Up),
        16 | 18 => Some(NativePortraitOrientation::Down),
        _ => None,
    }
}

fn record_unique_sensor<T>(found: &mut Option<T>, sensor: T) -> io::Result<()> {
    if found.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "multiple recognized native portrait sensors are ambiguous",
        ));
    }
    *found = Some(sensor);
    Ok(())
}

fn query_bits<const N: usize>(file: &File, event: u8) -> io::Result<[libc::c_ulong; N]> {
    let mut bits = [0; N];
    let request = read_request(0x20 + event, size_of::<[libc::c_ulong; N]>());
    // SAFETY: The initialized native-word buffer has the size encoded in the
    // read-only EVIOCGBIT request. The ioctl does not retain its pointer.
    let result = unsafe { libc::ioctl(file.as_raw_fd(), request as _, bits.as_mut_ptr()) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(bits)
    }
}

fn inspect_sensor(file: &File) -> io::Result<Option<[u8; NAME_BYTES]>> {
    if !file.metadata()?.file_type().is_char_device() {
        return Ok(None);
    }
    let event_bits = query_bits::<1>(file, 0)?;
    if !has_bit(&event_bits, EV_ABS as usize) {
        return Ok(None);
    }
    let axes = query_bits::<ABS_WORDS>(file, EV_ABS)?;
    if !pressure_only_axes(&axes) {
        return Ok(None);
    }
    if has_bit(&event_bits, EV_KEY as usize)
        && has_digitizer_keys(&query_bits::<KEY_WORDS>(file, EV_KEY)?)
    {
        return Ok(None);
    }
    let mut name = [0u8; NAME_BYTES];
    // SAFETY: name has the 128 bytes encoded in EVIOCGNAME and remains valid
    // for the call. The bounded device name is interpreted only up to its NUL.
    let result = unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            read_request(0x06, NAME_BYTES) as _,
            name.as_mut_ptr(),
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    name[NAME_BYTES - 1] = 0;
    if !known_portrait_sensor_name(&name) {
        return Ok(None);
    }
    Ok(Some(name))
}

/// Read one current pressure value from an already validated sensor descriptor.
pub(crate) fn query_native_portrait_value(fd: libc::c_int) -> io::Result<i32> {
    // Linux input_absinfo is six contiguous i32 fields: value, minimum,
    // maximum, fuzz, flat, resolution. Only the current value is consumed.
    let mut information = [0i32; 6];
    // SAFETY: The aligned array has the complete 24-byte input_absinfo layout
    // encoded by EVIOCGABS(ABS_PRESSURE); this metadata request is read-only.
    let result = unsafe {
        libc::ioctl(
            fd,
            read_request(0x40 + ABS_PRESSURE as u8, size_of::<[i32; 6]>()) as _,
            information.as_mut_ptr(),
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(information[0])
}

/// Retain the unique recognized sensor and its validated name, without a grab.
/// Its event queue is untouched; an observer must discard stale events itself.
pub(crate) fn open_native_portrait_sensor() -> io::Result<Option<(File, [u8; NAME_BYTES])>> {
    let mut found = None;
    for index in 0..=MAX_EVENT_INDEX {
        let path = format!("/dev/input/event{index}");
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if let Some(name) = inspect_sensor(&file).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("inspect native portrait sensor {path}: {error}"),
            )
        })? {
            record_unique_sensor(&mut found, (file, name))?;
        }
    }
    Ok(found)
}

/// Query a recognized pressure-only sensor without taking input ownership.
///
/// An absent or unrecognized sensor returns None. An unreadable candidate,
/// unknown native direction value, or multiple recognized sources returns an
/// explicit error; no cached direction replaces a failed startup/wake sample.
pub(crate) fn read_native_portrait_pose() -> io::Result<Option<NativePortraitOrientation>> {
    let Some((file, name)) = open_native_portrait_sensor()? else {
        return Ok(None);
    };
    normalize_pose(&name, query_native_portrait_value(file.as_raw_fd())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_bit(bits: &mut [libc::c_ulong], bit: usize) {
        bits[bit / WORD_BITS] |= 1 << (bit % WORD_BITS);
    }

    #[test]
    fn only_the_complete_pressure_only_axis_bitmap_is_eligible() {
        let mut axes = [0; ABS_WORDS];
        assert!(!pressure_only_axes(&axes));
        set_bit(&mut axes, ABS_PRESSURE);
        assert!(pressure_only_axes(&axes));
        for extra in 0..64 {
            if extra != ABS_PRESSURE {
                let mut with_extra = axes;
                set_bit(&mut with_extra, extra);
                assert!(!pressure_only_axes(&with_extra), "extra axis {extra}");
            }
        }
    }

    #[test]
    fn pressure_cannot_identify_touch_or_pen_sources() {
        let empty = [0; KEY_WORDS];
        assert!(!has_digitizer_keys(&empty));
        for button in 0x140..=0x14f {
            let mut keys = empty;
            set_bit(&mut keys, button);
            assert!(has_digitizer_keys(&keys), "digitizer button {button}");
        }
        let mut power_button = empty;
        set_bit(&mut power_button, 116);
        assert!(!has_digitizer_keys(&power_button));
    }

    #[test]
    fn reviewed_kx132_values_preserve_scribes_portrait_grouping() {
        for (value, expected) in [
            (15, NativePortraitOrientation::Up),
            (16, NativePortraitOrientation::Down),
            (17, NativePortraitOrientation::Up),
            (18, NativePortraitOrientation::Down),
        ] {
            assert_eq!(
                normalize_pose(b"Kindle KX132 accelerometer\0", value).unwrap(),
                Some(expected),
            );
        }
    }

    #[test]
    fn scoped_bma4xy_feature_uses_the_same_direction_contract() {
        for (value, expected) in [
            (15, NativePortraitOrientation::Up),
            (16, NativePortraitOrientation::Down),
            (17, NativePortraitOrientation::Up),
            (18, NativePortraitOrientation::Down),
        ] {
            assert_eq!(normalize_pose(b"bma4xy_feature\0", value).unwrap(), Some(expected));
        }
        assert_eq!(normalize_pose(b"other bma4xy pressure device", 15).unwrap(), None);
        assert_eq!(normalize_pose(b"bma4xy_feature\0", 0).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn unrelated_pressure_values_are_never_interpreted_as_direction() {
        for value in [15, 16, 17, 18, 0, i32::MAX] {
            assert_eq!(normalize_pose(b"pressure sensor\0", value).unwrap(), None);
            assert_eq!(normalize_pose(b"other sensor\0kx132", value).unwrap(), None);
        }
    }

    #[test]
    fn unknown_values_from_the_recognized_driver_are_errors() {
        for value in [i32::MIN, 0, 14, 19, i32::MAX] {
            assert_eq!(
                normalize_pose(b"kx132", value).unwrap_err().kind(),
                io::ErrorKind::InvalidData,
            );
        }
    }

    #[test]
    fn multiple_sensor_matches_are_ambiguous_even_if_they_agree() {
        for second in [
            NativePortraitOrientation::Up,
            NativePortraitOrientation::Down,
        ] {
            let mut found = None;
            record_unique_sensor(&mut found, NativePortraitOrientation::Up).unwrap();
            assert_eq!(
                record_unique_sensor(&mut found, second).unwrap_err().kind(),
                io::ErrorKind::InvalidData,
            );
            assert_eq!(found, Some(NativePortraitOrientation::Up));
        }
    }

    #[test]
    fn read_only_requests_match_linux_evdev_abi() {
        assert_eq!(size_of::<AbsoluteBits>(), 8);
        assert_eq!(size_of::<KeyBits>(), 96);
        assert_eq!(read_request(0x23, size_of::<AbsoluteBits>()), 0x80084523);
        assert_eq!(read_request(0x21, size_of::<KeyBits>()), 0x80604521);
        assert_eq!(read_request(0x06, NAME_BYTES), 0x80804506);
        assert_eq!(read_request(0x58, size_of::<[i32; 6]>()), 0x80184558);
    }
}
