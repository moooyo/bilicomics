//! Localized capability explanations shared by both reading settings views.

use crate::{i18n, native_media::Availability};

pub(crate) fn availability_note(availability: Availability) -> String {
    let key = if availability.available() {
        "media_jpeg_available".to_owned()
    } else {
        format!("media_unavailable_{}", availability.reason())
    };
    i18n::text(&key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_capability_has_a_localized_explanation() {
        for availability in [
            Availability::Available, Availability::UnsupportedPlatform,
            Availability::UnverifiedDevice,
            Availability::MissingHelper, Availability::MissingDriver,
            Availability::MissingDiagnostics, Availability::Busy, Availability::Failed,
        ] {
            let note = availability_note(availability);
            assert!(!note.is_empty());
            assert!(!note.starts_with("media_"));
        }
    }
}
