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

#[test]
fn an_already_cancelled_owned_input_never_reaches_lookup_or_codec() {
    let input = HashedImage::new(b"not an image".to_vec()).unwrap();
    let looked_up = Cell::new(false);
    let hashes = identity_computation_count();
    let decodes = decode_attempt_count();
    let result = decode_hashed_image_with_reuse_cancellable(
        &input, None, |_| {
            looked_up.set(true);
            None::<(ValidatedImage, usize)>
        }, &|| true,
    ).expect("Cancellation must not manufacture corrupt-encoding evidence");
    assert!(result.is_none());
    assert!(!looked_up.get());
    assert_eq!(decode_attempt_count(), decodes);
    assert_eq!(identity_computation_count(), hashes);

    let error = match decode_hashed_image_with_reuse_cancellable(
        &input, None, |_| None::<(ValidatedImage, usize)>, &|| false,
    ) {
        Err(error) => error,
        Ok(_) => panic!("The uncancelled malformed input must still be examined"),
    };
    assert_eq!(error.kind, IntegrityFailureKind::Corrupt);
    let evidence = error.encoding_evidence().expect("The actual malformed input must bind its failure evidence");
    assert_eq!(evidence.sha256(), input.sha256());
    assert_eq!(evidence.byte_len(), input.byte_len());
    assert_eq!(decode_attempt_count(), decodes + 1);
    assert_eq!(identity_computation_count(), hashes);
}

#[test]
fn cancellation_from_owned_input_lookup_precedes_policy_and_codec_work() {
    let bytes = fixture();
    let proof = validate_image(&bytes, None).unwrap();
    let input = HashedImage::new(bytes).unwrap();
    let hashes = identity_computation_count();
    let decodes = decode_attempt_count();
    for return_cached_pixels in [false, true] {
        let cancelled = Cell::new(false);
        let result = decode_hashed_image_with_reuse_cancellable(
            &input, Some(1), |digest| {
                assert_eq!(digest, input.sha256());
                cancelled.set(true);
                return_cached_pixels.then(|| (proof.clone(), 42usize))
            }, &|| cancelled.get(),
        ).expect("Cancellation after lookup must win over the smaller pixel limit");
        assert!(result.is_none());
        assert_eq!(decode_attempt_count(), decodes);
        assert_eq!(identity_computation_count(), hashes);
    }

    let (accepted, result) = decode_hashed_image_with_reuse_cancellable(
        &input, None, |_| Some((proof.clone(), 42usize)), &|| false,
    ).expect("Cancelled lookups must not poison later reuse").expect("The later operation was not cancelled");
    assert_eq!(accepted, proof);
    assert!(matches!(result, ReusableImage::Reused(42)));
    assert_eq!(decode_attempt_count(), decodes);
    assert_eq!(identity_computation_count(), hashes);
}
