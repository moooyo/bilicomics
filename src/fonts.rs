use std::{io::Read, sync::{Arc, atomic::{AtomicBool, Ordering}}};

use anyhow::{Context, Result, bail, ensure};
use flate2::read::GzDecoder;
use slint::{ComponentHandle, fontique_010::fontique};

use crate::ui::{AppWindow, Theme};

const MAX_FONT_BYTES: u64 = 64 * 1024 * 1024;
const FONT_FACE_COUNT: usize = 5;
pub const STARTUP_FONT_FAMILY: &str = "BiliComics Startup";

pub struct PreparedFont {
    index: usize,
    bytes: Arc<Vec<u8>>,
}

#[derive(Default)]
pub struct Fonts {
    faces: Vec<Arc<Vec<u8>>>,
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
        let family = register_family(Arc::new(Self::bootstrap_bytes().to_vec()))?;
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

    /// Deliver each immutable face as soon as its decompression is complete.
    /// Registration may overlap later decompression without changing UI families.
    pub fn prepare_faces(cancelled: &AtomicBool, mut deliver: impl FnMut(PreparedFont) -> Result<()>) -> Result<()> {
        let compressed: &[&[u8]] = &[
            include_bytes!("../assets/fonts/NotoSansSC-Regular.otf.gz"),
            include_bytes!("../assets/fonts/NotoSansSC-Medium.otf.gz"),
            include_bytes!("../assets/fonts/NotoSansSC-Bold.otf.gz"),
            include_bytes!("../assets/fonts/NotoSerifSC-Regular.otf.gz"),
            include_bytes!("../assets/fonts/NotoSerifSC-Bold.otf.gz"),
        ];
        for (index, bytes) in compressed.iter().enumerate() {
            if cancelled.load(Ordering::Acquire) { return Ok(()); }
            let span = crate::performance::enabled().then(crate::performance::Span::start);
            let prepared = (|| -> Result<Vec<u8>> {
                let mut decoded = Vec::new();
                GzDecoder::new(*bytes)
                    .take(MAX_FONT_BYTES + 1)
                    .read_to_end(&mut decoded)
                    .context("Read bundled font")?;
                if decoded.len() as u64 > MAX_FONT_BYTES {
                    bail!("The bundled font exceeds the font size limit");
                }
                Ok(decoded)
            })();
            if let Some(span) = span {
                let sample = span.finish();
                // Delivery and event-loop notification are outside this span.
                crate::performance::startup("font-face-prepare", &serde_json::json!({
                    "face_index": index,
                    "encoded_bytes": bytes.len(),
                    "decoded_bytes": prepared.as_ref().ok().map(Vec::len),
                    "wall_us": sample.wall_us,
                    "cpu_us": sample.cpu_us,
                    "outcome": if prepared.is_ok() { "prepared" } else { "failed" },
                }));
            }
            let decoded = prepared?;
            if cancelled.load(Ordering::Acquire) { return Ok(()); }
            deliver(PreparedFont { index, bytes: Arc::new(decoded) })?;
        }
        Ok(())
    }

    pub fn accept(&mut self, face: PreparedFont) -> Result<()> {
        ensure!(face.index == self.faces.len() && face.index < FONT_FACE_COUNT,
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
        self.faces[0].as_slice()
    }

    /// Register at most one face on the UI thread, then yield to the event loop.
    /// A true result means all faces have been registered.
    pub fn register_next(&mut self) -> Result<bool> {
        let Some(bytes) = self.faces.get(self.registered) else { return Ok(self.registered == FONT_FACE_COUNT); };
        let family = register_family(bytes.clone())?;
        if self.registered == 0 { self.sans_family = Some(family.clone()); }
        if self.registered == 3 { self.serif_family = Some(family); }
        self.registered += 1;
        Ok(self.registered == FONT_FACE_COUNT)
    }

    /// Switch business UI families only after every requested face is available.
    pub fn finish(&self, ui: &AppWindow) -> Result<()> {
        ensure!(self.registered == FONT_FACE_COUNT, "Bundled font registration is not complete");
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

fn register_family(bytes: Arc<Vec<u8>>) -> Result<String> {
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
    fn cancellation_before_work_does_not_decode_or_deliver_any_face() {
        let cancelled = AtomicBool::new(true);
        let mut deliveries = 0;
        Fonts::prepare_faces(&cancelled, |_| { deliveries += 1; Ok(()) }).unwrap();
        assert_eq!(deliveries, 0);
    }
}
