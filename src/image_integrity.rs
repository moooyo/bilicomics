//! Bounded complete-image validation shared by acquisition, persistence, and reading.
//!
//! Proofs describe the exact encoded bytes, not a remote service's content identity.
//! Only successful validation is memoized; every call hashes its current input again.

use std::{
    collections::VecDeque,
    io::{Cursor, Read},
    ops::Deref,
    sync::{Mutex, OnceLock},
};

use flate2::{Decompress, FlushDecompress, Status};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use sha2::{Digest, Sha256};

mod jpeg;

pub(crate) const MAX_ENCODED_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const MAX_SOURCE_PIXELS: u64 = 32_000_000;
pub(crate) const MAX_DECODE_ALLOCATION: u64 = 128 * 1024 * 1024;
pub(crate) const VALIDATION_VERSION: u32 = 1;

const MEMO_CAPACITY: usize = 128;
const MAX_CONTAINER_CHUNKS: usize = 65_536;
const STREAM_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IntegrityFailureKind {
    Corrupt,
    ResourceLimit,
    Unsupported,
}

/// A failed encoding check tied to bytes that were actually examined.
/// Its constructor and fields are private so storage cannot invent a repair proof.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InvalidEncodingEvidence {
    sha256: String,
    byte_len: u64,
}

impl InvalidEncodingEvidence {
    pub(crate) fn sha256(&self) -> &str { &self.sha256 }
    pub(crate) fn byte_len(&self) -> u64 { self.byte_len }
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[error("{code}")]
pub(crate) struct IntegrityError {
    pub(crate) kind: IntegrityFailureKind,
    pub(crate) code: &'static str,
    evidence: Option<InvalidEncodingEvidence>,
}

impl IntegrityError {
    pub(crate) fn encoding_evidence(&self) -> Option<&InvalidEncodingEvidence> { self.evidence.as_ref() }
}

/// Read-only metadata; callers cannot turn it back into a validation proof.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImageMetadata {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) format: ImageFormat,
    pub(crate) extension: &'static str,
    pub(crate) sha256: String,
    pub(crate) byte_len: u64,
    pub(crate) validation_version: u32,
}

/// A sealed success proof. Deref permits field reads without permitting mutation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ValidatedImage(ImageMetadata);

impl Deref for ValidatedImage {
    type Target = ImageMetadata;
    fn deref(&self) -> &Self::Target { &self.0 }
}

impl ValidatedImage {
    pub(crate) fn sha256(&self) -> &str { &self.0.sha256 }
    pub(crate) fn byte_len(&self) -> u64 { self.0.byte_len }
    pub(crate) fn width(&self) -> u32 { self.0.width }
    pub(crate) fn height(&self) -> u32 { self.0.height }
    pub(crate) fn validation_version(&self) -> u32 { self.0.validation_version }
    pub(crate) fn into_metadata(self) -> ImageMetadata { self.0 }
}

pub(crate) type DecodedImageProof = ValidatedImage;
pub(crate) type InvalidEncodingProof = InvalidEncodingEvidence;

#[derive(Clone, Copy, Debug)]
pub(super) struct Fault {
    kind: IntegrityFailureKind,
    code: &'static str,
}

impl Fault {
    pub(super) fn corrupt(code: &'static str) -> Self { Self { kind: IntegrityFailureKind::Corrupt, code } }
    pub(super) fn resource(code: &'static str) -> Self { Self { kind: IntegrityFailureKind::ResourceLimit, code } }
    pub(super) fn unsupported(code: &'static str) -> Self { Self { kind: IntegrityFailureKind::Unsupported, code } }

    fn with_evidence(self, sha256: &str, byte_len: u64) -> IntegrityError {
        IntegrityError {
            kind: self.kind, code: self.code,
            evidence: (self.kind == IntegrityFailureKind::Corrupt).then(|| InvalidEncodingEvidence {
                sha256: sha256.to_owned(), byte_len,
            }),
        }
    }
}

#[derive(Default)]
struct ValidationMemo { entries: VecDeque<ValidatedImage> }

impl ValidationMemo {
    fn get(&mut self, digest: &str) -> Option<ValidatedImage> {
        let position = self.entries.iter().position(|entry| entry.sha256() == digest)?;
        let entry = self.entries.remove(position)?;
        self.entries.push_back(entry.clone());
        Some(entry)
    }

    fn insert(&mut self, image: ValidatedImage) {
        if let Some(position) = self.entries.iter().position(|entry| entry.sha256() == image.sha256()) {
            self.entries.remove(position);
        }
        self.entries.push_back(image);
        while self.entries.len() > MEMO_CAPACITY { self.entries.pop_front(); }
    }
}

fn memo() -> &'static Mutex<ValidationMemo> {
    static MEMO: OnceLock<Mutex<ValidationMemo>> = OnceLock::new();
    MEMO.get_or_init(|| Mutex::new(ValidationMemo::default()))
}

#[cfg(test)]
thread_local! { static DECODE_ATTEMPTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

fn record_decode_attempt() {
    #[cfg(test)]
    DECODE_ATTEMPTS.with(|attempts| attempts.set(attempts.get().saturating_add(1)));
}

#[cfg(test)]
fn decode_attempt_count() -> usize { DECODE_ATTEMPTS.with(std::cell::Cell::get) }

#[cfg(test)]
fn memo_len() -> usize { memo().lock().unwrap_or_else(|error| error.into_inner()).entries.len() }

#[cfg(test)]
fn memo_capacity() -> usize { MEMO_CAPACITY }

/// Validate the complete static image, without retaining its decoded pixel buffer.
#[cfg(test)]
pub(crate) fn validate_image(encoded: &[u8], max_pixels: Option<u64>) -> Result<ValidatedImage, IntegrityError> {
    Ok(validate_image_cancellable(encoded, max_pixels, &|| false)?
        .expect("an uncancelled validation must not return cancellation"))
}

/// Cancellation never supplies invalid-encoding evidence or a cached failure.
pub(crate) fn validate_image_cancellable(encoded: &[u8], max_pixels: Option<u64>,
    is_cancelled: &dyn Fn() -> bool) -> Result<Option<ValidatedImage>, IntegrityError>
{
    if is_cancelled() { return Ok(None); }
    let identity = input_identity(encoded);
    if is_cancelled() { return Ok(None); }
    let (digest, byte_len) = identity?;
    if let Some(proof) = memo().lock().unwrap_or_else(|error| error.into_inner()).get(&digest) {
        check_policy(proof.width, proof.height, max_pixels).map_err(|error| error.with_evidence(&digest, byte_len))?;
        return Ok(Some(proof));
    }
    let result = decode_fresh_cancellable(encoded, max_pixels, digest, byte_len, is_cancelled);
    if is_cancelled() { return Ok(None); }
    let Some((proof, decoded)) = result? else { return Ok(None); };
    drop(decoded);
    memo().lock().unwrap_or_else(|error| error.into_inner()).insert(proof.clone());
    Ok(Some(proof))
}

/// Decode pixels using the same acceptance rules as download validation.
#[cfg(test)]
pub(crate) fn decode_image(encoded: &[u8], max_pixels: Option<u64>)
    -> Result<(ValidatedImage, DynamicImage), IntegrityError>
{
    let (digest, byte_len) = input_identity(encoded)?;
    let previous = memo().lock().unwrap_or_else(|error| error.into_inner()).get(&digest);
    if let Some(proof) = previous {
        check_policy(proof.width, proof.height, max_pixels).map_err(|error| error.with_evidence(&digest, byte_len))?;
        record_decode_attempt();
        let decoded = decode_pixels(encoded, proof.format).map_err(|error| error.with_evidence(&digest, byte_len))?;
        if decoded.width() != proof.width || decoded.height() != proof.height {
            return Err(Fault::corrupt("image_geometry_changed").with_evidence(&digest, byte_len));
        }
        return Ok((proof, decoded));
    }
    let (proof, decoded) = decode_fresh(encoded, max_pixels, digest, byte_len)?;
    memo().lock().unwrap_or_else(|error| error.into_inner()).insert(proof.clone());
    Ok((proof, decoded))
}

pub(crate) enum ReusableImage<T> {
    Reused(T),
    Decoded(DynamicImage),
}

/// Reuse immutable pixels only after hashing the current encoded bytes once.
/// The callback must return pixels derived from its sealed validation proof.
pub(crate) fn decode_image_with_reuse<T>(encoded: &[u8], max_pixels: Option<u64>,
    find: impl FnOnce(&str) -> Option<(ValidatedImage, T)>)
    -> Result<(ValidatedImage, ReusableImage<T>), IntegrityError>
{
    let (digest, byte_len) = input_identity(encoded)?;
    if let Some((proof, pixels)) = find(&digest) {
        if proof.sha256() == digest && proof.byte_len() == byte_len
            && proof.validation_version() == VALIDATION_VERSION {
            check_policy(proof.width(), proof.height(), max_pixels)
                .map_err(|error| error.with_evidence(&digest, byte_len))?;
            memo().lock().unwrap_or_else(|error| error.into_inner()).insert(proof.clone());
            return Ok((proof, ReusableImage::Reused(pixels)));
        }
    }
    let previous = memo().lock().unwrap_or_else(|error| error.into_inner()).get(&digest);
    if let Some(proof) = previous {
        check_policy(proof.width, proof.height, max_pixels).map_err(|error| error.with_evidence(&digest, byte_len))?;
        record_decode_attempt();
        let decoded = decode_pixels(encoded, proof.format).map_err(|error| error.with_evidence(&digest, byte_len))?;
        if decoded.width() != proof.width || decoded.height() != proof.height {
            return Err(Fault::corrupt("image_geometry_changed").with_evidence(&digest, byte_len));
        }
        return Ok((proof, ReusableImage::Decoded(decoded)));
    }
    let (proof, decoded) = decode_fresh(encoded, max_pixels, digest, byte_len)?;
    memo().lock().unwrap_or_else(|error| error.into_inner()).insert(proof.clone());
    Ok((proof, ReusableImage::Decoded(decoded)))
}

fn input_identity(encoded: &[u8]) -> Result<(String, u64), IntegrityError> {
    let byte_len = encoded.len() as u64;
    if byte_len > MAX_ENCODED_BYTES {
        return Err(IntegrityError { kind: IntegrityFailureKind::ResourceLimit, code: "image_size_limit", evidence: None });
    }
    let digest = format!("{:x}", Sha256::digest(encoded));
    if encoded.is_empty() {
        return Err(Fault::corrupt("image_empty").with_evidence(&digest, byte_len));
    }
    Ok((digest, byte_len))
}

fn decode_fresh(encoded: &[u8], max_pixels: Option<u64>, digest: String, byte_len: u64)
    -> Result<(ValidatedImage, DynamicImage), IntegrityError>
{
    Ok(decode_fresh_cancellable(encoded, max_pixels, digest, byte_len, &|| false)?
        .expect("an uncancelled decode must not return cancellation"))
}

fn decode_fresh_cancellable(encoded: &[u8], max_pixels: Option<u64>, digest: String, byte_len: u64,
    is_cancelled: &dyn Fn() -> bool) -> Result<Option<(ValidatedImage, DynamicImage)>, IntegrityError>
{
    let result = (|| {
        if is_cancelled() { return Ok(None); }
        record_decode_attempt();
        let format = image::guess_format(encoded).map_err(|_| Fault::corrupt("image_format_invalid"))?;
        let extension = match format {
            ImageFormat::Png => "png", ImageFormat::Jpeg => "jpg", ImageFormat::WebP => "webp", ImageFormat::Gif => "gif",
            _ => return Err(Fault::unsupported("image_format_unsupported")),
        };
        let decoded = if format == ImageFormat::Jpeg {
            // Classify supported JPEG coding modes before the image wrapper's
            // header decoder can turn a capability error into corruption.
            let Some(decoded) = jpeg::decode_jpeg_cancellable(encoded, is_cancelled)? else { return Ok(None); };
            decoded
        } else {
            if is_cancelled() { return Ok(None); }
            let mut reader = ImageReader::with_format(Cursor::new(encoded), format);
            reader.limits(decode_limits());
            let decoder = reader.into_decoder().map_err(decoder_fault)?;
            let (width, height) = decoder.dimensions();
            check_policy(width, height, max_pixels)?;
            if decoder.total_bytes() > MAX_DECODE_ALLOCATION {
                return Err(Fault::resource("image_decode_allocation_limit"));
            }
            drop(decoder);
            if is_cancelled() { return Ok(None); }
            match format {
                ImageFormat::Png => validate_png(encoded, width, height)?,
                ImageFormat::WebP => validate_webp(encoded)?,
                ImageFormat::Gif => validate_gif(encoded)?,
                _ => unreachable!(),
            }
            if is_cancelled() { return Ok(None); }
            let decoded = if format == ImageFormat::Gif {
                let Some(decoded) = decode_gif_cancellable(encoded, is_cancelled)? else { return Ok(None); };
                decoded
            } else { decode_pixels(encoded, format)? };
            if decoded.width() != width || decoded.height() != height {
                return Err(Fault::corrupt("image_geometry_changed"));
            }
            decoded
        };
        if is_cancelled() { return Ok(None); }
        let (width, height) = (decoded.width(), decoded.height());
        check_policy(width, height, max_pixels)?;
        let proof = ValidatedImage(ImageMetadata {
            width, height, format, extension, sha256: digest.clone(), byte_len,
            validation_version: VALIDATION_VERSION,
        });
        Ok(Some((proof, decoded)))
    })();
    result.map_err(|error: Fault| error.with_evidence(&digest, byte_len))
}

fn check_policy(width: u32, height: u32, max_pixels: Option<u64>) -> Result<(), Fault> {
    if width == 0 || height == 0 { return Err(Fault::corrupt("image_dimensions_invalid")); }
    let maximum = max_pixels.unwrap_or(MAX_SOURCE_PIXELS).min(MAX_SOURCE_PIXELS);
    if u64::from(width) * u64::from(height) > maximum {
        return Err(Fault::resource("image_pixel_limit"));
    }
    Ok(())
}

fn decode_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = None;
    limits.max_image_height = None;
    limits.max_alloc = Some(MAX_DECODE_ALLOCATION);
    limits
}

fn decode_pixels(encoded: &[u8], format: ImageFormat) -> Result<DynamicImage, Fault> {
    if format == ImageFormat::Jpeg { return jpeg::decode_jpeg(encoded); }
    if format == ImageFormat::Gif { return decode_gif(encoded); }
    let mut reader = ImageReader::with_format(Cursor::new(encoded), format);
    reader.limits(decode_limits());
    reader.decode().map_err(decoder_fault)
}

fn decoder_fault(error: image::ImageError) -> Fault {
    match error {
        image::ImageError::Limits(_) => Fault::resource("image_decode_allocation_limit"),
        image::ImageError::Unsupported(_) => Fault::unsupported("image_decode_unsupported"),
        _ => Fault::corrupt("image_decode_invalid"),
    }
}

fn validate_png(encoded: &[u8], width: u32, height: u32) -> Result<(), Fault> {
    if !encoded.starts_with(b"\x89PNG\r\n\x1a\n") { return Err(Fault::corrupt("png_signature_invalid")); }
    let mut position = 8usize;
    let mut chunks = 0usize;
    let mut idat = Vec::new();
    let mut idat_bytes = 0u64;
    let mut saw_idat = false;
    let mut ended_idat = false;
    let mut expected = None;
    while position < encoded.len() {
        chunks += 1;
        if chunks > MAX_CONTAINER_CHUNKS { return Err(Fault::resource("image_chunk_limit")); }
        let header = encoded.get(position..position + 8).ok_or_else(|| Fault::corrupt("png_container_truncated"))?;
        let length = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        let end = position.checked_add(length).and_then(|value| value.checked_add(12))
            .filter(|value| *value <= encoded.len()).ok_or_else(|| Fault::corrupt("png_container_truncated"))?;
        let kind = &header[4..8];
        let data = &encoded[position + 8..end - 4];
        if !kind.iter().all(u8::is_ascii_alphabetic) || !kind[2].is_ascii_uppercase() {
            return Err(Fault::corrupt("png_chunk_invalid"));
        }
        let expected_crc = u32::from_be_bytes(encoded[end - 4..end].try_into().unwrap());
        if crc32fast::hash(&encoded[position + 4..end - 4]) != expected_crc {
            return Err(Fault::corrupt("png_crc_invalid"));
        }
        if chunks == 1 {
            if kind != b"IHDR" || length != 13 { return Err(Fault::corrupt("png_header_invalid")); }
            let declared_width = u32::from_be_bytes(data[..4].try_into().unwrap());
            let declared_height = u32::from_be_bytes(data[4..8].try_into().unwrap());
            if declared_width != width || declared_height != height { return Err(Fault::corrupt("png_header_invalid")); }
            expected = Some(png_filtered_bytes(data, width, height)?);
        } else if kind == b"IHDR" {
            return Err(Fault::corrupt("png_header_invalid"));
        }
        if matches!(kind, b"acTL" | b"fcTL" | b"fdAT") {
            return Err(Fault::unsupported("animated_image_unsupported"));
        }
        if kind == b"IDAT" {
            if ended_idat { return Err(Fault::corrupt("png_idat_order_invalid")); }
            saw_idat = true;
            idat_bytes = idat_bytes.checked_add(data.len() as u64).ok_or_else(|| Fault::resource("image_size_limit"))?;
            idat.push(data);
        } else if saw_idat {
            ended_idat = true;
        }
        if kind == b"IEND" {
            if length != 0 || !saw_idat || end != encoded.len() { return Err(Fault::corrupt("png_end_invalid")); }
            return validate_png_zlib(idat, idat_bytes, expected.ok_or_else(|| Fault::corrupt("png_header_invalid"))?);
        }
        position = end;
    }
    Err(Fault::corrupt("png_end_missing"))
}

fn png_filtered_bytes(header: &[u8], width: u32, height: u32) -> Result<u64, Fault> {
    let depth = u64::from(header[8]);
    let channels = match header[9] {
        0 if matches!(depth, 1 | 2 | 4 | 8 | 16) => 1u64,
        2 if matches!(depth, 8 | 16) => 3,
        3 if matches!(depth, 1 | 2 | 4 | 8) => 1,
        4 if matches!(depth, 8 | 16) => 2,
        6 if matches!(depth, 8 | 16) => 4,
        _ => return Err(Fault::corrupt("png_header_invalid")),
    };
    if header[10] != 0 || header[11] != 0 || header[12] > 1 { return Err(Fault::corrupt("png_header_invalid")); }
    let passes: &[(u64, u64, u64, u64)] = if header[12] == 0 { &[(0, 0, 1, 1)] }
        else { &[(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)] };
    let mut expected = 0u64;
    for &(x, y, dx, dy) in passes {
        if u64::from(width) <= x || u64::from(height) <= y { continue; }
        let columns = (u64::from(width) - x).div_ceil(dx);
        let rows = (u64::from(height) - y).div_ceil(dy);
        let row_bytes = columns.checked_mul(channels).and_then(|value| value.checked_mul(depth))
            .map(|value| value.div_ceil(8)).ok_or_else(|| Fault::resource("image_decode_allocation_limit"))?;
        expected = row_bytes.checked_add(1).and_then(|value| value.checked_mul(rows))
            .and_then(|value| value.checked_add(expected)).ok_or_else(|| Fault::resource("image_decode_allocation_limit"))?;
    }
    // Filtering adds at most one byte per source pixel across the Adam7 passes.
    if expected > MAX_DECODE_ALLOCATION + MAX_SOURCE_PIXELS { return Err(Fault::resource("image_decode_allocation_limit")); }
    Ok(expected)
}

struct IdatReader<'a> { chunks: Vec<&'a [u8]>, chunk: usize, position: usize }

impl Read for IdatReader<'_> {
    fn read(&mut self, destination: &mut [u8]) -> std::io::Result<usize> {
        let mut written = 0;
        while written < destination.len() && self.chunk < self.chunks.len() {
            let remaining = &self.chunks[self.chunk][self.position..];
            if remaining.is_empty() { self.chunk += 1; self.position = 0; continue; }
            let count = remaining.len().min(destination.len() - written);
            destination[written..written + count].copy_from_slice(&remaining[..count]);
            self.position += count;
            written += count;
        }
        Ok(written)
    }
}

fn validate_png_zlib(chunks: Vec<&[u8]>, byte_len: u64, expected: u64) -> Result<(), Fault> {
    let mut reader = IdatReader { chunks, chunk: 0, position: 0 };
    let mut input = [0u8; STREAM_BUFFER_BYTES];
    let mut output = [0u8; STREAM_BUFFER_BYTES];
    let mut start = 0;
    let mut end = 0;
    let mut eof = false;
    let mut decoder = Decompress::new(true);
    loop {
        if start == end && !eof {
            start = 0;
            end = reader.read(&mut input).map_err(|_| Fault::corrupt("png_deflate_invalid"))?;
            eof = end == 0;
        }
        let before_in = decoder.total_in();
        let before_out = decoder.total_out();
        let status = decoder.decompress(&input[start..end], &mut output,
            if eof { FlushDecompress::Finish } else { FlushDecompress::None })
            .map_err(|_| Fault::corrupt("png_deflate_invalid"))?;
        let consumed = usize::try_from(decoder.total_in() - before_in).map_err(|_| Fault::corrupt("png_deflate_invalid"))?;
        start += consumed;
        if decoder.total_out() > expected { return Err(Fault::corrupt("png_scanline_length_invalid")); }
        if status == Status::StreamEnd {
            if decoder.total_out() != expected || decoder.total_in() != byte_len {
                return Err(Fault::corrupt("png_deflate_length_invalid"));
            }
            return Ok(());
        }
        if decoder.total_in() == before_in && decoder.total_out() == before_out {
            if eof || end - start == input.len() { return Err(Fault::corrupt("png_deflate_truncated")); }
            input.copy_within(start..end, 0);
            end -= start;
            start = 0;
            let read = reader.read(&mut input[end..]).map_err(|_| Fault::corrupt("png_deflate_invalid"))?;
            end += read;
            eof = read == 0;
        }
    }
}

fn validate_webp(encoded: &[u8]) -> Result<(), Fault> {
    if encoded.len() < 12 || &encoded[..4] != b"RIFF" || &encoded[8..12] != b"WEBP"
        || u64::from(u32::from_le_bytes(encoded[4..8].try_into().unwrap())) + 8 != encoded.len() as u64 {
        return Err(Fault::corrupt("webp_container_invalid"));
    }
    let mut position = 12usize;
    let mut chunks = 0;
    let mut payload = false;
    let mut extended = false;
    while position < encoded.len() {
        chunks += 1;
        if chunks > MAX_CONTAINER_CHUNKS { return Err(Fault::resource("image_chunk_limit")); }
        let header = encoded.get(position..position + 8).ok_or_else(|| Fault::corrupt("webp_container_truncated"))?;
        let length = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        let end = position.checked_add(8).and_then(|value| value.checked_add(length))
            .filter(|value| *value <= encoded.len()).ok_or_else(|| Fault::corrupt("webp_container_truncated"))?;
        let padded_end = end.checked_add(length & 1).filter(|value| *value <= encoded.len())
            .ok_or_else(|| Fault::corrupt("webp_container_truncated"))?;
        if padded_end != end && encoded[end] != 0 { return Err(Fault::corrupt("webp_padding_invalid")); }
        match &header[..4] {
            b"VP8X" => {
                if chunks != 1 || extended || length != 10 { return Err(Fault::corrupt("webp_header_invalid")); }
                let flags = encoded[position + 8];
                if flags & 0x02 != 0 { return Err(Fault::unsupported("animated_image_unsupported")); }
                if flags & 0xc1 != 0 || encoded[position + 9..position + 12] != [0, 0, 0] {
                    return Err(Fault::corrupt("webp_header_invalid"));
                }
                extended = true;
            },
            b"ANIM" | b"ANMF" => return Err(Fault::unsupported("animated_image_unsupported")),
            b"VP8 " | b"VP8L" => {
                if payload { return Err(Fault::corrupt("webp_image_payload_repeated")); }
                payload = true;
            },
            _ => {},
        }
        position = padded_end;
    }
    if !payload { return Err(Fault::corrupt("webp_image_payload_missing")); }
    Ok(())
}

fn validate_gif(encoded: &[u8]) -> Result<(), Fault> {
    if encoded.len() < 13 || !matches!(&encoded[..6], b"GIF87a" | b"GIF89a") {
        return Err(Fault::corrupt("gif_header_invalid"));
    }
    let width = u32::from(u16::from_le_bytes(encoded[6..8].try_into().unwrap()));
    let height = u32::from(u16::from_le_bytes(encoded[8..10].try_into().unwrap()));
    check_policy(width, height, None)?;
    let mut position = 13usize;
    if encoded[10] & 0x80 != 0 {
        position += 3usize << (usize::from(encoded[10] & 7) + 1);
        if position > encoded.len() { return Err(Fault::corrupt("gif_palette_truncated")); }
    }
    let mut frames = 0;
    let mut blocks = 0;
    loop {
        blocks += 1;
        if blocks > MAX_CONTAINER_CHUNKS { return Err(Fault::resource("image_chunk_limit")); }
        let marker = *encoded.get(position).ok_or_else(|| Fault::corrupt("gif_trailer_missing"))?;
        position += 1;
        match marker {
            0x3b => {
                if frames != 1 || position != encoded.len() { return Err(Fault::corrupt("gif_end_invalid")); }
                return Ok(());
            },
            0x21 => {
                let extension = *encoded.get(position).ok_or_else(|| Fault::corrupt("gif_extension_truncated"))?;
                position += 1;
                match extension {
                    0xf9 => {
                        if encoded.get(position) != Some(&4) { return Err(Fault::corrupt("gif_control_invalid")); }
                        let end = position.checked_add(6).filter(|end| *end <= encoded.len())
                            .ok_or_else(|| Fault::corrupt("gif_extension_truncated"))?;
                        if encoded[end - 1] != 0 { return Err(Fault::corrupt("gif_control_invalid")); }
                        position = end;
                    },
                    0xff => {
                        if encoded.get(position) != Some(&11) { return Err(Fault::corrupt("gif_application_invalid")); }
                        skip_gif_subblocks(encoded, &mut position)?;
                    },
                    0xfe => skip_gif_subblocks(encoded, &mut position)?,
                    // A plain-text extension has visible content that the image
                    // pixel decoder does not render as a static comic image.
                    _ => return Err(Fault::unsupported("gif_extension_unsupported")),
                }
            },
            0x2c => {
                frames += 1;
                if frames > 1 { return Err(Fault::unsupported("animated_image_unsupported")); }
                let end = position.checked_add(9).filter(|end| *end <= encoded.len())
                    .ok_or_else(|| Fault::corrupt("gif_frame_truncated"))?;
                let header = &encoded[position..end];
                let left = u32::from(u16::from_le_bytes(header[..2].try_into().unwrap()));
                let top = u32::from(u16::from_le_bytes(header[2..4].try_into().unwrap()));
                let frame_width = u32::from(u16::from_le_bytes(header[4..6].try_into().unwrap()));
                let frame_height = u32::from(u16::from_le_bytes(header[6..8].try_into().unwrap()));
                if frame_width == 0 || frame_height == 0 || left + frame_width > width || top + frame_height > height
                    || header[8] & 0x18 != 0 { return Err(Fault::corrupt("gif_frame_invalid")); }
                position = end;
                if header[8] & 0x80 != 0 {
                    position = position.checked_add(3usize << (usize::from(header[8] & 7) + 1))
                        .filter(|end| *end <= encoded.len()).ok_or_else(|| Fault::corrupt("gif_palette_truncated"))?;
                }
                let minimum = *encoded.get(position).ok_or_else(|| Fault::corrupt("gif_lzw_truncated"))?;
                if !(2..=8).contains(&minimum) { return Err(Fault::corrupt("gif_lzw_code_size_invalid")); }
                position += 1;
                skip_gif_subblocks(encoded, &mut position)?;
            },
            _ => return Err(Fault::corrupt("gif_block_invalid")),
        }
    }
}

fn skip_gif_subblocks(encoded: &[u8], position: &mut usize) -> Result<(), Fault> {
    loop {
        let length = usize::from(*encoded.get(*position).ok_or_else(|| Fault::corrupt("gif_subblock_truncated"))?);
        *position += 1;
        if length == 0 { return Ok(()); }
        *position = position.checked_add(length).filter(|end| *end <= encoded.len())
            .ok_or_else(|| Fault::corrupt("gif_subblock_truncated"))?;
    }
}

fn decode_gif(encoded: &[u8]) -> Result<DynamicImage, Fault> {
    Ok(decode_gif_cancellable(encoded, &|| false)?
        .expect("an uncancelled GIF decode must not return cancellation"))
}

fn decode_gif_cancellable(encoded: &[u8], is_cancelled: &dyn Fn() -> bool) -> Result<Option<DynamicImage>, Fault> {
    if is_cancelled() { return Ok(None); }
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::Indexed);
    options.set_memory_limit(gif::MemoryLimit::Bytes(std::num::NonZeroU64::new(MAX_DECODE_ALLOCATION).unwrap()));
    options.check_frame_consistency(true);
    options.check_lzw_end_code(true);
    let mut decoder = options.read_info(Cursor::new(encoded)).map_err(gif_fault)?;
    let width = u32::from(decoder.width());
    let height = u32::from(decoder.height());
    check_policy(width, height, None)?;
    let frame = decoder.next_frame_info().map_err(gif_fault)?
        .ok_or_else(|| Fault::corrupt("gif_frame_missing"))?;
    let (left, top, frame_width, frame_height, interlaced, transparent) =
        (u32::from(frame.left), u32::from(frame.top), u32::from(frame.width), u32::from(frame.height), frame.interlaced, frame.transparent);
    let palette = decoder.palette().map_err(gif_fault)?.to_vec();
    let pixel_bytes = u64::from(width) * u64::from(height) * 4;
    if pixel_bytes + u64::from(frame_width) > MAX_DECODE_ALLOCATION {
        return Err(Fault::resource("image_decode_allocation_limit"));
    }
    if is_cancelled() { return Ok(None); }
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(pixel_bytes as usize).map_err(|_| Fault::resource("image_decode_allocation_limit"))?;
    pixels.resize(pixel_bytes as usize, 0);
    let mut row = vec![0u8; frame_width as usize];
    let passes: &[(u32, u32)] = if interlaced { &[(0, 8), (4, 8), (2, 4), (1, 2)] } else { &[(0, 1)] };
    for &(start, step) in passes {
        for y in (start..frame_height).step_by(step as usize) {
            if is_cancelled() { return Ok(None); }
            if !decoder.fill_buffer(&mut row).map_err(gif_fault)? { return Err(Fault::corrupt("gif_pixels_truncated")); }
            for (x, &index) in row.iter().enumerate() {
                let color = palette.get(usize::from(index) * 3..usize::from(index) * 3 + 3)
                    .ok_or_else(|| Fault::corrupt("gif_palette_index_invalid"))?;
                let destination = (u64::from(top + y) * u64::from(width) + u64::from(left) + x as u64) * 4;
                let destination = usize::try_from(destination).map_err(|_| Fault::resource("image_decode_allocation_limit"))?;
                pixels[destination..destination + 3].copy_from_slice(color);
                pixels[destination + 3] = if transparent == Some(index) { 0 } else { 255 };
            }
        }
    }
    // Keep an output buffer while draining so the underlying reader actually
    // checks the LZW end code rather than skipping the remaining sub-blocks.
    if decoder.fill_buffer(&mut [0u8; 1]).map_err(gif_fault)? {
        return Err(Fault::corrupt("gif_pixels_exceed_frame"));
    }
    if decoder.next_frame_info().map_err(gif_fault)?.is_some() {
        return Err(Fault::unsupported("animated_image_unsupported"));
    }
    let mut tail = [0u8; 1];
    if decoder.into_inner().read(&mut tail).map_err(|_| Fault::corrupt("gif_end_invalid"))? != 0 {
        return Err(Fault::corrupt("gif_end_invalid"));
    }
    let image = image::RgbaImage::from_raw(width, height, pixels).ok_or_else(|| Fault::resource("image_decode_allocation_limit"))?;
    Ok(Some(DynamicImage::ImageRgba8(image)))
}

fn gif_fault(error: gif::DecodingError) -> Fault {
    match error {
        gif::DecodingError::MemoryLimit | gif::DecodingError::OutOfMemory => Fault::resource("image_decode_allocation_limit"),
        _ => Fault::corrupt("gif_decode_invalid"),
    }
}

#[cfg(test)]
#[path = "image_integrity_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "image_integrity_cancel_tests.rs"]
mod cancellation_tests;

