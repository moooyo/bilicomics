//! Incremental registration of build-expanded, immutable bundled fonts.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use aidoku_kindle::ui::{AppWindow, Theme};
use anyhow::{Result, ensure};
use slint::{ComponentHandle, fontique_010::fontique};

const STARTUP_FAMILY: &str = "BiliComics Startup";
static BUNDLED_FONTS: &[(&str, f32, &[u8])] = &[
    (
        "Noto Sans CJK SC",
        400.0,
        include_bytes!(concat!(env!("OUT_DIR"), "/fonts/NotoSansSC-Regular.otf")),
    ),
    (
        "Noto Sans CJK SC",
        500.0,
        include_bytes!(concat!(env!("OUT_DIR"), "/fonts/NotoSansSC-Medium.otf")),
    ),
    (
        "Noto Sans CJK SC",
        700.0,
        include_bytes!(concat!(env!("OUT_DIR"), "/fonts/NotoSansSC-Bold.otf")),
    ),
    (
        "Noto Serif CJK SC",
        400.0,
        include_bytes!(concat!(env!("OUT_DIR"), "/fonts/NotoSerifSC-Regular.otf")),
    ),
    (
        "Noto Serif CJK SC",
        700.0,
        include_bytes!(concat!(env!("OUT_DIR"), "/fonts/NotoSerifSC-Bold.otf")),
    ),
    (
        "Noto Serif CJK SC",
        900.0,
        include_bytes!(concat!(env!("OUT_DIR"), "/fonts/NotoSerifSC-Black.otf")),
    ),
    (
        "Literata",
        400.0,
        include_bytes!(concat!(env!("OUT_DIR"), "/fonts/Literata-Regular.ttf")),
    ),
    (
        "Literata",
        600.0,
        include_bytes!(concat!(env!("OUT_DIR"), "/fonts/Literata-SemiBold.ttf")),
    ),
    (
        "Literata",
        700.0,
        include_bytes!(concat!(env!("OUT_DIR"), "/fonts/Literata-Bold.ttf")),
    ),
];

type FontBytes = Arc<dyn AsRef<[u8]> + Send + Sync>;

pub struct PreparedFont {
    index: usize,
    bytes: FontBytes,
}

#[derive(Default)]
pub struct Fonts {
    faces: Vec<FontBytes>,
    registered: usize,
}

impl Fonts {
    /// Valid uncompressed bytes keep backend setup ahead of every worker.
    pub fn bootstrap_bytes() -> &'static [u8] {
        include_bytes!("../assets/fonts/StartupSansSC.otf")
    }

    pub fn register_bootstrap(ui: &AppWindow) -> Result<()> {
        register_face(Arc::new(Self::bootstrap_bytes()), STARTUP_FAMILY, 400.0)?;
        ui.global::<Theme>().set_sans(STARTUP_FAMILY.into());
        ui.global::<Theme>().set_serif(STARTUP_FAMILY.into());
        ui.global::<Theme>().set_numbers(STARTUP_FAMILY.into());
        Ok(())
    }

    /// Share static ELF-backed bytes without runtime expansion or full copies.
    pub fn prepare_faces(
        cancelled: &AtomicBool,
        mut deliver: impl FnMut(PreparedFont) -> Result<()>,
    ) -> Result<()> {
        for (index, (_, _, bytes)) in BUNDLED_FONTS.iter().enumerate() {
            if cancelled.load(Ordering::Acquire) {
                return Ok(());
            }
            deliver(PreparedFont {
                index,
                bytes: Arc::new(*bytes),
            })?;
        }
        aidoku_kindle::reader_timing::mark("fonts_loaded");
        Ok(())
    }

    pub fn accept(&mut self, face: PreparedFont) -> Result<()> {
        ensure!(
            face.index == self.faces.len() && face.index < BUNDLED_FONTS.len(),
            "Bundled font faces arrived out of order"
        );
        self.faces.push(face.bytes);
        Ok(())
    }

    pub fn can_register_next(&self) -> bool {
        self.registered < self.faces.len()
    }

    /// Register one face per event-loop turn without switching UI families.
    pub fn register_next(&mut self) -> Result<bool> {
        let Some(bytes) = self.faces.get(self.registered) else {
            return Ok(self.registered == BUNDLED_FONTS.len());
        };
        let (family, weight, _) = BUNDLED_FONTS[self.registered];
        register_face(bytes.clone(), family, weight)?;
        self.registered += 1;
        Ok(self.registered == BUNDLED_FONTS.len())
    }

    /// Publish the complete family set only after every requested weight exists.
    pub fn finish(&self, ui: &AppWindow) -> Result<()> {
        ensure!(
            self.registered == BUNDLED_FONTS.len(),
            "Bundled font registration is not complete"
        );
        ui.global::<Theme>().set_sans("Noto Sans CJK SC".into());
        ui.global::<Theme>().set_serif("Noto Serif CJK SC".into());
        ui.global::<Theme>().set_numbers("Literata".into());
        crate::FONT_INITIALIZATIONS.fetch_add(1, Ordering::AcqRel);
        aidoku_kindle::reader_timing::mark("fonts_registered");
        Ok(())
    }
}

fn register_face(bytes: FontBytes, family: &str, weight: f32) -> Result<()> {
    let mut collection = slint::fontique_010::shared_collection();
    let registered = collection.register_fonts(fontique::Blob::new(bytes), None);
    ensure!(
        registered.iter().any(|(id, faces)| {
            collection.family_name(*id) == Some(family)
                && faces.iter().any(|face| face.weight().value() == weight)
        }),
        "Bundled font did not register as {family} weight {weight}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_font_delivery_cannot_finish_the_family_switch() {
        let mut fonts = Fonts::default();
        assert!(!fonts.can_register_next());
        assert!(!fonts.register_next().unwrap());
        fonts
            .accept(PreparedFont {
                index: 0,
                bytes: Arc::new(vec![1]),
            })
            .unwrap();
        assert!(fonts.can_register_next());
        assert_eq!(fonts.registered, 0);
    }

    #[test]
    fn out_of_order_or_repeated_faces_preserve_existing_bytes() {
        let mut fonts = Fonts::default();
        assert!(
            fonts
                .accept(PreparedFont {
                    index: 1,
                    bytes: Arc::new(vec![2]),
                })
                .is_err()
        );
        fonts
            .accept(PreparedFont {
                index: 0,
                bytes: Arc::new(vec![1]),
            })
            .unwrap();
        assert!(
            fonts
                .accept(PreparedFont {
                    index: 0,
                    bytes: Arc::new(vec![9]),
                })
                .is_err()
        );
        assert_eq!(fonts.faces[0].as_ref().as_ref(), &[1]);
    }

    #[test]
    fn cancellation_before_work_does_not_publish_a_face() {
        let mut deliveries = 0;
        Fonts::prepare_faces(&AtomicBool::new(true), |_| {
            deliveries += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(deliveries, 0);
    }

    #[test]
    fn expanded_fonts_match_the_existing_published_payloads() {
        use sha2::{Digest, Sha256};
        use std::collections::BTreeSet;

        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../assets/fonts/manifest.json")).unwrap();
        let expected = manifest["fonts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|font| {
                (
                    font["uncompressed_sha256"].as_str().unwrap().to_owned(),
                    font["uncompressed_bytes"].as_u64().unwrap(),
                )
            })
            .collect::<BTreeSet<_>>();
        let observed = BUNDLED_FONTS
            .iter()
            .map(|(_, _, bytes)| (format!("{:x}", Sha256::digest(bytes)), bytes.len() as u64))
            .collect::<BTreeSet<_>>();
        assert_eq!(observed.len(), BUNDLED_FONTS.len());
        assert_eq!(observed, expected);
    }

    #[test]
    fn prepared_faces_borrow_the_embedded_payloads() {
        let mut delivered = 0;
        Fonts::prepare_faces(&AtomicBool::new(false), |face| {
            let bytes = face.bytes.as_ref().as_ref();
            assert_eq!(bytes.as_ptr(), BUNDLED_FONTS[face.index].2.as_ptr());
            assert_eq!(bytes.len(), BUNDLED_FONTS[face.index].2.len());
            delivered += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(delivered, BUNDLED_FONTS.len());
    }
}
