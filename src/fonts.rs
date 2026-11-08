use std::{io::Read, sync::Arc};

use anyhow::{Context, Result, bail, ensure};
use flate2::read::GzDecoder;
use slint::{ComponentHandle, fontique_010::fontique};

use crate::ui::{AppWindow, Theme};

const MAX_FONT_BYTES: u64 = 64 * 1024 * 1024;
pub const STARTUP_FONT_FAMILY: &str = "BiliComics Startup";

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

    /// Decompression performs no UI or process-environment mutation.
    /// Call on the startup worker after backend installation.
    pub fn load() -> Result<Self> {
        let compressed: &[&[u8]] = &[
            include_bytes!("../assets/fonts/NotoSansSC-Regular.otf.gz"),
            include_bytes!("../assets/fonts/NotoSansSC-Medium.otf.gz"),
            include_bytes!("../assets/fonts/NotoSansSC-Bold.otf.gz"),
            include_bytes!("../assets/fonts/NotoSerifSC-Regular.otf.gz"),
            include_bytes!("../assets/fonts/NotoSerifSC-Bold.otf.gz"),
        ];
        let mut faces = Vec::new();
        for bytes in compressed {
            let mut decoded = Vec::new();
            GzDecoder::new(*bytes)
                .take(MAX_FONT_BYTES + 1)
                .read_to_end(&mut decoded)
                .context("Read bundled font")?;
            if decoded.len() as u64 > MAX_FONT_BYTES {
                bail!("The bundled font exceeds the font size limit");
            }
            faces.push(Arc::new(decoded));
        }
        Ok(Self { faces, registered: 0, sans_family: None, serif_family: None })
    }

    pub fn default_bytes(&self) -> &[u8] {
        self.faces[0].as_slice()
    }

    /// Register at most one face on the UI thread, then yield to the event loop.
    /// A true result means all faces have been registered.
    pub fn register_next(&mut self) -> Result<bool> {
        let Some(bytes) = self.faces.get(self.registered) else { return Ok(true); };
        let family = register_family(bytes.clone())?;
        if self.registered == 0 { self.sans_family = Some(family.clone()); }
        if self.registered == 3 { self.serif_family = Some(family); }
        self.registered += 1;
        Ok(self.registered == self.faces.len())
    }

    /// Switch business UI families only after every requested face is available.
    pub fn finish(&self, ui: &AppWindow) -> Result<()> {
        ensure!(self.registered == self.faces.len(), "Bundled font registration is not complete");
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
