use super::*;

fn cache_for(bytes: &[u8]) -> SigningAssetCache {
    SigningAssetCache::new(&format!("{:x}", Sha256::digest(bytes)))
}

#[test]
fn warm_asset_uses_one_read_and_hash_and_keeps_one_immutable_snapshot() {
    let directory = tempfile::tempdir().unwrap(); let path = directory.path().join("synthetic.wasm");
    let bytes = b"synthetic pinned module";
    fs::write(&path, bytes).unwrap();
    let cache = cache_for(bytes);
    let first = cache.load_if_present(&path).unwrap().unwrap();
    for _ in 0..100 {
        let next = cache.load_if_present(&path).unwrap().unwrap();
        assert!(Arc::ptr_eq(&first, &next));
        assert_eq!(next.bytes(), bytes);
    }
    assert_eq!(cache.statistics(), (1, 1, 100));
}

#[test]
fn changed_or_missing_assets_invalidate_without_changing_in_flight_bytes_or_negative_caching() {
    let directory = tempfile::tempdir().unwrap(); let path = directory.path().join("synthetic.wasm");
    let bytes = b"synthetic pinned module";
    fs::write(&path, bytes).unwrap();
    let cache = cache_for(bytes);
    let old = cache.load_if_present(&path).unwrap().unwrap();
    fs::write(&path, b"different bytes and length").unwrap();
    assert_eq!(cache.load_if_present(&path).err().unwrap().code, "signing_asset_integrity_failed");
    assert_eq!(old.bytes(), bytes);
    fs::remove_file(&path).unwrap();
    assert!(cache.load_if_present(&path).unwrap().is_none());
    fs::write(&path, bytes).unwrap();
    let repaired = cache.load_if_present(&path).unwrap().unwrap();
    assert!(!Arc::ptr_eq(&old, &repaired));
    assert_eq!(cache.statistics().0, 3);
    assert_eq!(cache.statistics().1, 3);
}

#[cfg(unix)]
#[test]
fn a_replaced_file_with_the_same_length_and_bytes_is_revalidated_by_identity() {
    let directory = tempfile::tempdir().unwrap(); let path = directory.path().join("synthetic.wasm");
    let replacement = directory.path().join("replacement.wasm");
    let bytes = b"synthetic pinned module";
    fs::write(&path, bytes).unwrap(); fs::write(&replacement, bytes).unwrap();
    let cache = cache_for(bytes);
    let first = cache.load_if_present(&path).unwrap().unwrap();
    fs::rename(&replacement, &path).unwrap();
    let second = cache.load_if_present(&path).unwrap().unwrap();
    assert!(!Arc::ptr_eq(&first, &second));
    assert_eq!(cache.statistics(), (2, 2, 0));
}

#[cfg(unix)]
#[test]
fn a_symlink_is_not_accepted_as_a_warm_asset_path() {
    let directory = tempfile::tempdir().unwrap(); let path = directory.path().join("synthetic.wasm");
    let other = directory.path().join("other.wasm"); let bytes = b"synthetic pinned module";
    fs::write(&path, bytes).unwrap(); fs::write(&other, bytes).unwrap();
    let cache = cache_for(bytes); cache.load_if_present(&path).unwrap().unwrap();
    fs::remove_file(&path).unwrap(); std::os::unix::fs::symlink(&other, &path).unwrap();
    assert_eq!(cache.load_if_present(&path).err().unwrap().code, "signing_asset_integrity_failed");
}

#[test]
fn cancelled_asset_loading_does_not_read_or_hash_before_its_first_allocation() {
    let directory = tempfile::tempdir().unwrap(); let path = directory.path().join("synthetic.wasm");
    let bytes = b"synthetic pinned module"; fs::write(&path, bytes).unwrap();
    let cache = cache_for(bytes);
    let error = cache.load_if_present_cancellable(&path, &|| true).err().unwrap();
    assert_eq!(error.kind, super::super::ErrorKind::Cancelled);
    assert_eq!(cache.statistics(), (0, 0, 0));
    cache.load_if_present(&path).unwrap().unwrap();
    assert_eq!(cache.statistics(), (1, 1, 0));
}
