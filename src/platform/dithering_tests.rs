use super::*;

struct RecordingDitheringSink {
    available: AtomicBool,
    requested: AtomicBool,
}

impl RecordingDitheringSink {
    fn new(available: bool) -> Self {
        Self {
            available: AtomicBool::new(available),
            requested: AtomicBool::new(false),
        }
    }
}

impl DitheringSink for RecordingDitheringSink {
    fn hardware_y4_available(&self) -> bool {
        self.available.load(Ordering::Acquire)
    }

    fn set_hardware_y4(&self, enabled: bool) -> bool {
        self.requested.store(enabled, Ordering::Release);
        enabled && self.hardware_y4_available()
    }
}

#[test]
fn a_desktop_profile_never_claims_hardware_dithering() -> anyhow::Result<()> {
    let platform = Platform::new(DeviceProfile::Scribe, false)?;
    assert!(!platform.hardware_y4_available());
    assert!(!platform.set_reader_hardware_y4(true));
    assert!(platform.reader_hardware_y4_requested());
    platform.set_reader_hardware_y4(false);
    assert!(!platform.reader_hardware_y4_requested());
    Ok(())
}

#[test]
fn native_capability_loss_preserves_the_displayed_frames_fallback_intent() -> anyhow::Result<()> {
    let platform = Platform::new(DeviceProfile::Scribe, false)?;
    let sink = Arc::new(RecordingDitheringSink::new(true));
    platform.set_dithering_sink(sink.clone())?;
    assert!(platform.hardware_y4_available());
    assert!(platform.set_reader_hardware_y4(true));
    sink.available.store(false, Ordering::Release);
    assert!(!platform.hardware_y4_available());
    assert!(platform.reader_hardware_y4_requested());
    assert!(sink.requested.load(Ordering::Acquire));
    assert!(!platform.set_reader_hardware_y4(true));
    assert!(sink.requested.load(Ordering::Acquire));
    assert!(!platform.set_reader_hardware_y4(false));
    assert!(!platform.reader_hardware_y4_requested());
    assert!(!sink.requested.load(Ordering::Acquire));
    Ok(())
}

#[test]
fn attaching_a_sink_does_not_change_the_retained_frame_format() -> anyhow::Result<()> {
    let platform = Platform::new(DeviceProfile::Paperwhite5, false)?;
    platform.set_reader_hardware_y4(true);
    let sink = Arc::new(RecordingDitheringSink::new(false));
    platform.set_dithering_sink(sink.clone())?;
    assert!(!platform.hardware_y4_available());
    assert!(platform.reader_hardware_y4_requested());
    assert!(sink.requested.load(Ordering::Acquire));
    Ok(())
}
