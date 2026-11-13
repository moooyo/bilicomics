//! Explicit final-frame dithering policy, independent of application profiles.

use std::io::{self, Read};
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

#[derive(Default)]
struct State {
    available: AtomicBool,
    requested: AtomicBool,
    revision: AtomicU32,
}

/// A producer-side choice for the image already published to the scene.
///
/// Availability describes a reviewed device/driver interface, not a physical
/// panel-quality measurement. The owner must switch this with the matching
/// Gray8 image, rather than when an asynchronous preparation starts.
#[derive(Clone, Default)]
pub struct DitheringHandle {
    state: Arc<State>,
    wake: Option<Arc<OwnedFd>>,
}

impl DitheringHandle {
    pub(crate) fn native(wake: Arc<OwnedFd>) -> Self {
        // Read only identity and framebuffer metadata. Installation must not
        // map display memory, submit updates, or acquire an input device.
        #[cfg(not(test))]
        let supported = std::fs::File::open("/proc/usid")
            .and_then(read_supported_scribe)
            .unwrap_or(false)
            && crate::framebuffer::probe_native_framebuffer(None, None)
                .is_ok_and(|information| information.output_inverted.is_some());
        // Unit fixtures must not depend on, or inspect, their host's devices.
        #[cfg(test)]
        let supported = false;
        Self::with_availability(supported, Some(wake))
    }

    fn with_availability(available: bool, wake: Option<Arc<OwnedFd>>) -> Self {
        Self {
            state: Arc::new(State {
                available: AtomicBool::new(available),
                ..State::default()
            }),
            wake,
        }
    }

    pub fn hardware_y4_available(&self) -> bool {
        self.state.available.load(Ordering::Acquire)
    }

    /// Stage the mode of the currently presented image and wake the renderer.
    ///
    /// A true request is retained after explicit hardware rejection so a Gray8
    /// scene receives software dithering until its replacement is published.
    /// The return value reports actual hardware eligibility, not acceptance of
    /// an ioctl or completion of a panel update.
    /// Call this from the UI scene-publication transaction.
    pub fn set_hardware_y4(&self, enabled: bool) -> bool {
        if self.state.requested.swap(enabled, Ordering::AcqRel) != enabled {
            self.changed();
        }
        enabled && self.hardware_y4_available()
    }

    pub fn hardware_y4_enabled(&self) -> bool {
        self.requested() && self.hardware_y4_available()
    }

    pub(crate) fn requested(&self) -> bool {
        self.state.requested.load(Ordering::Acquire)
    }

    pub(crate) fn revision(&self) -> u32 {
        self.state.revision.load(Ordering::Acquire)
    }

    pub(crate) fn reject_hardware(&self) {
        if self.state.available.swap(false, Ordering::AcqRel) {
            self.changed();
        }
    }

    fn changed(&self) {
        self.state.revision.fetch_add(1, Ordering::AcqRel);
        if let Some(wake) = self.wake.as_ref() {
            crate::wakeup::poke(wake);
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(available: bool) -> Self {
        Self::with_availability(available, None)
    }
}

/// The proc transport may append NUL padding. FBInk's reviewed identity reader
/// consumes exactly the first sixteen bytes, independently of the file length.
/// Require a complete read before classifying the fixed-width identity.
fn read_supported_scribe(mut reader: impl Read) -> io::Result<bool> {
    let mut serial = [0u8; 16];
    reader.read_exact(&mut serial)?;
    Ok(supported_scribe(&serial))
}

/// Identify only the two reviewed monochrome Scribe generations. Never retain
/// or log a complete serial number. Codes follow KOReader 7c38cf104cb9ea89df243e47ec820ef4aa97cab8.
fn supported_scribe(serial: &[u8]) -> bool {
    let serial = serial.trim_ascii();
    if serial.len() != 16 || serial[0] != b'G'
        || !serial.iter().all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return false;
    }
    matches!(&serial[3..6],
        b"27J" | b"2BL" | b"263" | b"227" | b"2BM" | b"23L" | b"23M" | b"270"
        | b"3V0" | b"3V1" | b"3X5" | b"3UV" | b"3X4" | b"3X3" | b"41E" | b"410")
}

/// Quantize logical gray with the same four-by-four palette convention as the
/// application's software path, anchored to final physical pixel coordinates.
pub(crate) fn software_gray(value: u8, x: usize, y: usize) -> u8 {
    const BAYER: [[i16; 4]; 4] = [
        [0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5],
    ];
    let adjusted = (i16::from(value) + BAYER[y % 4][x % 4] - 8).clamp(0, 255) as u16;
    (((adjusted + 8) / 17).min(15) * 17) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_identity_transport_accepts_nul_padding_without_consuming_it() {
        for suffix in [b"".as_slice(), b"\0", b"\0\0\0\0", b"\0\0\n", b"\n"] {
            // The public variant code is not a unique device identifier.
            let mut input = b"G0023L0000000000".to_vec();
            input.extend_from_slice(suffix);
            let mut reader = io::Cursor::new(input);
            assert!(read_supported_scribe(&mut reader).unwrap());
            assert_eq!(reader.position(), 16);
            assert_eq!(&reader.get_ref()[16..], suffix);
        }
    }

    #[test]
    fn proc_identity_transport_rejects_short_reads_and_malformed_fixed_width_identities() {
        let synthetic = b"G0023L0000000000";
        for length in 0..16 {
            assert_eq!(read_supported_scribe(&synthetic[..length]).unwrap_err().kind(),
                io::ErrorKind::UnexpectedEof);
        }
        for position in [0, 3, 5, 6, 15] {
            let mut input = synthetic.to_vec();
            input[position] = 0;
            input.extend_from_slice(b"\0\0\0");
            assert!(!read_supported_scribe(input.as_slice()).unwrap());
        }
        for input in [b"G001PX0000000000".as_slice(), b"G003H30000000000", b"B0023L0000000000",
            b"G0023l0000000000", b"G0023L000000000!"]
        {
            assert!(!read_supported_scribe(input).unwrap());
        }
    }

    struct IdentityReader {
        position: usize,
        fail_after: Option<usize>,
    }

    impl Read for IdentityReader {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            assert!(self.position < 16, "The identity reader must not inspect transport padding");
            assert!(output.len() <= 16 - self.position, "Every read must be bounded by the identity width");
            if self.fail_after.is_some_and(|limit| self.position >= limit) {
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }
            let limit = self.fail_after.unwrap_or(16);
            let count = output.len().min(3).min(limit - self.position);
            output[..count].copy_from_slice(&b"G0023L0000000000"[self.position..self.position + count]);
            self.position += count;
            Ok(count)
        }
    }

    #[test]
    fn proc_identity_transport_is_bounded_even_when_reads_are_fragmented() {
        let mut reader = IdentityReader { position: 0, fail_after: None };
        assert!(read_supported_scribe(&mut reader).unwrap());
        assert_eq!(reader.position, 16);
    }

    #[test]
    fn proc_identity_transport_does_not_claim_capability_after_read_errors() {
        for fail_after in [0, 6, 15] {
            let mut reader = IdentityReader { position: 0, fail_after: Some(fail_after) };
            assert_eq!(read_supported_scribe(&mut reader).unwrap_err().raw_os_error(), Some(libc::EIO));
            assert_eq!(reader.position, fail_after);
        }
    }

    #[test]
    fn device_identity_admits_only_reviewed_scribe_codes() {
        for code in ["27J", "2BL", "263", "227", "2BM", "23L", "23M", "270",
            "3V0", "3V1", "3X5", "3UV", "3X4", "3X3", "41E", "410"]
        {
            assert!(supported_scribe(format!("G00{code}0000000000\n").as_bytes()));
        }
        for value in ["G001PX0000000000", "G003H30000000000", "B002270000000000",
            "9002270000000000", "G00227000000000", "G0022700000000000",
            "G00227000000000!", "g002270000000000", "G00227é000000000", ""]
        {
            assert!(!supported_scribe(value.as_bytes()), "unexpected identity admitted");
        }
    }

    #[test]
    fn rejection_retains_the_raw_image_request_until_its_replacement() {
        let handle = DitheringHandle::for_test(true);
        assert!(handle.hardware_y4_available());
        assert!(!handle.hardware_y4_enabled());
        assert!(handle.set_hardware_y4(true));
        let active_revision = handle.revision();
        assert!(handle.set_hardware_y4(true));
        assert_eq!(handle.revision(), active_revision);
        handle.reject_hardware();
        assert!(!handle.hardware_y4_available());
        assert!(!handle.hardware_y4_enabled());
        assert!(handle.requested());
        assert_ne!(handle.revision(), active_revision);
        assert!(!handle.set_hardware_y4(false));
        assert!(!handle.requested());
        assert!(!handle.set_hardware_y4(true));
        assert!(handle.requested());
    }

    #[test]
    fn software_fallback_preserves_palette_and_uses_physical_phase() {
        for y in 0..4 {
            for x in 0..4 {
                for level in 0..=15 {
                    assert_eq!(software_gray(level * 17, x, y), level * 17);
                }
                for value in 0..=255 {
                    assert_eq!(software_gray(value, x, y) % 17, 0);
                    assert_eq!(software_gray(value, x + 4, y + 4), software_gray(value, x, y));
                }
            }
        }
        let tones = (0..16).map(|phase| software_gray(110, phase % 4, phase / 4))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(tones.into_iter().collect::<Vec<_>>(), [102, 119]);
    }
}
