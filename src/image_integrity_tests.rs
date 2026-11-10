use super::*;

use image::{DynamicImage, ImageFormat, ImageReader, Rgb, RgbImage};
use sha2::Digest;
use std::{io::{Cursor, Write}, sync::Arc};

#[test]
fn sealed_pixel_reuse_rehashes_current_bytes_and_avoids_another_decode() {
    let bytes = encoded_fixture(ImageFormat::Png);
    let before = decode_attempt_count();
    let (proof, first) = decode_image_with_reuse(&bytes, None, |_| None::<(ValidatedImage, usize)>)
        .expect("Decode the complete source before caching pixels");
    assert!(matches!(first, ReusableImage::Decoded(_)));
    assert_eq!(decode_attempt_count(), before + 1);
    let identity = proof.sha256().to_owned();
    let (reused_proof, reused) = decode_image_with_reuse(&bytes, None, |digest| {
        assert_eq!(digest, identity);
        Some((proof.clone(), 42usize))
    }).expect("Reuse pixels with the matching sealed proof");
    assert_eq!(reused_proof, proof);
    assert!(matches!(reused, ReusableImage::Reused(42)));
    assert_eq!(decode_attempt_count(), before + 1);
}

#[test]
fn reusable_pixels_do_not_bypass_identity_policy_or_complete_validation() {
    let bytes = encoded_fixture(ImageFormat::Png);
    let proof = validated(&bytes, None);
    let other = encoded_fixture(ImageFormat::Jpeg);
    let before = decode_attempt_count();
    let (_, decoded) = decode_image_with_reuse(&other, None, |_| Some((proof.clone(), 42usize)))
        .expect("A mismatched proof must fall back to complete decoding");
    assert!(matches!(decoded, ReusableImage::Decoded(_)));
    assert_eq!(decode_attempt_count(), before + 1);

    let limited = decode_image_with_reuse(&bytes, Some(1), |_| Some((proof.clone(), 42usize)));
    assert!(matches!(limited, Err(IntegrityError { kind: IntegrityFailureKind::ResourceLimit, .. })));
    let truncated = &bytes[..bytes.len() - 12];
    let invalid = decode_image_with_reuse(truncated, None, |_| Some((proof, 42usize)));
    assert!(matches!(invalid, Err(IntegrityError { kind: IntegrityFailureKind::Corrupt, .. })));
}

#[test]
fn owned_encoded_identity_is_hashed_once_across_decode_and_pixel_reuse() {
    let bytes = encoded_fixture(ImageFormat::Png);
    let expected_digest = format!("{:x}", sha2::Sha256::digest(&bytes));
    let expected_length = bytes.len() as u64;
    let identity_before = identity_computation_count();
    let input = HashedImage::new(bytes).expect("Seal the exact owned encoded bytes");
    assert_eq!(input.sha256(), expected_digest);
    assert_eq!(input.byte_len(), expected_length);
    assert_eq!(identity_computation_count(), identity_before + 1);

    let decode_before = decode_attempt_count();
    let (proof, decoded) = decode_hashed_image_with_reuse_cancellable(
        &input, None, |_| None::<(ValidatedImage, Arc<DynamicImage>)>, &|| false,
    ).expect("Decode the sealed input").expect("The operation was not cancelled");
    let source = match decoded {
        ReusableImage::Decoded(source) => Arc::new(source),
        ReusableImage::Reused(_) => panic!("No pixels were supplied for the first decode"),
    };
    assert_eq!((source.width(), source.height()), (FIXTURE_WIDTH, FIXTURE_HEIGHT));
    assert_eq!(proof.sha256(), input.sha256());
    assert_eq!(proof.byte_len(), input.byte_len());
    assert_eq!(decode_attempt_count(), decode_before + 1);
    assert_eq!(identity_computation_count(), identity_before + 1);

    for _ in 0..3 {
        let (reused_proof, reused) = decode_hashed_image_with_reuse_cancellable(
            &input, None, |digest| {
                assert_eq!(digest, input.sha256());
                Some((proof.clone(), source.clone()))
            }, &|| false,
        ).expect("Reuse exact pixels from the same sealed input").expect("The operation was not cancelled");
        assert_eq!(reused_proof, proof);
        match reused {
            ReusableImage::Reused(retained) => assert!(Arc::ptr_eq(&retained, &source)),
            ReusableImage::Decoded(_) => panic!("An exact pixel hit must avoid codec allocation"),
        }
        assert_eq!(decode_attempt_count(), decode_before + 1);
        assert_eq!(identity_computation_count(), identity_before + 1);
    }

    let (decoded_proof, decoded_again) = decode_hashed_image_with_reuse_cancellable(
        &input, None, |_| None::<(ValidatedImage, Arc<DynamicImage>)>, &|| false,
    ).expect("Decode again when pixels were evicted").expect("The operation was not cancelled");
    assert_eq!(decoded_proof, proof);
    match decoded_again {
        ReusableImage::Decoded(decoded) => assert_eq!(decoded.to_rgba8(), source.to_rgba8()),
        ReusableImage::Reused(_) => panic!("Evicted pixels must be decoded again"),
    }
    assert_eq!(decode_attempt_count(), decode_before + 2);
    assert_eq!(identity_computation_count(), identity_before + 1);
}

#[test]
fn a_different_sealed_proof_cannot_replace_owned_encoded_input() {
    let bytes = encoded_fixture(ImageFormat::Png);
    let expected = image::load_from_memory(&bytes).expect("Decode the independent fixture pixels").to_rgba8();
    let other_proof = validated(&encoded_fixture(ImageFormat::Jpeg), None);
    let input = HashedImage::new(bytes).expect("Seal the current PNG bytes");
    assert_ne!(input.sha256(), other_proof.sha256());
    let hashes = identity_computation_count();
    let decodes = decode_attempt_count();
    let (proof, output) = decode_hashed_image_with_reuse_cancellable(
        &input, None, |_| Some((other_proof, 42usize)), &|| false,
    ).expect("Ignore unrelated cached pixels").expect("The operation was not cancelled");
    assert_eq!(proof.sha256(), input.sha256());
    assert_eq!(proof.byte_len(), input.byte_len());
    match output {
        ReusableImage::Decoded(decoded) => assert_eq!(decoded.to_rgba8(), expected),
        ReusableImage::Reused(_) => panic!("Another input's proof cannot authorize cached pixels"),
    }
    assert_eq!(decode_attempt_count(), decodes + 1);
    assert_eq!(identity_computation_count(), hashes);
}

#[test]
fn owned_input_reuse_still_enforces_a_stricter_pixel_policy() {
    let bytes = encoded_fixture(ImageFormat::Png);
    let proof = validated(&bytes, None);
    let input = HashedImage::new(bytes).expect("Seal the complete input");
    let pixels = u64::from(FIXTURE_WIDTH) * u64::from(FIXTURE_HEIGHT);
    let hashes = identity_computation_count();
    let decodes = decode_attempt_count();
    let error = match decode_hashed_image_with_reuse_cancellable(
        &input, Some(pixels - 1), |_| Some((proof.clone(), 42usize)), &|| false,
    ) {
        Err(error) => error,
        Ok(_) => panic!("A sealed input cannot bypass the caller's smaller pixel budget"),
    };
    assert_eq!(error.kind, IntegrityFailureKind::ResourceLimit);
    assert_eq!(error.code, "image_pixel_limit");
    assert!(error.encoding_evidence().is_none());
    assert_eq!(decode_attempt_count(), decodes);
    assert_eq!(identity_computation_count(), hashes);

    let (accepted, output) = decode_hashed_image_with_reuse_cancellable(
        &input, Some(pixels), |_| Some((proof.clone(), 42usize)), &|| false,
    ).expect("The exact source fits the original pixel budget").expect("The operation was not cancelled");
    assert_eq!(accepted, proof);
    assert!(matches!(output, ReusableImage::Reused(42)));
    assert_eq!(decode_attempt_count(), decodes);
    assert_eq!(identity_computation_count(), hashes);
}

#[test]
fn corrupt_owned_input_cannot_reuse_a_valid_proof_or_cache_a_failure() {
    let valid = encoded_fixture(ImageFormat::Png);
    let old_proof = validated(&valid, None);
    let broken = png_with_idat(FIXTURE_WIDTH, FIXTURE_HEIGHT, &[0x78, 0x9c, 0x07, 0, 0, 0, 0]);
    assert_header_dimensions(&broken, ImageFormat::Png, FIXTURE_WIDTH, FIXTURE_HEIGHT);
    let input = HashedImage::new(broken).expect("An encoded identity is not a successful decode proof");
    assert_ne!(input.sha256(), old_proof.sha256());
    let hashes = identity_computation_count();
    let decodes = decode_attempt_count();
    for attempt in 1..=2 {
        let error = match decode_hashed_image_with_reuse_cancellable(
            &input, None, |_| Some((old_proof.clone(), 42usize)), &|| false,
        ) {
            Err(error) => error,
            Ok(_) => panic!("Corrupt current bytes must not inherit another image's proof"),
        };
        assert_eq!(error.kind, IntegrityFailureKind::Corrupt);
        let evidence = error.encoding_evidence().expect("Corruption must identify the examined owned bytes");
        assert_eq!(evidence.sha256(), input.sha256());
        assert_eq!(evidence.byte_len(), input.byte_len());
        assert_eq!(identity_computation_count(), hashes);
        assert_eq!(decode_attempt_count(), decodes + attempt);
    }
}

const FIXTURE_WIDTH: u32 = 7;
const FIXTURE_HEIGHT: u32 = 11;
static MEMO_TEST_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn encoded_fixture(format: ImageFormat) -> Vec<u8> {
    let pixels = RgbImage::from_fn(FIXTURE_WIDTH, FIXTURE_HEIGHT, |x, y| {
        Rgb([(x * 29 + y * 7) as u8, (x * 11 + y * 19) as u8, (x * 3 + y * 31) as u8])
    });
    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(pixels).write_to(&mut output, format)
        .expect("The small synthetic image must encode");
    output.into_inner()
}

fn validated(bytes: &[u8], max_pixels: Option<u64>) -> ValidatedImage {
    match validate_image(bytes, max_pixels) {
        Ok(image) => image,
        Err(error) => panic!("A valid synthetic image was rejected: {}", error.code),
    }
}

fn rejected(bytes: &[u8], max_pixels: Option<u64>) -> IntegrityError {
    match validate_image(bytes, max_pixels) {
        Ok(_) => panic!("An invalid or disallowed synthetic image was accepted"),
        Err(error) => error,
    }
}

fn assert_header_dimensions(bytes: &[u8], format: ImageFormat, width: u32, height: u32) {
    let dimensions = ImageReader::with_format(Cursor::new(bytes), format).into_dimensions()
        .expect("The counterexample must retain a readable image header");
    assert_eq!(dimensions, (width, height));
}

fn append_png_chunk(bytes: &mut Vec<u8>, kind: &[u8; 4], payload: &[u8]) {
    bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    bytes.extend_from_slice(kind);
    bytes.extend_from_slice(payload);
    let mut checksum = crc32fast::Hasher::new();
    checksum.update(kind);
    checksum.update(payload);
    bytes.extend_from_slice(&checksum.finalize().to_be_bytes());
}

fn png_with_idat(width: u32, height: u32, payload: &[u8]) -> Vec<u8> {
    png_with_color(width, height, 8, 0, payload)
}

fn png_with_color(width: u32, height: u32, bit_depth: u8, color_type: u8, payload: &[u8]) -> Vec<u8> {
    png_with_color_chunks(width, height, bit_depth, color_type, &[payload])
}

fn png_with_color_chunks(width: u32, height: u32, bit_depth: u8, color_type: u8, payloads: &[&[u8]]) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[bit_depth, color_type, 0, 0, 0]);
    append_png_chunk(&mut bytes, b"IHDR", &header);
    for payload in payloads { append_png_chunk(&mut bytes, b"IDAT", payload); }
    append_png_chunk(&mut bytes, b"IEND", &[]);
    bytes
}

fn complete_grayscale_zlib() -> Vec<u8> {
    let mut scanlines = Vec::new();
    for y in 0..FIXTURE_HEIGHT {
        scanlines.push(0);
        scanlines.extend((0..FIXTURE_WIDTH).map(|x| (x * 17 + y * 9) as u8));
    }
    let mut compressed = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    compressed.write_all(&scanlines).expect("The complete synthetic scanlines must compress");
    compressed.finish().expect("The complete synthetic zlib stream must finish")
}

fn with_unique_png_text(mut bytes: Vec<u8>, label: &str, sequence: usize) -> Vec<u8> {
    let tail = bytes.split_off(bytes.len() - 12);
    assert_eq!(&tail[4..8], b"IEND");
    let text = format!("integrity-test\0{label}-{sequence}");
    append_png_chunk(&mut bytes, b"tEXt", text.as_bytes());
    bytes.extend_from_slice(&tail);
    bytes
}

fn jpeg_entropy_offset(bytes: &[u8]) -> usize {
    assert!(bytes.starts_with(&[0xff, 0xd8]));
    let mut position = 2;
    loop {
        assert_eq!(bytes[position], 0xff);
        while bytes[position] == 0xff { position += 1; }
        let marker = bytes[position];
        position += 1;
        assert!(!matches!(marker, 0xd8 | 0xd9 | 0x01 | 0xd0..=0xd7));
        let length = usize::from(u16::from_be_bytes([bytes[position], bytes[position + 1]]));
        assert!(length >= 2);
        let end = position.checked_add(length).expect("The synthetic JPEG segment must fit");
        assert!(end <= bytes.len());
        if marker == 0xda { return end; }
        position = end;
    }
}

fn append_jpeg_segment(bytes: &mut Vec<u8>, marker: u8, payload: &[u8]) {
    bytes.extend_from_slice(&[0xff, marker]);
    let length = u16::try_from(payload.len() + 2).expect("The synthetic JPEG segment must fit");
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.extend_from_slice(payload);
}

fn append_zero_jpeg_huffman_tables(bytes: &mut Vec<u8>) {
    for selector in [0x00, 0x10] {
        // A single code 0 represents category 0 or EOB; all-one padding is unused.
        let mut table = vec![selector, 1];
        table.extend_from_slice(&[0; 15]);
        table.push(0);
        append_jpeg_segment(bytes, 0xc4, &table);
    }
}

fn sequential_grayscale_jpeg(width: u16, height: u16, entropy: &[u8], interval: Option<u16>) -> Vec<u8> {
    let mut bytes = vec![0xff, 0xd8];
    let mut quantization = vec![0];
    quantization.extend_from_slice(&[1; 64]);
    append_jpeg_segment(&mut bytes, 0xdb, &quantization);
    let mut frame = vec![8];
    frame.extend_from_slice(&height.to_be_bytes());
    frame.extend_from_slice(&width.to_be_bytes());
    frame.extend_from_slice(&[1, 1, 0x11, 0]);
    append_jpeg_segment(&mut bytes, 0xc0, &frame);
    append_zero_jpeg_huffman_tables(&mut bytes);
    if let Some(interval) = interval { append_jpeg_segment(&mut bytes, 0xdd, &interval.to_be_bytes()); }
    append_jpeg_segment(&mut bytes, 0xda, &[1, 1, 0, 0, 63, 0]);
    bytes.extend_from_slice(entropy);
    bytes.extend_from_slice(&[0xff, 0xd9]);
    bytes
}

fn sequential_separate_component_scans(
    width: u16, height: u16, components: &[(u8, u8)], entropy: &[&[u8]], interval: Option<u16>,
) -> Vec<u8> {
    assert_eq!(components.len(), entropy.len());
    let mut bytes = vec![0xff, 0xd8];
    let mut quantization = vec![0];
    quantization.extend_from_slice(&[1; 64]);
    append_jpeg_segment(&mut bytes, 0xdb, &quantization);
    let mut frame = vec![8];
    frame.extend_from_slice(&height.to_be_bytes());
    frame.extend_from_slice(&width.to_be_bytes());
    frame.push(u8::try_from(components.len()).expect("The synthetic frame must have few components"));
    for &(id, sampling) in components { frame.extend_from_slice(&[id, sampling, 0]); }
    append_jpeg_segment(&mut bytes, 0xc0, &frame);
    append_zero_jpeg_huffman_tables(&mut bytes);
    if let Some(interval) = interval { append_jpeg_segment(&mut bytes, 0xdd, &interval.to_be_bytes()); }
    for (&(id, _), data) in components.iter().zip(entropy) {
        append_jpeg_segment(&mut bytes, 0xda, &[1, id, 0, 0, 63, 0]);
        bytes.extend_from_slice(data);
    }
    bytes.extend_from_slice(&[0xff, 0xd9]);
    bytes
}

fn assert_jpeg_unsupported_without_evidence(bytes: &[u8]) {
    let decoded = match decode_image(bytes, None) {
        Ok(_) => panic!("An out-of-contract JPEG must not be decoded as supported"),
        Err(error) => error,
    };
    for error in [rejected(bytes, None), decoded] {
        assert_eq!(error.kind, IntegrityFailureKind::Unsupported);
        assert!(error.encoding_evidence().is_none());
    }
}

fn progressive_zero_jpeg() -> (Vec<u8>, usize, usize) {
    let mut bytes = vec![0xff, 0xd8];
    let mut quantization = vec![0];
    quantization.extend_from_slice(&[1; 64]);
    append_jpeg_segment(&mut bytes, 0xdb, &quantization);
    append_jpeg_segment(&mut bytes, 0xc2, &[8, 0, 8, 0, 8, 1, 1, 0x11, 0]);
    append_zero_jpeg_huffman_tables(&mut bytes);
    append_jpeg_segment(&mut bytes, 0xda, &[1, 1, 0, 0, 0, 0]);
    let dc_entropy = bytes.len();
    bytes.push(0x7f);
    append_jpeg_segment(&mut bytes, 0xda, &[1, 1, 0, 1, 63, 0]);
    let ac_entropy = bytes.len();
    bytes.push(0x7f);
    bytes.extend_from_slice(&[0xff, 0xd9]);
    (bytes, dc_entropy, ac_entropy)
}

#[test]
fn complete_png_jpeg_and_webp_payloads_return_their_verified_identity() {
    for (format, extension) in [
        (ImageFormat::Png, "png"), (ImageFormat::Jpeg, "jpg"), (ImageFormat::WebP, "webp"),
    ] {
        let bytes = encoded_fixture(format);
        let image = validated(&bytes, None);
        assert_eq!((image.width, image.height), (FIXTURE_WIDTH, FIXTURE_HEIGHT));
        assert_eq!(image.format, format);
        assert_eq!(image.extension, extension);
        assert_eq!(image.sha256, format!("{:x}", sha2::Sha256::digest(&bytes)));
    }
}

#[test]
fn png_with_valid_chunks_and_checksums_but_invalid_deflate_is_corrupt() {
    // The zlib header is valid; the DEFLATE block uses the reserved block type.
    // Every PNG chunk, including IDAT and IEND, has a newly calculated CRC.
    let bytes = png_with_idat(FIXTURE_WIDTH, FIXTURE_HEIGHT, &[0x78, 0x9c, 0x07, 0, 0, 0, 0]);
    assert_header_dimensions(&bytes, ImageFormat::Png, FIXTURE_WIDTH, FIXTURE_HEIGHT);
    let error = rejected(&bytes, None);
    assert!(matches!(error.kind, IntegrityFailureKind::Corrupt));
}

#[test]
fn png_with_a_complete_zlib_stream_but_missing_scanlines_is_corrupt() {
    let mut compressed = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    compressed.write_all(&[0, 37]).expect("The synthetic short scanline must compress");
    let payload = compressed.finish().expect("The synthetic zlib stream must finish");
    let bytes = png_with_idat(FIXTURE_WIDTH, FIXTURE_HEIGHT, &payload);
    assert_header_dimensions(&bytes, ImageFormat::Png, FIXTURE_WIDTH, FIXTURE_HEIGHT);
    let error = rejected(&bytes, None);
    assert!(matches!(error.kind, IntegrityFailureKind::Corrupt));
}

#[test]
fn one_png_zlib_stream_may_cross_multiple_idat_chunks() {
    let payload = complete_grayscale_zlib();
    let split = payload.len() / 2;
    let bytes = png_with_color_chunks(FIXTURE_WIDTH, FIXTURE_HEIGHT, 8, 0,
        &[&payload[..split], &payload[split..]]);
    let proof = validated(&bytes, None);
    assert_eq!((proof.width, proof.height), (FIXTURE_WIDTH, FIXTURE_HEIGHT));
}

#[test]
fn png_with_all_scanlines_but_without_the_adler_checksum_is_corrupt() {
    let mut payload = complete_grayscale_zlib();
    payload.truncate(payload.len() - 4);
    let bytes = png_with_idat(FIXTURE_WIDTH, FIXTURE_HEIGHT, &payload);
    assert_header_dimensions(&bytes, ImageFormat::Png, FIXTURE_WIDTH, FIXTURE_HEIGHT);
    let error = rejected(&bytes, None);
    assert!(matches!(error.kind, IntegrityFailureKind::Corrupt));
}

#[test]
fn png_with_a_wrong_adler_checksum_and_recomputed_chunk_crcs_is_corrupt() {
    let mut payload = complete_grayscale_zlib();
    let last = payload.len() - 1;
    payload[last] ^= 0xff;
    let bytes = png_with_idat(FIXTURE_WIDTH, FIXTURE_HEIGHT, &payload);
    assert_header_dimensions(&bytes, ImageFormat::Png, FIXTURE_WIDTH, FIXTURE_HEIGHT);
    let error = rejected(&bytes, None);
    assert!(matches!(error.kind, IntegrityFailureKind::Corrupt));
}

#[test]
fn png_rejects_an_additional_idat_payload_after_the_complete_zlib_stream() {
    let payload = complete_grayscale_zlib();
    let bytes = png_with_color_chunks(FIXTURE_WIDTH, FIXTURE_HEIGHT, 8, 0,
        &[&payload, &[0x13, 0x37, 0x42]]);
    assert_header_dimensions(&bytes, ImageFormat::Png, FIXTURE_WIDTH, FIXTURE_HEIGHT);
    let error = rejected(&bytes, None);
    assert!(matches!(error.kind, IntegrityFailureKind::Corrupt));
}

#[test]
fn jpeg_with_sof_and_eoi_but_without_entropy_data_is_corrupt() {
    let original = encoded_fixture(ImageFormat::Jpeg);
    let mut bytes = original[..jpeg_entropy_offset(&original)].to_vec();
    bytes.extend_from_slice(&[0xff, 0xd9]);
    assert!(bytes.starts_with(&[0xff, 0xd8]) && bytes.ends_with(&[0xff, 0xd9]));
    assert_header_dimensions(&bytes, ImageFormat::Jpeg, FIXTURE_WIDTH, FIXTURE_HEIGHT);
    let error = rejected(&bytes, None);
    assert!(matches!(error.kind, IntegrityFailureKind::Corrupt));
}

#[test]
fn jpeg_with_only_partial_entropy_and_a_replaced_eoi_is_corrupt() {
    let original = encoded_fixture(ImageFormat::Jpeg);
    let start = jpeg_entropy_offset(&original);
    let entropy_length = original.len() - start - 2;
    assert!(entropy_length > 2);
    let mut bytes = original[..start + entropy_length / 2].to_vec();
    bytes.extend_from_slice(&[0xff, 0xd9]);
    assert_header_dimensions(&bytes, ImageFormat::Jpeg, FIXTURE_WIDTH, FIXTURE_HEIGHT);
    let error = rejected(&bytes, None);
    assert!(matches!(error.kind, IntegrityFailureKind::Corrupt));
}

#[test]
fn a_twelve_bit_extended_jpeg_is_unsupported_without_corruption_evidence() {
    let mut bytes = sequential_grayscale_jpeg(8, 8, &[0x3f], None);
    let frame = bytes.windows(2).position(|marker| marker == [0xff, 0xc0])
        .expect("The synthetic JPEG must contain its frame header");
    bytes[frame + 1] = 0xc1;
    bytes[frame + 4] = 12;
    // Extended sequential JPEG permits twelve-bit samples. An unsupported
    // precision is not evidence that a legacy object's encoding is corrupt.
    for error in [rejected(&bytes, None), decode_image(&bytes, None).unwrap_err()] {
        assert_eq!(error.kind, IntegrityFailureKind::Unsupported);
        assert!(error.encoding_evidence().is_none());
    }
}

#[test]
fn a_jpeg_with_dnl_height_is_unsupported_without_corruption_evidence() {
    let mut bytes = sequential_grayscale_jpeg(8, 0, &[0x3f], None);
    bytes.truncate(bytes.len() - 2);
    // DNL supplies the height after the first scan when SOF declares zero lines.
    append_jpeg_segment(&mut bytes, 0xdc, &8u16.to_be_bytes());
    bytes.extend_from_slice(&[0xff, 0xd9]);
    for error in [rejected(&bytes, None), decode_image(&bytes, None).unwrap_err()] {
        assert_eq!(error.kind, IntegrityFailureKind::Unsupported);
        assert!(error.encoding_evidence().is_none());
    }
}

#[test]
fn separate_sequential_scans_with_subsampled_color_are_unsupported_without_evidence() {
    // The Y scan contains four blocks; each chroma scan contains one block.
    let bytes = sequential_separate_component_scans(16, 16,
        &[(1, 0x22), (2, 0x11), (3, 0x11)], &[&[0x00], &[0x3f], &[0x3f]], None);
    assert_jpeg_unsupported_without_evidence(&bytes);
}

#[test]
fn separate_sequential_scans_reset_restart_count_but_remain_unsupported() {
    // Each scan contains two MCUs, below DRI 3; no scan needs a restart marker.
    let bytes = sequential_separate_component_scans(16, 8,
        &[(1, 0x11), (2, 0x11), (3, 0x11)], &[&[0x0f], &[0x0f], &[0x0f]], Some(3));
    assert_jpeg_unsupported_without_evidence(&bytes);
}

#[test]
fn a_single_sequential_component_with_nonunit_sampling_is_unsupported_without_evidence() {
    let bytes = sequential_separate_component_scans(16, 16, &[(1, 0x22)], &[&[0x00]], None);
    assert_jpeg_unsupported_without_evidence(&bytes);
}

#[test]
fn malformed_baseline_scan_tails_are_corrupt_and_bind_evidence_to_the_examined_bytes() {
    for tail in [[1, 63, 0], [0, 62, 0], [0, 63, 0x10], [0, 63, 1]] {
        let mut bytes = sequential_grayscale_jpeg(8, 8, &[0x3f], None);
        let end = jpeg_entropy_offset(&bytes);
        bytes[end - 3..end].copy_from_slice(&tail);
        let decoded = match decode_image(&bytes, None) {
            Ok(_) => panic!("Malformed baseline SOS parameters must not decode"),
            Err(error) => error,
        };
        for error in [rejected(&bytes, None), decoded] {
            assert_eq!(error.kind, IntegrityFailureKind::Corrupt, "Baseline SOS tail {tail:?}");
            let evidence = error.encoding_evidence().expect("A malformed baseline SOS must have corruption evidence");
            assert_eq!(evidence.sha256(), format!("{:x}", sha2::Sha256::digest(&bytes)));
            assert_eq!(evidence.byte_len(), bytes.len() as u64);
        }
    }
}

#[test]
fn repeated_full_sequential_scans_hit_the_capability_guard_without_corruption_evidence() {
    let mut bytes = sequential_grayscale_jpeg(8, 8, &[0x3f], None);
    bytes.truncate(bytes.len() - 2);
    // This duplicate full scan is not asserted to be a conforming JPEG.
    // The capability guard must stop before a partial decoder result is trusted.
    append_jpeg_segment(&mut bytes, 0xda, &[1, 1, 0, 0, 63, 0]);
    bytes.extend_from_slice(&[0x3f, 0xff, 0xd9]);
    assert_jpeg_unsupported_without_evidence(&bytes);
}

#[test]
fn a_complete_progressive_jpeg_decodes_both_dc_and_ac_scans() {
    let (bytes, _, _) = progressive_zero_jpeg();
    assert_header_dimensions(&bytes, ImageFormat::Jpeg, 8, 8);
    let proof = validated(&bytes, None);
    let (decoded_proof, decoded) = match decode_image(&bytes, None) {
        Ok(image) => image,
        Err(error) => panic!("A complete progressive JPEG must decode: {}", error.code),
    };
    assert_eq!(decoded_proof, proof);
    assert_eq!((decoded.width(), decoded.height()), (8, 8));
    assert_eq!(decoded.to_luma8().into_raw(), vec![128; 64]);
}

#[test]
fn a_progressive_jpeg_cannot_replace_missing_dc_entropy_with_the_next_scan_marker() {
    let (mut bytes, dc_entropy, _) = progressive_zero_jpeg();
    bytes.remove(dc_entropy);
    assert_header_dimensions(&bytes, ImageFormat::Jpeg, 8, 8);
    let error = rejected(&bytes, None);
    eprintln!("Progressive missing-DC integrity result: {error:?}");
    assert_eq!(error.kind, IntegrityFailureKind::Corrupt, "Unexpected JPEG classification: {error:?}");
    let evidence = error.encoding_evidence().expect("Missing DC entropy must provide corruption evidence");
    assert_eq!(evidence.sha256(), format!("{:x}", sha2::Sha256::digest(&bytes)));
    assert_eq!(evidence.byte_len(), bytes.len() as u64);
}

#[test]
fn a_progressive_jpeg_cannot_replace_missing_ac_entropy_with_the_end_marker() {
    let (mut bytes, _, ac_entropy) = progressive_zero_jpeg();
    bytes.remove(ac_entropy);
    assert_header_dimensions(&bytes, ImageFormat::Jpeg, 8, 8);
    let error = rejected(&bytes, None);
    assert!(matches!(error.kind, IntegrityFailureKind::Corrupt));
}

#[test]
fn complete_sequential_jpeg_blocks_decode_to_the_declared_pixels() {
    let bytes = sequential_grayscale_jpeg(16, 8, &[0x0f], None);
    let (_, decoded) = match decode_image(&bytes, None) {
        Ok(image) => image,
        Err(error) => panic!("Two complete sequential blocks must decode: {}", error.code),
    };
    assert_eq!((decoded.width(), decoded.height()), (16, 8));
    assert_eq!(decoded.to_luma8().into_raw(), vec![128; 128]);
}

#[test]
fn sequential_jpeg_padding_cannot_supply_a_missing_declared_block() {
    let bytes = sequential_grayscale_jpeg(16, 8, &[0x3f], None);
    assert_header_dimensions(&bytes, ImageFormat::Jpeg, 16, 8);
    let error = rejected(&bytes, None);
    eprintln!("Sequential missing-block integrity result: {error:?}");
    assert_eq!(error.kind, IntegrityFailureKind::Corrupt, "Unexpected JPEG classification: {error:?}");
    let evidence = error.encoding_evidence().expect("Missing sequential entropy must provide corruption evidence");
    assert_eq!(evidence.sha256(), format!("{:x}", sha2::Sha256::digest(&bytes)));
    assert_eq!(evidence.byte_len(), bytes.len() as u64);
}

#[test]
fn jpeg_restart_intervals_require_ordered_markers_between_complete_mcus() {
    let entropy = [0x3f, 0xff, 0xd0, 0x3f, 0xff, 0xd1, 0x3f];
    validated(&sequential_grayscale_jpeg(24, 8, &entropy, Some(1)), None);
    let mut wrong_order = entropy;
    wrong_order[5] = 0xd2;
    let error = rejected(&sequential_grayscale_jpeg(24, 8, &wrong_order, Some(1)), None);
    assert!(matches!(error.kind, IntegrityFailureKind::Corrupt));
    let error = rejected(&sequential_grayscale_jpeg(16, 8, &[0x0f], Some(1)), None);
    assert!(matches!(error.kind, IntegrityFailureKind::Corrupt));
}

#[test]
fn jpeg_marker_fill_is_accepted_but_extra_scan_or_terminal_bytes_are_not() {
    validated(&sequential_grayscale_jpeg(8, 8, &[0x3f, 0xff], None), None);
    let error = rejected(&sequential_grayscale_jpeg(8, 8, &[0x3f, 0], None), None);
    assert!(matches!(error.kind, IntegrityFailureKind::Corrupt));
    let mut bytes = sequential_grayscale_jpeg(8, 8, &[0x3f], None);
    bytes.push(0);
    let error = rejected(&bytes, None);
    assert!(matches!(error.kind, IntegrityFailureKind::Corrupt));
}

#[test]
fn all_one_jpeg_huffman_codes_cannot_reinterpret_alignment_padding_as_blocks() {
    let mut bytes = sequential_grayscale_jpeg(8, 8, &[0x3f], None);
    let table = bytes.windows(2).position(|marker| marker[0] == 0xff && marker[1] == 0xc4)
        .expect("The synthetic JPEG must contain a DC Huffman table");
    let old_length = usize::from(u16::from_be_bytes([bytes[table + 2], bytes[table + 3]]));
    bytes[table + 5] = 2;
    bytes.insert(table + 2 + old_length, 1);
    bytes[table + 2..table + 4].copy_from_slice(&u16::try_from(old_length + 1).unwrap().to_be_bytes());
    let error = rejected(&bytes, None);
    assert!(matches!(error.kind, IntegrityFailureKind::Corrupt));
}

#[test]
fn webp_with_a_valid_riff_length_and_vp8l_header_but_no_image_body_is_corrupt() {
    // The final byte is RIFF padding, not part of the five-byte VP8L payload.
    // Header inspection succeeds without reading the missing transform data.
    let bytes = vec![
        b'R', b'I', b'F', b'F', 18, 0, 0, 0,
        b'W', b'E', b'B', b'P',
        b'V', b'P', b'8', b'L', 5, 0, 0, 0,
        0x2f, 0x06, 0x80, 0x02, 0x00, 0x00,
    ];
    assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize + 8, bytes.len());
    assert_header_dimensions(&bytes, ImageFormat::WebP, FIXTURE_WIDTH, FIXTURE_HEIGHT);
    let error = rejected(&bytes, None);
    assert!(matches!(error.kind, IntegrityFailureKind::Corrupt));
}

#[test]
fn missing_container_tails_are_rejected_for_all_supported_formats() {
    for format in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP, ImageFormat::Gif] {
        let mut bytes = encoded_fixture(format);
        let removed = if format == ImageFormat::Jpeg { 2 } else { 1 };
        bytes.truncate(bytes.len() - removed);
        let error = rejected(&bytes, None);
        assert!(matches!(error.kind, IntegrityFailureKind::Corrupt));
    }
}

#[test]
fn oversized_source_header_is_rejected_before_its_invalid_body_is_decoded() {
    let (width, height) = (8_000, 4_001);
    assert!(u64::from(width) * u64::from(height) > MAX_SOURCE_PIXELS);
    let bytes = png_with_idat(width, height, &[0x78, 0x9c, 0x07, 0, 0, 0, 0]);
    assert!(bytes.len() < 100);
    assert_header_dimensions(&bytes, ImageFormat::Png, width, height);
    let error = rejected(&bytes, None);
    assert!(matches!(error.kind, IntegrityFailureKind::ResourceLimit));
}

#[test]
fn an_oversized_decoded_buffer_is_rejected_before_the_invalid_pixel_body() {
    let (width, height) = (8_000, 4_000);
    let pixels = u64::from(width) * u64::from(height);
    assert!(pixels <= MAX_SOURCE_PIXELS && pixels * 8 > MAX_DECODE_ALLOCATION);
    let bytes = png_with_color(width, height, 16, 6, &[0x78, 0x9c, 0x07, 0, 0, 0, 0]);
    assert!(bytes.len() < 100);
    assert_header_dimensions(&bytes, ImageFormat::Png, width, height);
    let error = rejected(&bytes, None);
    assert!(matches!(error.kind, IntegrityFailureKind::ResourceLimit));
}

#[test]
fn a_stricter_cover_pixel_limit_is_applied_after_a_successful_page_validation() {
    let bytes = encoded_fixture(ImageFormat::Png);
    let pixels = u64::from(FIXTURE_WIDTH) * u64::from(FIXTURE_HEIGHT);
    validated(&bytes, None);
    let error = rejected(&bytes, Some(pixels - 1));
    assert!(matches!(error.kind, IntegrityFailureKind::ResourceLimit));
    validated(&bytes, Some(pixels));
    let error = rejected(&bytes, Some(1));
    assert!(matches!(error.kind, IntegrityFailureKind::ResourceLimit));
    validated(&bytes, None);
}

#[test]
fn a_complete_static_gif_is_supported_and_decodes_its_single_frame() {
    let bytes = encoded_fixture(ImageFormat::Gif);
    assert_header_dimensions(&bytes, ImageFormat::Gif, FIXTURE_WIDTH, FIXTURE_HEIGHT);
    let proof = validated(&bytes, None);
    assert_eq!(proof.format, ImageFormat::Gif);
    assert_eq!(proof.extension, "gif");
    assert_eq!((proof.width, proof.height), (FIXTURE_WIDTH, FIXTURE_HEIGHT));
    let (decoded_proof, decoded) = match decode_image(&bytes, None) {
        Ok(image) => image,
        Err(error) => panic!("A complete static GIF must decode: {}", error.code),
    };
    assert_eq!(decoded_proof, proof);
    assert_eq!((decoded.width(), decoded.height()), (FIXTURE_WIDTH, FIXTURE_HEIGHT));
}

#[test]
fn a_static_gif_with_complete_blocks_and_trailer_but_short_lzw_data_is_corrupt() {
    let bytes = vec![
        b'G', b'I', b'F', b'8', b'9', b'a',
        7, 0, 11, 0, 0x80, 0, 0,
        0, 0, 0, 255, 255, 255,
        0x2c, 0, 0, 0, 0, 7, 0, 11, 0, 0,
        // A clear code and at most one pixel cannot fill the declared frame.
        2, 1, 0x04, 0, 0x3b,
    ];
    assert_header_dimensions(&bytes, ImageFormat::Gif, FIXTURE_WIDTH, FIXTURE_HEIGHT);
    let error = rejected(&bytes, None);
    assert!(matches!(error.kind, IntegrityFailureKind::Corrupt));
}

#[test]
fn a_valid_multi_frame_gif_is_explicitly_unsupported() {
    let mut bytes = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
        let frames = [17, 91].map(|tone| image::Frame::new(image::RgbaImage::from_pixel(
            FIXTURE_WIDTH, FIXTURE_HEIGHT, image::Rgba([tone, tone, tone, 255]))));
        encoder.encode_frames(frames.into_iter()).expect("The synthetic animated GIF must encode");
    }
    assert_header_dimensions(&bytes, ImageFormat::Gif, FIXTURE_WIDTH, FIXTURE_HEIGHT);
    let error = rejected(&bytes, None);
    assert!(matches!(error.kind, IntegrityFailureKind::Unsupported));
}

#[test]
fn a_successful_exact_byte_memo_hit_does_not_decode_the_same_image_again() {
    let _serial = MEMO_TEST_SERIAL.lock().unwrap_or_else(|error| error.into_inner());
    let bytes = with_unique_png_text(encoded_fixture(ImageFormat::Png), "successful-hit", 0);
    let before = decode_attempt_count();
    let first = validated(&bytes, None);
    assert_eq!(decode_attempt_count(), before + 1);
    let second = validated(&bytes, None);
    assert_eq!(decode_attempt_count(), before + 1);
    assert_eq!(first, second);
}

#[test]
fn corrupt_encoding_failures_are_examined_again_instead_of_entering_the_success_memo() {
    let _serial = MEMO_TEST_SERIAL.lock().unwrap_or_else(|error| error.into_inner());
    let broken = png_with_idat(FIXTURE_WIDTH, FIXTURE_HEIGHT, &[0x78, 0x9c, 0x07, 0, 0, 0, 0]);
    let bytes = with_unique_png_text(broken, "failed-validation", 0);
    let before = decode_attempt_count();
    let first = rejected(&bytes, None);
    let second = rejected(&bytes, None);
    assert!(matches!(first.kind, IntegrityFailureKind::Corrupt));
    assert!(matches!(second.kind, IntegrityFailureKind::Corrupt));
    assert_eq!(decode_attempt_count(), before + 2);
}

#[test]
fn successful_validation_memo_is_bounded_and_evicts_the_least_recently_used_proof() {
    let _serial = MEMO_TEST_SERIAL.lock().unwrap_or_else(|error| error.into_inner());
    let capacity = memo_capacity();
    assert!(capacity > 0);
    let original = encoded_fixture(ImageFormat::Png);
    let mut local = ValidationMemo::default();
    let mut proofs = Vec::with_capacity(capacity);
    for sequence in 0..capacity {
        let bytes = with_unique_png_text(original.clone(), "bounded-memo", sequence);
        let proof = validated(&bytes, None);
        local.insert(proof.clone());
        proofs.push(proof);
        assert!(memo_len() <= capacity);
    }
    let first = proofs[0].sha256.clone();
    assert!(local.get(&first).is_some());
    let additional = with_unique_png_text(original, "bounded-memo", capacity);
    let additional = validated(&additional, None);
    let newest = additional.sha256.clone();
    local.insert(additional);
    assert_eq!(local.entries.len(), capacity);
    if capacity > 1 {
        assert!(local.get(&proofs[1].sha256).is_none());
        assert!(local.get(&first).is_some());
    } else {
        assert!(local.get(&first).is_none());
    }
    assert!(local.get(&newest).is_some());
    assert!(memo_len() <= capacity);
}
