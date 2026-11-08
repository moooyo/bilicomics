use std::str::FromStr;

/// A design geometry, not a declaration of hardware compatibility.
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

    pub const fn logical_size(self) -> (u32, u32) {
        match self {
            Self::Paperwhite5 => (618, 824),
            Self::Paperwhite6 => (632, 840),
            Self::Scribe => (930, 1240),
            Self::Scribe2025 => (990, 1320),
        }
    }

    pub const fn physical_size(self) -> (u32, u32) {
        let (width, height) = self.logical_size();
        (width * 2, height * 2)
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
    fn design_units_have_the_same_physical_scale_on_every_profile() {
        for profile in [
            DeviceProfile::Paperwhite5,
            DeviceProfile::Paperwhite6,
            DeviceProfile::Scribe,
            DeviceProfile::Scribe2025,
        ] {
            let logical = profile.logical_size();
            assert_eq!(profile.physical_size(), (logical.0 * 2, logical.1 * 2));
            assert_eq!(profile.is_scribe(), logical.0 >= 900);
        }
    }

    #[test]
    fn unknown_profiles_are_not_silently_assumed_compatible() {
        assert!(DeviceProfile::parse("colorsoft").is_none());
        assert!(DeviceProfile::parse("unknown").is_none());
    }
}
