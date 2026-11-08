//! JPEG container boundaries and bounded decoding with the pinned strict decoder.
//!
//! Sequential images require one scan containing every frame component. A
//! single-component frame must use ordinary 1x1 sampling. Progressive images
//! retain their separate scan path; unsupported modes never supply repair proof.

use std::io::Cursor;

use image::{DynamicImage, RgbImage};
use zune_core::{colorspace::ColorSpace, options::DecoderOptions};
use zune_jpeg::{JpegDecoder, errors::DecodeErrors};

use super::{Fault, MAX_DECODE_ALLOCATION, MAX_ENCODED_BYTES, MAX_SOURCE_PIXELS};

type Result<T> = std::result::Result<T, Fault>;

const MAX_SEGMENTS: usize = 65_536;
const ROW_BUDGET_PER_SAMPLE: u64 = 512;
const FIXED_SCRATCH_BUDGET: u64 = 64 * 1024;

struct Component {
    id: u8,
    horizontal: u64,
    vertical: u64,
}

struct Frame {
    width: u32,
    height: u32,
    progressive: bool,
    components: Vec<Component>,
    first_scan_components: Option<usize>,
}

impl Frame {
    fn from_segment(payload: &[u8], progressive: bool) -> Result<Self> {
        if payload.len() < 6 { return Err(Fault::corrupt("jpeg_frame_truncated")); }
        if payload[0] != 8 { return Err(Fault::unsupported("jpeg_precision_unsupported")); }
        let height = u32::from(u16::from_be_bytes([payload[1], payload[2]]));
        let width = u32::from(u16::from_be_bytes([payload[3], payload[4]]));
        if height == 0 { return Err(Fault::unsupported("jpeg_dynamic_height_unsupported")); }
        if width == 0 { return Err(Fault::corrupt("jpeg_dimensions_invalid")); }
        if u64::from(width) * u64::from(height) > MAX_SOURCE_PIXELS {
            return Err(Fault::resource("image_pixel_limit"));
        }
        let count = usize::from(payload[5]);
        if count == 0 { return Err(Fault::corrupt("jpeg_components_invalid")); }
        if count > 4 { return Err(Fault::unsupported("jpeg_components_unsupported")); }
        if payload.len() != 6 + 3 * count { return Err(Fault::corrupt("jpeg_frame_length_invalid")); }
        let mut components: Vec<Component> = Vec::with_capacity(count);
        for definition in payload[6..].chunks_exact(3) {
            let horizontal = u64::from(definition[1] >> 4);
            let vertical = u64::from(definition[1] & 15);
            if !(1..=4).contains(&horizontal) || !(1..=4).contains(&vertical)
                || definition[2] > 3 || components.iter().any(|component| component.id == definition[0]) {
                return Err(Fault::corrupt("jpeg_component_invalid"));
            }
            components.push(Component { id: definition[0], horizontal, vertical });
        }
        if components.len() == 1 && (components[0].horizontal != 1 || components[0].vertical != 1) {
            return Err(Fault::unsupported("jpeg_single_component_sampling_unsupported"));
        }
        Ok(Self { width, height, progressive, components, first_scan_components: None })
    }

    fn check_scan_header(&mut self, payload: &[u8]) -> Result<()> {
        let count = payload.first().copied().map(usize::from)
            .ok_or_else(|| Fault::corrupt("jpeg_scan_truncated"))?;
        if count == 0 || count > self.components.len() || payload.len() != 4 + 2 * count {
            return Err(Fault::corrupt("jpeg_scan_length_invalid"));
        }
        if !self.progressive {
            if count != self.components.len() || self.first_scan_components.is_some() {
                // Do not enter the decoder's separated sequential scan paths:
                // their sampled-row and cross-scan restart accounting is not
                // established by this application validation version.
                return Err(Fault::unsupported("jpeg_separated_sequential_unsupported"));
            }
            if payload[1 + 2 * count..] != [0, 63, 0] {
                return Err(Fault::corrupt("jpeg_sequential_scan_parameters_invalid"));
            }
        }
        if self.first_scan_components.is_none() { self.first_scan_components = Some(count); }
        Ok(())
    }

    fn allocation_budget(&self, encoded_len: usize) -> Result<u64> {
        let horizontal = self.components.iter().map(|component| component.horizontal).max().unwrap();
        let vertical = self.components.iter().map(|component| component.vertical).max().unwrap();
        let columns = u64::from(self.width).div_ceil(8 * horizontal);
        let rows = u64::from(self.height).div_ceil(8 * vertical);
        let sampling = self.components.iter().map(|component| component.horizontal * component.vertical).sum::<u64>();
        // Header component order must not make the preflight underestimate an early stride.
        let row_columns = u64::from(self.width).div_ceil(8);
        let stride = self.components.iter().map(|component| row_columns * component.horizontal * 8).sum::<u64>();
        let maximum_stride = row_columns * horizontal * 8;
        let output = u64::from(self.width) * u64::from(self.height) * 3;
        let buffered = self.progressive || self.first_scan_components != Some(self.components.len());
        // The pinned decoder buffers i16 DCT planes for progressive and separated scans.
        let coefficients = if buffered { checked_product(&[128, columns, rows, sampling])? } else { 0 };
        // This conservatively covers row coefficients, upsampling buffers, and scratch.
        let row_buffers = checked_product(&[stride, ROW_BUDGET_PER_SAMPLE])?;
        let scratch = checked_product(&[maximum_stride, 64])?;
        // APP metadata may be retained and reassembled while the borrowed input remains external.
        let metadata = (encoded_len as u64).checked_mul(2)
            .ok_or_else(|| Fault::resource("jpeg_decode_allocation_limit"))?;
        [output, coefficients, row_buffers, scratch, metadata, FIXED_SCRATCH_BUDGET]
            .into_iter().try_fold(0u64, |sum, value| sum.checked_add(value)
                .ok_or_else(|| Fault::resource("jpeg_decode_allocation_limit")))
    }
}

fn checked_product(values: &[u64]) -> Result<u64> {
    values.iter().try_fold(1u64, |product, value| product.checked_mul(*value)
        .ok_or_else(|| Fault::resource("jpeg_decode_allocation_limit")))
}

fn marker(bytes: &[u8], position: &mut usize) -> Result<u8> {
    if bytes.get(*position) != Some(&0xff) { return Err(Fault::corrupt("jpeg_marker_missing")); }
    while bytes.get(*position) == Some(&0xff) { *position += 1; }
    let value = bytes.get(*position).copied().ok_or_else(|| Fault::corrupt("jpeg_marker_truncated"))?;
    *position += 1;
    if value == 0 { return Err(Fault::corrupt("jpeg_marker_invalid")); }
    Ok(value)
}

fn segment<'a>(bytes: &'a [u8], position: &mut usize) -> Result<&'a [u8]> {
    let start = (*position).checked_add(2).filter(|end| *end <= bytes.len())
        .ok_or_else(|| Fault::corrupt("jpeg_segment_truncated"))?;
    let length = usize::from(u16::from_be_bytes([bytes[*position], bytes[*position + 1]]));
    if length < 2 { return Err(Fault::corrupt("jpeg_segment_length_invalid")); }
    let end = (*position).checked_add(length).filter(|end| *end <= bytes.len())
        .ok_or_else(|| Fault::corrupt("jpeg_segment_truncated"))?;
    let payload = &bytes[start..end];
    *position = end;
    Ok(payload)
}

fn entropy_boundary(bytes: &[u8], position: &mut usize) -> Result<()> {
    while *position < bytes.len() {
        if bytes[*position] != 0xff { *position += 1; continue; }
        let start = *position;
        while bytes.get(*position) == Some(&0xff) { *position += 1; }
        let value = bytes.get(*position).copied().ok_or_else(|| Fault::corrupt("jpeg_entropy_truncated"))?;
        if value == 0 {
            if *position != start + 1 { return Err(Fault::corrupt("jpeg_entropy_stuffing_invalid")); }
            *position += 1;
        } else if (0xd0..=0xd7).contains(&value) {
            *position += 1;
        } else {
            *position = start;
            return Ok(());
        }
    }
    Err(Fault::corrupt("jpeg_end_missing"))
}

fn container(bytes: &[u8]) -> Result<Frame> {
    if !bytes.starts_with(&[0xff, 0xd8]) { return Err(Fault::corrupt("jpeg_start_missing")); }
    let mut position = 2;
    let mut frame: Option<Frame> = None;
    for _ in 0..MAX_SEGMENTS {
        match marker(bytes, &mut position)? {
            0xd9 => {
                if position != bytes.len() { return Err(Fault::corrupt("jpeg_trailing_data")); }
                let frame = frame.ok_or_else(|| Fault::corrupt("jpeg_frame_missing"))?;
                if frame.first_scan_components.is_none() { return Err(Fault::corrupt("jpeg_scan_missing")); }
                return Ok(frame);
            }
            coding @ (0xc0..=0xc2) => {
                if frame.is_some() { return Err(Fault::unsupported("jpeg_multiple_frames_unsupported")); }
                frame = Some(Frame::from_segment(segment(bytes, &mut position)?, coding == 0xc2)?);
            }
            0xda => {
                let payload = segment(bytes, &mut position)?;
                frame.as_mut().ok_or_else(|| Fault::corrupt("jpeg_scan_without_frame"))?.check_scan_header(payload)?;
                entropy_boundary(bytes, &mut position)?;
            }
            0xc4 | 0xdb | 0xdd | 0xe0..=0xef | 0xfe => { segment(bytes, &mut position)?; }
            0xd0..=0xd8 => return Err(Fault::corrupt("jpeg_unexpected_standalone_marker")),
            _ => return Err(Fault::unsupported("jpeg_coding_unsupported")),
        }
    }
    Err(Fault::resource("jpeg_segment_limit"))
}

fn known_entropy_format_error(message: &str) -> bool {
    // The pinned decode_huff! macro reports these two malformed-entropy
    // branches as Format rather than HuffmanDecode. Match its complete
    // generated message structure without promoting other format failures.
    if let Some(code) = message.strip_prefix("Bad Huffman Code 0x")
        .and_then(|code| code.strip_suffix(", corrupt JPEG"))
    {
        return !code.is_empty() && code.bytes().all(|byte| byte.is_ascii_hexdigit());
    }
    if let Some(comparison) = message.strip_prefix("Code length ") {
        if let Some((length, available)) = comparison.split_once("  more than bits left ") {
            return !length.is_empty() && !available.is_empty()
                && length.bytes().all(|byte| byte.is_ascii_digit())
                && available.bytes().all(|byte| byte.is_ascii_digit());
        }
    }
    false
}

fn decoder_fault(error: DecodeErrors) -> Fault {
    #[cfg(test)]
    eprintln!("JPEG decoder rejection before integrity classification: {error:?}");
    match error {
        DecodeErrors::LargeDimensions(_) => Fault::resource("jpeg_decoder_dimension_limit"),
        DecodeErrors::Unsupported(_) | DecodeErrors::TooSmallOutput(_, _) => Fault::unsupported("jpeg_decoder_unsupported"),
        DecodeErrors::Format(message) => {
            if message.starts_with("Too many scans") || message.contains("exceeded limit") {
                Fault::resource("jpeg_decoder_scan_limit")
            } else if message.starts_with("[strict-mode]:") || known_entropy_format_error(&message) {
                Fault::corrupt("jpeg_entropy_invalid")
            } else {
                Fault::unsupported("jpeg_decoder_rejected")
            }
        }
        DecodeErrors::FormatStatic(message) => {
            if message.starts_with("Too many markers") || message.contains("exceeded limit") {
                Fault::resource("jpeg_decoder_scan_limit")
            } else if message.starts_with("[strict-mode]:") || message == "Premature end of buffer" {
                Fault::corrupt("jpeg_entropy_invalid")
            } else {
                Fault::unsupported("jpeg_decoder_rejected")
            }
        }
        DecodeErrors::ExhaustedData | DecodeErrors::IllegalMagicBytes(_) | DecodeErrors::ZeroError
        | DecodeErrors::HuffmanDecode(_) | DecodeErrors::DqtError(_) | DecodeErrors::SosError(_)
        | DecodeErrors::SofError(_) | DecodeErrors::MCUError(_) => Fault::corrupt("jpeg_encoding_invalid"),
        DecodeErrors::IoErrors(_) => Fault::unsupported("jpeg_decoder_io_error"),
    }
}

/// Decode all pixels with strict entropy consumption while retaining progressive support.
pub(super) fn decode_jpeg(bytes: &[u8]) -> Result<DynamicImage> {
    Ok(decode_jpeg_cancellable(bytes, &|| false)?
        .expect("an uncancelled JPEG decode must not return cancellation"))
}

pub(super) fn decode_jpeg_cancellable(bytes: &[u8], is_cancelled: &dyn Fn() -> bool)
    -> Result<Option<DynamicImage>>
{
    if is_cancelled() { return Ok(None); }
    if bytes.len() as u64 > MAX_ENCODED_BYTES { return Err(Fault::resource("image_size_limit")); }
    let frame = container(bytes)?;
    if frame.allocation_budget(bytes.len())? > MAX_DECODE_ALLOCATION {
        return Err(Fault::resource("jpeg_decode_allocation_limit"));
    }
    if is_cancelled() { return Ok(None); }
    let options = DecoderOptions::default().set_strict_mode(true)
        .set_max_width(u16::MAX as usize).set_max_height(u16::MAX as usize)
        .jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut input = Cursor::new(bytes);
    let mut decoder = JpegDecoder::new_with_options(&mut input, options);
    decoder.decode_headers().map_err(decoder_fault)?;
    if decoder.dimensions() != Some((frame.width as usize, frame.height as usize)) {
        return Err(Fault::corrupt("jpeg_geometry_changed"));
    }
    let length = decoder.output_buffer_size().ok_or_else(|| Fault::resource("jpeg_decode_allocation_limit"))?;
    if length as u64 != u64::from(frame.width) * u64::from(frame.height) * 3
        || length as u64 > MAX_DECODE_ALLOCATION {
        return Err(Fault::resource("jpeg_decode_allocation_limit"));
    }
    if is_cancelled() { return Ok(None); }
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(length).map_err(|_| Fault::resource("jpeg_decode_allocation_limit"))?;
    pixels.resize(length, 0);
    decoder.decode_into(&mut pixels).map_err(decoder_fault)?;
    if is_cancelled() { return Ok(None); }
    if decoder.output_colorspace() != Some(ColorSpace::RGB) {
        return Err(Fault::unsupported("jpeg_output_colorspace_unsupported"));
    }
    drop(decoder);
    if input.position() != bytes.len() as u64 { return Err(Fault::corrupt("jpeg_unconsumed_data")); }
    let image = RgbImage::from_raw(frame.width, frame.height, pixels)
        .ok_or_else(|| Fault::unsupported("jpeg_decoder_output_invalid"))?;
    Ok(Some(DynamicImage::ImageRgb8(image)))
}
