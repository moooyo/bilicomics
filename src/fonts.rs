//! Incremental registration of build-expanded, immutable bundled fonts.

use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

use anyhow::{Context, Result, ensure};
use slint::{ComponentHandle, fontique_010::fontique};

use crate::ui::{AppWindow, Theme};

pub const STARTUP_FONT_FAMILY: &str = "BiliComics Startup";

struct BundledFont {
    encoded_bytes: usize,
    bytes: &'static [u8],
}

static BUNDLED_FONTS: &[BundledFont] = &[
    BundledFont {
        encoded_bytes: include_bytes!("../assets/fonts/NotoSansSC-Regular.otf.gz").len(),
        bytes: include_bytes!(concat!(env!("OUT_DIR"), "/fonts/NotoSansSC-Regular.otf")),
    },
    BundledFont {
        encoded_bytes: include_bytes!("../assets/fonts/NotoSansSC-Medium.otf.gz").len(),
        bytes: include_bytes!(concat!(env!("OUT_DIR"), "/fonts/NotoSansSC-Medium.otf")),
    },
    BundledFont {
        encoded_bytes: include_bytes!("../assets/fonts/NotoSansSC-Bold.otf.gz").len(),
        bytes: include_bytes!(concat!(env!("OUT_DIR"), "/fonts/NotoSansSC-Bold.otf")),
    },
    BundledFont {
        encoded_bytes: include_bytes!("../assets/fonts/NotoSerifSC-Regular.otf.gz").len(),
        bytes: include_bytes!(concat!(env!("OUT_DIR"), "/fonts/NotoSerifSC-Regular.otf")),
    },
    BundledFont {
        encoded_bytes: include_bytes!("../assets/fonts/NotoSerifSC-Bold.otf.gz").len(),
        bytes: include_bytes!(concat!(env!("OUT_DIR"), "/fonts/NotoSerifSC-Bold.otf")),
    },
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
    sans_family: Option<String>,
    serif_family: Option<String>,
}

impl Fonts {
    /// Valid, uncompressed startup font; usable before any worker is created.
    pub fn bootstrap_bytes() -> &'static [u8] {
        include_bytes!("../assets/fonts/StartupSansSC.otf")
    }

    pub fn register_bootstrap(ui: &AppWindow) -> Result<()> {
        let family = register_family(Arc::new(Self::bootstrap_bytes()))?;
        ensure!(family == STARTUP_FONT_FAMILY, "The startup font family does not match its provenance");
        ui.global::<Theme>().set_ui_font(family.clone().into());
        ui.global::<Theme>().set_serif_font(family.into());
        Ok(())
    }

    /// Compatibility path that prepares the complete bundle on a worker.
    pub fn load() -> Result<Self> {
        let mut fonts = Self::default();
        Self::prepare_faces(&AtomicBool::new(false), |face| fonts.accept(face))?;
        Ok(fonts)
    }

    /// Share static ELF-backed faces without runtime expansion or full copies.
    /// Registration remains ordered and leaves UI families unchanged until finish.
    pub fn prepare_faces(cancelled: &AtomicBool, mut deliver: impl FnMut(PreparedFont) -> Result<()>) -> Result<()> {
        for (index, font) in BUNDLED_FONTS.iter().enumerate() {
            if cancelled.load(Ordering::Acquire) { return Ok(()); }
            let span = crate::performance::enabled().then(crate::performance::Span::start);
            let bytes: FontBytes = Arc::new(font.bytes);
            if let Some(span) = span {
                let sample = span.finish();
                // Delivery and event-loop notification are outside this span.
                crate::performance::startup("font-face-prepare", &serde_json::json!({
                    "face_index": index,
                    "encoded_bytes": font.encoded_bytes,
                    "decoded_bytes": font.bytes.len(),
                    "storage": "static-embedded",
                    "wall_us": sample.wall_us,
                    "cpu_us": sample.cpu_us,
                    "outcome": "prepared",
                }));
            }
            if cancelled.load(Ordering::Acquire) { return Ok(()); }
            deliver(PreparedFont { index, bytes })?;
        }
        Ok(())
    }

    pub fn accept(&mut self, face: PreparedFont) -> Result<()> {
        ensure!(face.index == self.faces.len() && face.index < BUNDLED_FONTS.len(),
            "Bundled font faces arrived out of order");
        self.faces.push(face.bytes);
        Ok(())
    }

    pub fn can_register_next(&self) -> bool {
        self.registered < self.faces.len()
    }

    pub fn registered_count(&self) -> usize {
        self.registered
    }

    pub fn default_bytes(&self) -> &[u8] {
        self.faces[0].as_ref().as_ref()
    }

    /// Register at most one face on the UI thread, then yield to the event loop.
    /// A true result means all faces have been registered.
    pub fn register_next(&mut self) -> Result<bool> {
        let Some(bytes) = self.faces.get(self.registered) else { return Ok(self.registered == BUNDLED_FONTS.len()); };
        let family = register_family(bytes.clone())?;
        if self.registered == 0 { self.sans_family = Some(family.clone()); }
        if self.registered == 3 { self.serif_family = Some(family); }
        self.registered += 1;
        Ok(self.registered == BUNDLED_FONTS.len())
    }

    /// Switch business UI families only after every requested face is available.
    pub fn finish(&self, ui: &AppWindow) -> Result<()> {
        ensure!(self.registered == BUNDLED_FONTS.len(), "Bundled font registration is not complete");
        ui.global::<Theme>().set_ui_font(self.sans_family.as_ref().context("Missing Sans font")?.clone().into());
        ui.global::<Theme>().set_serif_font(self.serif_family.as_ref().context("Missing Serif font")?.clone().into());
        Ok(())
    }

    /// Compatibility path for callers without phased startup.
    pub fn register(&self, ui: &AppWindow) -> Result<()> {
        let mut sans = None;
        let mut serif = None;
        for (index, bytes) in self.faces.iter().enumerate() {
            let family = register_family(bytes.clone())?;
            if index == 0 {
                sans = Some(family.clone());
            }
            if index == 3 {
                serif = Some(family);
            }
        }
        ui.global::<Theme>().set_ui_font(sans.context("Missing Sans font")?.into());
        ui.global::<Theme>().set_serif_font(serif.context("Missing Serif font")?.into());
        Ok(())
    }
}

fn register_family(bytes: FontBytes) -> Result<String> {
    let mut collection = slint::fontique_010::shared_collection();
    let families = collection.register_fonts(fontique::Blob::new(bytes), None);
    families.first().and_then(|(id, _)| collection.family_name(*id))
        .map(str::to_owned).context("Register a bundled font family")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partial_delivery_is_not_a_complete_registration() {
        let mut fonts = Fonts::default();
        assert!(!fonts.register_next().unwrap());
        assert!(!fonts.can_register_next());
        fonts.accept(PreparedFont { index: 0, bytes: Arc::new(Vec::new()) }).unwrap();
        assert!(fonts.can_register_next());
        assert_eq!(fonts.registered_count(), 0);
    }

    #[test]
    fn an_out_of_order_or_repeated_face_is_rejected_without_replacing_prior_bytes() {
        let mut fonts = Fonts::default();
        assert!(fonts.accept(PreparedFont { index: 1, bytes: Arc::new(vec![2]) }).is_err());
        fonts.accept(PreparedFont { index: 0, bytes: Arc::new(vec![1]) }).unwrap();
        assert!(fonts.accept(PreparedFont { index: 0, bytes: Arc::new(vec![9]) }).is_err());
        assert_eq!(fonts.default_bytes(), &[1]);
        assert_eq!(fonts.faces.len(), 1);
    }

    #[test]
    fn cancellation_before_work_does_not_deliver_any_face() {
        let cancelled = AtomicBool::new(true);
        let mut deliveries = 0;
        Fonts::prepare_faces(&cancelled, |_| { deliveries += 1; Ok(()) }).unwrap();
        assert_eq!(deliveries, 0);
    }

    #[test]
    fn cancellation_after_delivery_stops_remaining_faces() {
        let cancelled = AtomicBool::new(false);
        let mut deliveries = 0;
        Fonts::prepare_faces(&cancelled, |_| {
            deliveries += 1;
            cancelled.store(true, Ordering::Release);
            Ok(())
        }).unwrap();
        assert_eq!(deliveries, 1);
    }

    #[test]
    fn failed_delivery_stops_remaining_faces() {
        let mut deliveries = 0;
        let result = Fonts::prepare_faces(&AtomicBool::new(false), |_| {
            deliveries += 1;
            anyhow::bail!("Rejected font delivery")
        });
        assert!(result.is_err());
        assert_eq!(deliveries, 1);
    }

    #[test]
    fn expanded_faces_match_the_canonical_gzip_sources() {
        use flate2::read::GzDecoder;
        use std::io::Read;

        let canonical: &[&[u8]] = &[
            include_bytes!("../assets/fonts/NotoSansSC-Regular.otf.gz"),
            include_bytes!("../assets/fonts/NotoSansSC-Medium.otf.gz"),
            include_bytes!("../assets/fonts/NotoSansSC-Bold.otf.gz"),
            include_bytes!("../assets/fonts/NotoSerifSC-Regular.otf.gz"),
            include_bytes!("../assets/fonts/NotoSerifSC-Bold.otf.gz"),
        ];
        assert_eq!(canonical.len(), BUNDLED_FONTS.len());
        for (index, (font, compressed)) in BUNDLED_FONTS.iter().zip(canonical).enumerate() {
            let mut original = Vec::new();
            GzDecoder::new(*compressed).take(64 * 1024 * 1024 + 1)
                .read_to_end(&mut original).unwrap();
            assert!(original.len() <= 64 * 1024 * 1024);
            assert_eq!(font.encoded_bytes, compressed.len());
            assert!(font.bytes == original.as_slice(), "Expanded font bytes changed at index {index}");
        }
    }

    #[test]
    fn prepared_faces_share_the_embedded_payloads() {
        let mut deliveries = 0;
        Fonts::prepare_faces(&AtomicBool::new(false), |face| {
            let bytes = face.bytes.as_ref().as_ref();
            let embedded = BUNDLED_FONTS[face.index].bytes;
            assert_eq!(bytes.as_ptr(), embedded.as_ptr());
            assert_eq!(bytes.len(), embedded.len());
            deliveries += 1;
            Ok(())
        }).unwrap();
        assert_eq!(deliveries, BUNDLED_FONTS.len());
    }
}
