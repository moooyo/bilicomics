use std::str::FromStr;

/// A design profile and native framebuffer expectation, not a hardware compatibility claim.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DeviceProfile {
    Paperwhite5,
    #[default]
    Paperwhite6,
    Scribe,
    Scribe2025,
}

impl DeviceProfile {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "pw5" | "paperwhite5" => Some(Self::Paperwhite5),
            "pw6" | "paperwhite6" => Some(Self::Paperwhite6),
            "scribe" | "scribe2022" | "scribe2024" => Some(Self::Scribe),
            "scribe2025" => Some(Self::Scribe2025),
            _ => None,
        }
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::Paperwhite5 => "pw5",
            Self::Paperwhite6 => "pw6",
            Self::Scribe => "scribe",
            Self::Scribe2025 => "scribe2025",
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Paperwhite5 => "Kindle Paperwhite 5",
            Self::Paperwhite6 => "Kindle Paperwhite 6",
            Self::Scribe => "Kindle Scribe (2022 / 2024)",
            Self::Scribe2025 => "Kindle Scribe (2025)",
        }
    }

    /// Desktop design canvas in logical pixels; not necessarily half the native framebuffer.
    pub const fn logical_size(self) -> (u32, u32) {
        match self {
            Self::Paperwhite5 => (618, 824),
            Self::Paperwhite6 => (632, 840),
            Self::Scribe => (930, 1240),
            Self::Scribe2025 => (990, 1320),
        }
    }

    /// Expected visible framebuffer in pixels, independent of the reference artwork.
    pub const fn native_framebuffer_size(self) -> (u32, u32) {
        match self {
            Self::Paperwhite5 => (1236, 1648),
            // The stock Sangria device tree declares this for both panel and display.
            Self::Paperwhite6 => (1272, 1696),
            Self::Scribe => (1860, 2480),
            Self::Scribe2025 => (1980, 2640),
        }
    }

    pub const fn is_scribe(self) -> bool {
        self.logical_size().0 >= 900
    }
}

impl FromStr for DeviceProfile {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value).ok_or("unknown device profile; use pw5, pw6, scribe, or scribe2025")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_framebuffers_do_not_inherit_desktop_design_sizes() {
        for (profile, design, native, scribe) in [
            (DeviceProfile::Paperwhite5, (618, 824), (1236, 1648), false),
            (DeviceProfile::Paperwhite6, (632, 840), (1272, 1696), false),
            (DeviceProfile::Scribe, (930, 1240), (1860, 2480), true),
            (DeviceProfile::Scribe2025, (990, 1320), (1980, 2640), true),
        ] {
            assert_eq!(profile.logical_size(), design);
            assert_eq!(profile.native_framebuffer_size(), native);
            assert_eq!(profile.is_scribe(), scribe);
        }
    }

    #[test]
    fn unknown_profiles_are_not_silently_assumed_compatible() {
        assert!(DeviceProfile::parse("colorsoft").is_none());
        assert!(DeviceProfile::parse("unknown").is_none());
    }
}
