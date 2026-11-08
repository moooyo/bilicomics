use super::*;

use std::{cell::Cell, io::Cursor};

fn fixture() -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::new_luma8(9, 13).write_to(&mut bytes, ImageFormat::Png).unwrap();
    bytes.into_inner()
}

#[test]
fn cancellation_after_hashing_skips_codec_allocation_and_does_not_create_a_failure_proof() {
    let bytes = fixture();
    let checks = Cell::new(0usize);
    let before = decode_attempt_count();
    let result = validate_image_cancellable(&bytes, None, &|| {
        let count = checks.get() + 1; checks.set(count); count >= 2
    }).unwrap();
    assert!(result.is_none());
    assert_eq!(decode_attempt_count(), before);
    assert!(validate_image(&bytes, None).is_ok());
}

#[test]
fn already_cancelled_malformed_input_is_not_reported_as_corruption() {
    assert!(validate_image_cancellable(b"not an image", None, &|| true).unwrap().is_none());
    let error = validate_image(b"not an image", None).unwrap_err();
    assert_eq!(error.kind, IntegrityFailureKind::Corrupt);
    assert!(error.encoding_evidence().is_some());
}

#[test]
fn an_uncancelled_hook_preserves_validation_policy() {
    let bytes = fixture();
    let checked = validate_image_cancellable(&bytes, None, &|| false).unwrap().unwrap();
    assert_eq!((checked.width, checked.height), (9, 13));
    let error = validate_image_cancellable(&bytes, Some(1), &|| false).unwrap_err();
    assert_eq!(error.kind, IntegrityFailureKind::ResourceLimit);
    assert!(error.encoding_evidence().is_none());
}
