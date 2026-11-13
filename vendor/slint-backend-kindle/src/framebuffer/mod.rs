mod ffi;
mod probe;
pub(crate) use probe::probe_native_framebuffer;
pub(crate) use probe::probe_output_inversion;
#[cfg(test)]
mod output_tests;
#[cfg(test)]
mod refresh_submission_tests;
#[cfg(test)]
mod dithering_tests;
#[cfg(test)]
mod test_support;

use crate::refresh::{RefreshWaveform, ResolvedRefresh};
use crate::dithering::{DitheringHandle, software_gray};
use std::cell::Cell;
use std::ops::Range;
use std::os::fd::AsRawFd;

use ffi::{
    EPDC_FLAG_FORCE_MONOCHROME, MTK_EPDC_FLAG_USE_DITHERING_Y4,
    FBIOGET_FSCREENINFO, FBIOGET_VSCREENINFO, FbFixScreeninfo,
    FbVarScreeninfo, MXCFB_SEND_UPDATE, MXCFB_SEND_UPDATE_MTK, MXCFB_SEND_UPDATE_REX,
    MXCFB_SEND_UPDATE_ZELDA, MXCFB_WAIT_FOR_UPDATE_COMPLETE, TEMP_USE_AMBIENT, UPDATE_MODE_FULL,
    UPDATE_MODE_PARTIAL, UpdateMarkerData, UpdateRect, UpdateRequest, UpdateRequestMtk,
    UpdateRequestRex, UpdateRequestZelda, WAVEFORM_MODE_A2_LEGACY, WAVEFORM_MODE_A2_MODERN,
    WAVEFORM_MODE_AUTO, WAVEFORM_MODE_DU, WAVEFORM_MODE_GC16,
};

/// Which MXCFB update ioctl this kernel accepts, varies with the Kindle model
///
//We probe on first refresh and remember the winner instead of retrying the failing ioctl every frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UpdateVariant {
    /// `MXCFB_SEND_UPDATE` — 72-byte struct, older devices.
    Legacy,
    /// `MXCFB_SEND_UPDATE_REX` — 80-byte struct, Paperwhite 10th gen and newer.
    Rex,
    /// `MXCFB_SEND_UPDATE_ZELDA` — 88-byte struct, KOA2/KOA3 (Oasis 2/3).
    Zelda,
    /// `MXCFB_SEND_UPDATE_MTK` — 96-byte struct, MediaTek devices (Basic 2022,
    /// Paperwhite 5, Scribe).
    Mtk,
    /// No variant was accepted. Cached so a hopeless kernel costs one probe
    /// and one error log total, not three failing ioctls per frame.
    Unsupported,
}

impl UpdateVariant {
    /// Probed oldest ABI first.
    const PROBE_ORDER: [UpdateVariant; 4] = [Self::Legacy, Self::Rex, Self::Zelda, Self::Mtk];
}

/// Diagnostic result of submitting an update, not proof of panel completion.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RefreshSubmission {
    pub ioctl_attempts: u32,
    pub accepted: bool,
    pub last_errno: Option<i32>,
    pub y4_attempted: bool,
    pub software_dither_fallback: bool,
    pub dithering_delivery_uncertain: bool,
}

#[cfg(test)]
fn refresh_submission_with(
    cached: Option<UpdateVariant>,
    width: u32,
    height: u32,
    submit: impl FnMut(UpdateVariant) -> Result<(), Option<i32>>,
) -> (Option<UpdateVariant>, RefreshSubmission) {
    refresh_submission_with_policy(cached, width, height, false, submit)
}

fn refresh_submission_with_policy(
    cached: Option<UpdateVariant>,
    width: u32,
    height: u32,
    stop_on_uncertain: bool,
    mut submit: impl FnMut(UpdateVariant) -> Result<(), Option<i32>>,
) -> (Option<UpdateVariant>, RefreshSubmission) {
    if width == 0 || height == 0 || matches!(cached, Some(UpdateVariant::Unsupported)) {
        return (cached, RefreshSubmission::default());
    }
    if let Some(variant) = cached {
        let result = submit(variant);
        return (cached, RefreshSubmission {
            ioctl_attempts: 1,
            accepted: result.is_ok(),
            last_errno: result.err().flatten(),
            ..Default::default()
        });
    }

    let mut submission = RefreshSubmission::default();
    for variant in UpdateVariant::PROBE_ORDER {
        submission.ioctl_attempts += 1;
        match submit(variant) {
            Ok(()) => {
                submission.accepted = true;
                submission.last_errno = None;
                return (Some(variant), submission);
            }
            Err(errno) => {
                submission.last_errno = errno;
                if stop_on_uncertain && !explicit_update_rejection(errno) {
                    return (None, submission);
                }
            }
        }
    }
    (Some(UpdateVariant::Unsupported), submission)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FramebufferLayout {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) stride: usize,
    pub(crate) bytes_per_pixel: usize,
    pub(crate) bits_per_pixel: u32,
}

/// Memory-mapped handle to the Kindle's e-ink framebuffer.
///
/// Pixel format is 8-bit grayscale (one byte per pixel) on most models, or
/// 32 bpp on color models like the Colorsoft. The `stride` may be wider than
/// `width * bytes_per_pixel` due to hardware alignment requirements.
pub(crate) struct Framebuffer {
    file: std::fs::File,
    map: *mut u8,
    len: usize,
    pub(crate) width: u32,
    pub(crate) height: u32,
    stride: usize,
    /// Bytes per pixel in the mmap: 1 (8-bit grayscale, most Kindles) or
    /// 4 (32-bit BGRA/RGBA, e.g. Kindle Colorsoft).
    bytes_per_pixel: usize,
    /// Compensate native output polarity without changing logical scene pixels.
    output_inverted: bool,
    dithering: DitheringHandle,
    bilevel: bool,
    /// Once an update has uncertain delivery, this mapping cannot safely be
    /// rewritten or resubmitted, including after a later scene/mode revision.
    dithering_delivery_failure: Cell<Option<Option<i32>>>,
    /// The update ioctl variant this kernel accepts, cached after the first
    /// successful refresh. `None` until then.
    update_variant: Cell<Option<UpdateVariant>>,
    /// Each logical update owns an independent marker, including failed attempts.
    last_issued_marker: Cell<u32>,
    last_submitted_marker: Cell<Option<u32>>,
    last_completed_marker: Cell<Option<u32>>,
}

// SAFETY: The mmap is process-wide and we only access it from the event loop thread.
unsafe impl Send for Framebuffer {}

impl Framebuffer {
    /// Copy the retained mapping layout without touching its pixels or devices.
    pub(crate) fn retained_layout(&self) -> FramebufferLayout {
        FramebufferLayout {
            width: self.width,
            height: self.height,
            stride: self.stride,
            bytes_per_pixel: self.bytes_per_pixel,
            // The mapping admits only the existing 8-bit and 32-bit paths.
            bits_per_pixel: self.bytes_per_pixel as u32 * 8,
        }
    }

    /// Open the framebuffer device and query its geometry from the kernel.
    ///
    /// This works on any Kindle model - the resolution and stride are read at
    /// runtime rather than being hardcoded.
    pub(crate) fn open() -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/fb0")?;

        let fd = file.as_raw_fd();

        let mut vinfo = FbVarScreeninfo::default();
        if unsafe {
            libc::ioctl(
                fd,
                FBIOGET_VSCREENINFO as _,
                &mut vinfo as *mut _ as *mut libc::c_void,
            )
        } == -1
        {
            return Err(std::io::Error::last_os_error());
        }

        let mut finfo = FbFixScreeninfo::default();
        if unsafe {
            libc::ioctl(
                fd,
                FBIOGET_FSCREENINFO as _,
                &mut finfo as *mut _ as *mut libc::c_void,
            )
        } == -1
        {
            return Err(std::io::Error::last_os_error());
        }

        let width = vinfo.xres;
        let height = vinfo.yres;
        let stride = finfo.line_length as usize;

        // Most Kindles expose an 8-bit grayscale framebuffer, but color models
        // (Kindle Colorsoft) expose a 32 bpp one. The render path produces one
        // gray byte per pixel; for 32 bpp we replicate it into all three color
        // channels, so the byte order (BGRA vs RGBA) doesn't matter.
        let bytes_per_pixel = match vinfo.bits_per_pixel {
            8 => 1,
            32 => 4,
            other => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unsupported framebuffer depth: {other} bpp (expected 8 or 32)"),
                ));
            }
        };

        if width == 0 || height == 0 || stride < width as usize * bytes_per_pixel {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "invalid framebuffer geometry: {width}x{height}, stride={stride}, bpp={}",
                    vinfo.bits_per_pixel
                ),
            ));
        }

        let len = stride * height as usize;

        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };

        if map == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }

        Ok(Self {
            file,
            map: map as *mut u8,
            len,
            width,
            height,
            stride,
            bytes_per_pixel,
            output_inverted: false,
            dithering: DitheringHandle::default(),
            bilevel: false,
            dithering_delivery_failure: Cell::new(None),
            update_variant: Cell::new(None),
            last_issued_marker: Cell::new(0),
            last_submitted_marker: Cell::new(None),
            last_completed_marker: Cell::new(None),
        })
    }

    /// Configure only the logical-gray-to-memory conversion, without writing pixels.
    pub(crate) fn set_output_inverted(&mut self, enabled: bool) {
        self.output_inverted = enabled;
    }

    /// Read the current logical-gray-to-memory compensation without native I/O.
    pub(crate) fn output_inverted(&self) -> bool {
        self.output_inverted
    }

    fn output_mask(&self) -> u8 {
        if self.output_inverted { 0xff } else { 0x00 }
    }

    /// Bind processing to the scene that will be written in this render.
    pub(crate) fn set_dithering(&mut self, handle: DitheringHandle, bilevel: bool) {
        if self.bytes_per_pixel != 1
            || matches!(self.update_variant.get(), Some(UpdateVariant::Legacy | UpdateVariant::Rex
                | UpdateVariant::Zelda | UpdateVariant::Unsupported))
        {
            handle.reject_hardware();
        }
        self.dithering = handle;
        self.bilevel = bilevel;
    }

    fn hardware_y4_active(&self) -> bool {
        !self.bilevel && self.bytes_per_pixel == 1 && self.dithering.hardware_y4_enabled()
    }

    pub(crate) fn software_dither_active(&self) -> bool {
        self.dithering.requested() && !self.dithering.hardware_y4_available() && !self.bilevel
    }

    pub(crate) fn require_known_dithering_delivery(&self) -> std::io::Result<()> {
        match self.dithering_delivery_failure.get() {
            None => Ok(()),
            Some(Some(errno)) => Err(std::io::Error::from_raw_os_error(errno)),
            Some(None) => Err(std::io::Error::other("the display driver returned an unknown update outcome")),
        }
    }

    fn block_uncertain_dithering_delivery(&self, result: &mut RefreshSubmission) {
        self.dithering_delivery_failure.set(Some(result.last_errno));
        result.dithering_delivery_uncertain = true;
    }

    /// Gray8 retained behind overlays cannot safely receive a binary waveform.
    /// Promote every request while such a scene is current, including fallback.
    pub(crate) fn resolve_dithering(&self, mut refresh: ResolvedRefresh) -> ResolvedRefresh {
        if self.dithering.requested() && !self.bilevel {
            refresh.waveform = RefreshWaveform::Gc16;
            if self.hardware_y4_active() {
                let region = aligned_dither_region(UpdateRect {
                    left: refresh.region.x as u32,
                    top: refresh.region.y as u32,
                    width: refresh.region.width,
                    height: refresh.region.height,
                }, self.width, self.height);
                refresh.region = crate::PhysicalRect { x: region.left as i32, y: region.top as i32,
                    width: region.width, height: region.height };
            }
        }
        refresh
    }

    /// Write logical grayscale pixels into the mmap at row `y`.
    pub(crate) fn write_line(&mut self, y: usize, x_range: Range<usize>, pixels: &[u8]) {
        if self.dithering_delivery_failure.get().is_some() { return; }
        let bpp = self.bytes_per_pixel;
        let output_mask = self.output_mask();
        let fallback = self.dithering.requested() && !self.dithering.hardware_y4_available() && !self.bilevel;
        let row_span = unsafe {
            std::slice::from_raw_parts_mut(
                self.map.add(y * self.stride + x_range.start * bpp),
                pixels.len() * bpp,
            )
        };
        match bpp {
            1 if output_mask == 0 && !fallback => row_span.copy_from_slice(pixels),
            1 => {
                for (offset, (dst, &value)) in row_span.iter_mut().zip(pixels).enumerate() {
                    let value = if fallback { software_gray(value, x_range.start + offset, y) } else { value };
                    *dst = value ^ output_mask;
                }
            }
            _ => {
                for (offset, (dst, &value)) in row_span.chunks_exact_mut(4).zip(pixels).enumerate() {
                    let value = if fallback { software_gray(value, x_range.start + offset, y) } else { value };
                    let g = value ^ output_mask;
                    dst.copy_from_slice(&[g, g, g, 0xff]);
                }
            }
        }
    }

    /// Fill the visible area with logical gray (0x00 = black, 0xff = white).
    pub(crate) fn fill(&mut self, value: u8) {
        if self.dithering_delivery_failure.get().is_some() { return; }
        let bpp = self.bytes_per_pixel;
        let output_mask = self.output_mask();
        let logical = value;
        let value = value ^ output_mask;
        let fallback = self.software_dither_active();
        for y in 0..self.height as usize {
            let row = unsafe {
                std::slice::from_raw_parts_mut(
                    self.map.add(y * self.stride),
                    self.width as usize * bpp,
                )
            };
            if fallback {
                for (x, pixel) in row.chunks_exact_mut(bpp).enumerate() {
                    let value = software_gray(logical, x, y) ^ output_mask;
                    pixel[0] = value;
                    if bpp == 4 { pixel.copy_from_slice(&[value, value, value, 0xff]); }
                }
            } else {
                match bpp {
                    1 => row.fill(value),
                    _ => {
                        for px in row.chunks_exact_mut(4) {
                            px.copy_from_slice(&[value, value, value, 0xff]);
                        }
                    }
                }
            }
        }
    }

    /// Ask the EPDC to refresh a region of the e-ink panel.
    ///
    /// On the first call we probe which update ioctl the kernel accepts and
    /// cache it, so later frames issue exactly one ioctl instead of retrying a
    /// known-failing one every refresh.
    fn send_update(&self, region: UpdateRect, waveform: u32, mode: u32) -> RefreshSubmission {
        self.send_update_using(region, waveform, mode,
            |variant, region, waveform, mode, marker, y4| {
                if self.send_update_variant(variant, region, waveform, mode, marker, y4) {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error().raw_os_error())
                }
            },
            || self.wait_for_pending_update_complete_checked())
    }

    fn send_update_using(
        &self,
        mut region: UpdateRect,
        mut waveform: u32,
        mode: u32,
        mut submit: impl FnMut(UpdateVariant, UpdateRect, u32, u32, u32, bool) -> Result<(), Option<i32>>,
        wait: impl FnOnce() -> std::io::Result<()>,
    ) -> RefreshSubmission {
        if let Some(errno) = self.dithering_delivery_failure.get() {
            return RefreshSubmission { last_errno: errno, dithering_delivery_uncertain: true,
                ..Default::default() };
        }
        if region.width == 0 || region.height == 0 {
            return RefreshSubmission::default();
        }
        let y4 = self.hardware_y4_active();
        let gray8_scene = self.dithering.requested() && !self.bilevel;
        if gray8_scene {
            waveform = WAVEFORM_MODE_GC16;
        }
        if y4 {
            region = aligned_dither_region(region, self.width, self.height);
            // Eligibility requires an identified Scribe, whose ABI is MTK.
            // Never probe another ABI with a raw image and a missing Y4 flag.
            self.update_variant.set(Some(UpdateVariant::Mtk));
        }
        let mut result = self.submit_update_with_policy(region.width, region.height, gray8_scene,
            |variant, marker| submit(variant, region, waveform, mode, marker,
                y4 && variant == UpdateVariant::Mtk));
        result.y4_attempted = y4 && result.ioctl_attempts != 0;
        if !result.accepted && result.y4_attempted
            && explicit_update_rejection(result.last_errno)
        {
            // These errors explicitly reject the operation. Other errors may
            // have uncertain delivery and must never trigger an automatic retry.
            self.dithering.reject_hardware();
            if let Err(error) = wait() {
                result.last_errno = error.raw_os_error();
                self.block_uncertain_dithering_delivery(&mut result);
                return result;
            }
            // Hardware processing may have been used for any retained pixel.
            // Settle the prior accepted update, then replace the complete
            // visible mapping before refreshing it without a hardware flag.
            self.software_dither_mapping();
            if result.last_errno == Some(libc::ENOTTY) {
                self.update_variant.set(None);
            }
            let full = UpdateRect { top: 0, left: 0, width: self.width, height: self.height };
            let retry = self.submit_update_with_policy(full.width, full.height, true,
                |variant, marker| submit(variant, full, WAVEFORM_MODE_GC16, UPDATE_MODE_FULL, marker, false));
            result.ioctl_attempts = result.ioctl_attempts.saturating_add(retry.ioctl_attempts);
            result.accepted = retry.accepted;
            result.last_errno = retry.last_errno;
            result.software_dither_fallback = true;
            if !retry.accepted && retry.ioctl_attempts != 0
                && !explicit_update_rejection(retry.last_errno)
            {
                self.block_uncertain_dithering_delivery(&mut result);
            }
        } else if gray8_scene && !result.accepted && result.ioctl_attempts != 0
            && !explicit_update_rejection(result.last_errno)
        {
            self.block_uncertain_dithering_delivery(&mut result);
        }
        result
    }

    fn software_dither_mapping(&self) {
        if self.dithering_delivery_failure.get().is_some() { return; }
        let mask = self.output_mask();
        for y in 0..self.height as usize {
            for x in 0..self.width as usize {
                let offset = y * self.stride + x * self.bytes_per_pixel;
                // SAFETY: Coordinates remain within the owned visible mapping.
                // This executes on the UI thread after the preceding update's
                // checked completion; no slice aliases these writes.
                unsafe {
                    let logical = *self.map.add(offset) ^ mask;
                    let value = software_gray(logical, x, y) ^ mask;
                    *self.map.add(offset) = value;
                    if self.bytes_per_pixel == 4 {
                        *self.map.add(offset + 1) = value;
                        *self.map.add(offset + 2) = value;
                    }
                }
            }
        }
    }

    #[cfg(test)]
    fn submit_update_with(
        &self,
        width: u32,
        height: u32,
        submit: impl FnMut(UpdateVariant, u32) -> Result<(), Option<i32>>,
    ) -> RefreshSubmission {
        self.submit_update_with_policy(width, height, false, submit)
    }

    fn submit_update_with_policy(
        &self,
        width: u32,
        height: u32,
        stop_on_uncertain: bool,
        mut submit: impl FnMut(UpdateVariant, u32) -> Result<(), Option<i32>>,
    ) -> RefreshSubmission {
        if let Some(errno) = self.dithering_delivery_failure.get() {
            return RefreshSubmission { last_errno: errno, dithering_delivery_uncertain: true,
                ..Default::default() };
        }
        let cached = self.update_variant.get();
        if width == 0 || height == 0 || cached == Some(UpdateVariant::Unsupported) {
            return RefreshSubmission::default();
        }
        let Some(marker) = self.last_issued_marker.get().checked_add(1) else {
            return RefreshSubmission { last_errno: Some(libc::EOVERFLOW), ..Default::default() };
        };
        // Consume the identity before probing: an uncertain failure must never
        // let a later update claim an older completion receipt.
        self.last_issued_marker.set(marker);
        let (selected, submission) = refresh_submission_with_policy(cached, width, height, stop_on_uncertain,
            |variant| submit(variant, marker));
        if cached.is_none() && submission.ioctl_attempts != 0 {
            if selected == Some(UpdateVariant::Unsupported) {
                log::error!(
                    "EPDC refresh failed: no known MXCFB_SEND_UPDATE variant \
                     was accepted; the screen will not update"
                );
            }
            self.update_variant.set(selected);
        }
        if submission.accepted {
            self.last_submitted_marker.set(Some(marker));
        }
        submission
    }

    /// Issue one specific update ioctl variant. Returns whether it succeeded.
    fn send_update_variant(
        &self,
        variant: UpdateVariant,
        region: UpdateRect,
        waveform: u32,
        mode: u32,
        marker: u32,
        y4: bool,
    ) -> bool {
        match variant {
            UpdateVariant::Legacy => self.send_update_legacy(region, waveform, mode, marker),
            UpdateVariant::Rex => self.send_update_rex(region, waveform, mode, marker),
            UpdateVariant::Zelda => self.send_update_zelda(region, waveform, mode, marker),
            UpdateVariant::Mtk => self.send_update_mtk(region, waveform, mode, marker, y4),
            UpdateVariant::Unsupported => false,
        }
    }

    /// Hand one of the `UpdateRequest*` structs to the kernel. Returns whether
    /// the ioctl succeeded.
    fn send_update_ioctl<T>(&self, request: libc::c_ulong, update: &T) -> bool {
        // SAFETY: `update` outlives the ioctl and matches the kernel's struct.
        unsafe { libc::ioctl(self.file.as_raw_fd(), request as _, update as *const _) != -1 }
    }

    /// Issue the legacy `MXCFB_SEND_UPDATE` (72-byte struct). Returns whether the
    /// ioctl succeeded.
    fn send_update_legacy(&self, region: UpdateRect, waveform: u32, mode: u32, marker: u32) -> bool {
        let update = UpdateRequest {
            update_region: region,
            waveform_mode: waveform,
            update_mode: mode,
            update_marker: marker,
            temperature: TEMP_USE_AMBIENT,
            flags: if waveform == WAVEFORM_MODE_DU {
                EPDC_FLAG_FORCE_MONOCHROME
            } else {
                0
            },
            ..Default::default()
        };
        self.send_update_ioctl(MXCFB_SEND_UPDATE, &update)
    }

    /// Issue the modern `MXCFB_SEND_UPDATE_REX` (80-byte struct). Returns whether
    /// the ioctl succeeded.
    fn send_update_rex(&self, region: UpdateRect, waveform: u32, mode: u32, marker: u32) -> bool {
        let update = UpdateRequestRex {
            update_region: region,
            waveform_mode: waveform,
            update_mode: mode,
            update_marker: marker,
            temperature: TEMP_USE_AMBIENT,
            flags: if waveform == WAVEFORM_MODE_DU {
                EPDC_FLAG_FORCE_MONOCHROME
            } else {
                0
            },
            ..Default::default()
        };
        self.send_update_ioctl(MXCFB_SEND_UPDATE_REX, &update)
    }

    /// Issue `MXCFB_SEND_UPDATE_MTK` (96-byte struct, MediaTek devices).
    /// Returns whether the ioctl succeeded.
    fn send_update_mtk(&self, region: UpdateRect, waveform: u32, mode: u32, marker: u32, y4: bool) -> bool {
        let update = mtk_update(region, waveform, mode, marker, y4);
        self.send_update_ioctl(MXCFB_SEND_UPDATE_MTK, &update)
    }

    /// Issue `MXCFB_SEND_UPDATE_ZELDA` (88-byte struct, KOA2/KOA3 — Oasis 2/3).
    /// Returns whether the ioctl succeeded.
    fn send_update_zelda(&self, region: UpdateRect, waveform: u32, mode: u32, marker: u32) -> bool {
        let update = UpdateRequestZelda {
            update_region: region,
            waveform_mode: waveform,
            update_mode: mode,
            update_marker: marker,
            temperature: TEMP_USE_AMBIENT,
            flags: if waveform == WAVEFORM_MODE_DU {
                EPDC_FLAG_FORCE_MONOCHROME
            } else {
                0
            },
            ..Default::default()
        };
        self.send_update_ioctl(MXCFB_SEND_UPDATE_ZELDA, &update)
    }

    /// Apply a request resolved against the current framebuffer bounds.
    pub(crate) fn refresh_planned(&self, refresh: ResolvedRefresh) -> RefreshSubmission {
        if refresh.waveform == RefreshWaveform::Auto && !refresh.flashing {
            return self.refresh_region(
                slint::PhysicalPosition::new(refresh.region.x, refresh.region.y),
                slint::PhysicalSize::new(refresh.region.width, refresh.region.height),
            );
        }
        let waveform = waveform_for_driver(refresh.waveform, self.update_variant.get());
        self.send_update(
            UpdateRect {
                top: refresh.region.y as u32,
                left: refresh.region.x as u32,
                width: refresh.region.width,
                height: refresh.region.height,
            },
            waveform,
            if refresh.flashing {
                UPDATE_MODE_FULL
            } else {
                UPDATE_MODE_PARTIAL
            },
        )
    }

    /// Full-screen GC16 refresh
    pub(crate) fn refresh_full(&self) -> RefreshSubmission {
        self.send_update(
            UpdateRect {
                top: 0,
                left: 0,
                width: self.width,
                height: self.height,
            },
            WAVEFORM_MODE_GC16,
            UPDATE_MODE_FULL,
        )
    }

    /// Require acceptance of this full-frame presentation update.
    #[cfg(test)]
    pub(crate) fn refresh_full_checked(&self) -> std::io::Result<RefreshSubmission> {
        let submission = self.refresh_full();
        if submission.accepted {
            Ok(submission)
        } else {
            Err(submission.last_errno.map(std::io::Error::from_raw_os_error)
                .unwrap_or_else(|| std::io::Error::other("the EPDC rejected the full-frame update")))
        }
    }

    /// Block until the EPDC has applied the latest successfully submitted update.
    ///
    /// Used before suspending to RAM so the panel doesn't latch mid-refresh.
    /// Best-effort: a failing ioctl is ignored, since this is purely defensive.
    pub(crate) fn wait_for_update_complete(&self) {
        let _ = self.wait_for_update_complete_checked();
    }

    /// Require completion evidence before acknowledging a sleep presentation.
    pub(crate) fn wait_for_update_complete_checked(&self) -> std::io::Result<()> {
        self.require_known_dithering_delivery()?;
        self.wait_for_update_complete_with(|marker| {
            // SAFETY: The live descriptor and record match the kernel wait ABI.
            if unsafe { libc::ioctl(self.file.as_raw_fd(),
                MXCFB_WAIT_FOR_UPDATE_COMPLETE as _, marker as *mut _) } < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        })
    }

    /// Finish preceding asynchronous reads before changing mapped pixels.
    pub(crate) fn wait_for_pending_update_complete_checked(&self) -> std::io::Result<()> {
        self.require_known_dithering_delivery()?;
        if self.last_submitted_marker.get().is_none() {
            return Ok(());
        }
        self.wait_for_update_complete_checked()
    }

    fn wait_for_update_complete_with(
        &self,
        wait: impl FnOnce(&mut UpdateMarkerData) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        self.require_known_dithering_delivery()?;
        let submitted = self.last_submitted_marker.get()
            .ok_or_else(|| std::io::Error::other("no EPDC update was successfully submitted"))?;
        if self.last_completed_marker.get() == Some(submitted) {
            return Ok(());
        }
        let mut marker = UpdateMarkerData { update_marker: submitted, collision_test: 0 };
        wait(&mut marker)?;
        self.last_completed_marker.set(Some(submitted));
        Ok(())
    }

    /// Partial refresh of a dirty rectangle
    pub(crate) fn refresh_region(
        &self,
        origin: slint::PhysicalPosition,
        size: slint::PhysicalSize,
    ) -> RefreshSubmission {
        if size.width == 0 || size.height == 0 {
            return RefreshSubmission::default();
        }
        self.send_update(
            UpdateRect {
                top: origin.y as u32,
                left: origin.x as u32,
                width: size.width,
                height: size.height,
            },
            WAVEFORM_MODE_AUTO,
            UPDATE_MODE_PARTIAL,
        )
    }
}

impl Drop for Framebuffer {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.map as *mut libc::c_void, self.len) };
    }
}

fn waveform_for_driver(waveform: RefreshWaveform, variant: Option<UpdateVariant>) -> u32 {
    match waveform {
        RefreshWaveform::Auto => WAVEFORM_MODE_AUTO,
        RefreshWaveform::Gc16 => WAVEFORM_MODE_GC16,
        RefreshWaveform::Du => WAVEFORM_MODE_DU,
        RefreshWaveform::A2 => match variant {
            Some(UpdateVariant::Legacy) => WAVEFORM_MODE_A2_LEGACY,
            Some(UpdateVariant::Rex | UpdateVariant::Zelda | UpdateVariant::Mtk) => {
                WAVEFORM_MODE_A2_MODERN
            }
            _ => WAVEFORM_MODE_AUTO,
        },
    }
}

fn aligned_dither_region(region: UpdateRect, width: u32, height: u32) -> UpdateRect {
    if region.width == 0 || region.height == 0 {
        return region;
    }
    let left = region.left.min(width);
    let top = region.top.min(height);
    if left == width || top == height {
        return UpdateRect { left, top, width: 0, height: 0 };
    }
    let right = (u64::from(region.left) + u64::from(region.width)).min(u64::from(width));
    let bottom = (u64::from(region.top) + u64::from(region.height)).min(u64::from(height));
    let left = left / 8 * 8;
    let top = top / 8 * 8;
    let right = right.div_ceil(8).saturating_mul(8).min(u64::from(width)) as u32;
    let bottom = bottom.div_ceil(8).saturating_mul(8).min(u64::from(height)) as u32;
    UpdateRect { left, top, width: right - left, height: bottom - top }
}

fn mtk_update(region: UpdateRect, waveform: u32, mode: u32, marker: u32, y4: bool) -> UpdateRequestMtk {
    let y4 = y4 && waveform == WAVEFORM_MODE_GC16;
    UpdateRequestMtk {
        update_region: region,
        waveform_mode: waveform,
        update_mode: mode,
        update_marker: marker,
        temperature: TEMP_USE_AMBIENT,
        flags: if y4 { MTK_EPDC_FLAG_USE_DITHERING_Y4 }
            else if waveform == WAVEFORM_MODE_DU { EPDC_FLAG_FORCE_MONOCHROME }
            else { 0 },
        // MTK chooses the MDP Y8->Y4 algorithm through flags. These EPDCv2
        // fields must remain zero; quant_bit=7 is an i.MX convention.
        dither_mode: 0,
        quant_bit: 0,
        hist_bw_waveform_mode: if y4 { WAVEFORM_MODE_DU } else { 0 },
        hist_gray_waveform_mode: if y4 { WAVEFORM_MODE_GC16 } else { 0 },
        ..Default::default()
    }
}

fn explicit_update_rejection(errno: Option<i32>) -> bool {
    matches!(errno, Some(libc::EINVAL | libc::ENOTTY | libc::EOPNOTSUPP))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waveform_constants_follow_the_detected_driver_family() {
        assert_eq!(
            waveform_for_driver(RefreshWaveform::A2, Some(UpdateVariant::Legacy)),
            4
        );
        for variant in [UpdateVariant::Rex, UpdateVariant::Zelda, UpdateVariant::Mtk] {
            assert_eq!(waveform_for_driver(RefreshWaveform::A2, Some(variant)), 6);
        }
        assert_eq!(
            waveform_for_driver(RefreshWaveform::A2, None),
            WAVEFORM_MODE_AUTO
        );
        assert_eq!(
            waveform_for_driver(RefreshWaveform::A2, Some(UpdateVariant::Unsupported)),
            WAVEFORM_MODE_AUTO
        );
        assert_eq!(
            waveform_for_driver(RefreshWaveform::Du, Some(UpdateVariant::Mtk)),
            1
        );
        assert_eq!(
            waveform_for_driver(RefreshWaveform::Gc16, Some(UpdateVariant::Mtk)),
            2
        );
    }
}
