//! Immutable worker-prepared RGBA pixels for constant-size UI attachment.
//!
//! Slint 1.17.1 exposes no ownership transfer from Vec to SharedPixelBuffer.
//! Conversion therefore happens before worker delivery, never during UI import.
//! The retained budget counts each live pixel allocation once; allocator and
//! SharedVector header bookkeeping are excluded, as with the previous Vec budget.

use anyhow::{Result, ensure};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

const COPY_BLOCK_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub(super) struct PreparedPixels {
    pixels: SharedPixelBuffer<Rgba8Pixel>,
}

impl PreparedPixels {
    /// Consume finalized RGBA without changing quantization, alpha, or Fast data.
    /// Allocation/default initialization is bounded by the existing viewport
    /// limit; copying checks cancellation inside unusually wide rows as well.
    pub(super) fn from_rgba_owned(
        rgba: Vec<u8>, width: u32, height: u32, mut cancelled: impl FnMut() -> bool,
    ) -> Result<Option<Self>> {
        if cancelled() { return Ok(None); }
        let count = u64::from(width).checked_mul(u64::from(height));
        ensure!(width > 0 && height > 0 && count.is_some_and(|count|
            count <= crate::reader::MAX_VIEWPORT_PIXELS), "Invalid prepared pixel dimensions");
        let length = usize::try_from(count.unwrap().checked_mul(4).unwrap())?;
        ensure!(rgba.len() == length, "Prepared RGBA length does not match its dimensions");
        let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
        if cancelled() { return Ok(None); }
        for (destination, source) in pixels.make_mut_bytes().chunks_mut(COPY_BLOCK_BYTES)
            .zip(rgba.chunks(COPY_BLOCK_BYTES)) {
            if cancelled() { return Ok(None); }
            destination.copy_from_slice(source);
        }
        if cancelled() { return Ok(None); }
        // The returned payload retains only the Slint allocation. In particular,
        // no ready frame or mailbox result can retain this original Vec too.
        drop(rgba);
        Ok(Some(Self { pixels }))
    }

    pub(super) fn width(&self) -> u32 { self.pixels.width() }
    pub(super) fn height(&self) -> u32 { self.pixels.height() }
    pub(super) fn byte_len(&self) -> usize { self.pixels.as_bytes().len() }
    pub(super) fn as_bytes(&self) -> &[u8] { self.pixels.as_bytes() }

    /// Call on the UI thread after all generation and geometry checks pass.
    pub(super) fn into_image(self) -> Image { Image::from_rgba8(self.pixels) }

    /// Attach another UI reference without making the payload mutable or copied.
    pub(super) fn image(&self) -> Image { Image::from_rgba8(self.pixels.clone()) }

    #[cfg(test)]
    pub(super) fn same_allocation(&self, other: &Self) -> bool {
        self.allocation_key() == other.allocation_key()
    }

    fn allocation_key(&self) -> (usize, usize) {
        (self.pixels.as_bytes().as_ptr() as usize, self.byte_len())
    }
}

/// A frame has at most three raster slots. Compare live allocation identities,
/// without pixel comparisons or an additional allocation on the UI thread.
pub(super) fn unique_pixel_bytes(pixels: &[&PreparedPixels]) -> u64 {
    pixels.iter().enumerate().fold(0u64, |total, (index, payload)| {
        let identity = payload.allocation_key();
        if pixels[..index].iter().any(|previous| previous.allocation_key() == identity) { total }
        else { total.saturating_add(payload.byte_len() as u64) }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepared(values: Vec<u8>, width: u32, height: u32) -> PreparedPixels {
        PreparedPixels::from_rgba_owned(values, width, height, || false).unwrap().unwrap()
    }

    #[test]
    fn shared_pixels_are_send_sync_and_preserve_every_rgba_component() {
        fn require_send_sync<T: Send + Sync>() {}
        require_send_sync::<PreparedPixels>();
        let values = vec![0, 17, 68, 127, 255, 136, 204, 255];
        let pixels = prepared(values.clone(), 2, 1);
        assert_eq!(pixels.as_bytes(), values);
        assert_eq!((pixels.width(), pixels.height(), pixels.byte_len()), (2, 1, 8));
    }

    #[test]
    fn ui_images_and_shared_clones_keep_the_worker_allocation() {
        let pixels = prepared(vec![17, 17, 17, 255, 255, 255, 255, 255], 2, 1);
        let address = pixels.as_bytes().as_ptr();
        let clone = pixels.clone();
        assert!(pixels.same_allocation(&clone));
        assert_eq!(pixels.image().to_rgba8().unwrap().as_bytes().as_ptr(), address);
        assert_eq!(clone.into_image().to_rgba8().unwrap().as_bytes().as_ptr(), address);
        assert_eq!(pixels.as_bytes(), [17, 17, 17, 255, 255, 255, 255, 255]);
    }

    #[test]
    fn malformed_payloads_and_pre_cancelled_work_are_never_published() {
        assert!(PreparedPixels::from_rgba_owned(vec![0; 7], 2, 1, || false).is_err());
        assert!(PreparedPixels::from_rgba_owned(Vec::new(), 0, 1, || false).is_err());
        assert!(PreparedPixels::from_rgba_owned(vec![0; 8], 2, 1, || true).unwrap().is_none());
    }

    #[test]
    fn payload_accounting_counts_live_clones_once_and_equal_distinct_buffers_twice() {
        let first = prepared(vec![17, 17, 17, 255], 1, 1);
        let clone = first.clone();
        let other = prepared(vec![17, 17, 17, 255], 1, 1);
        assert_eq!(unique_pixel_bytes(&[&first, &clone]), 4);
        assert_eq!(unique_pixel_bytes(&[&first, &clone, &other]), 8);
        assert_eq!(unique_pixel_bytes(&[]), 0);
        assert_eq!(first.as_bytes(), other.as_bytes());
        assert!(!first.same_allocation(&other));
    }

    #[test]
    fn a_wide_single_row_can_cancel_after_copying_a_block() {
        let width = 65_537;
        let mut checks = 0;
        let outcome = PreparedPixels::from_rgba_owned(vec![255; width * 4], width as u32, 1, || {
            checks += 1;
            checks >= 4
        }).unwrap();
        assert!(outcome.is_none());
        assert_eq!(checks, 4);
    }
}
