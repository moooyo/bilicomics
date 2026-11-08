use super::*;
use crate::domain::PageDescriptor;
use crate::image_integrity::validate_image;
use sha2::{Digest, Sha256};
use std::io::Cursor;

const ACCOUNT: &str = "store-integrity-fixture";

fn manifest(count: usize) -> ImageIndex {
    ImageIndex {
        episode_id: "episode".to_owned(),
        revision: "revision".to_owned(),
        images: (0..count).map(|index| PageDescriptor {
            id: format!("page-{index}"), index, width: 8, height: 12,
        }).collect(),
        ..Default::default()
    }
}

fn running_task(count: usize) -> DownloadRecord {
    DownloadRecord {
        id: "task".to_owned(), comic_id: "comic".to_owned(),
        episode_id: "episode".to_owned(), revision: "revision".to_owned(),
        status: DownloadStatus::Running, total_pages: count as u32, generation: 1,
        ..Default::default()
    }
}

fn metadata_receipt() -> PageReceipt {
    let sha256 = "a".repeat(64);
    PageReceipt {
        episode_id: "episode".to_owned(), revision: "revision".to_owned(),
        page_id: "page-0".to_owned(), file_name: format!("{sha256}.png"), sha256,
        bytes: 42, width: 8, height: 12, verified_at: 20,
        ..Default::default()
    }
}

fn publish_metadata(store: &mut Store, receipt: &PageReceipt) -> Result<()> {
    let generation = store.load_download(ACCOUNT, "task")?.context("The fixture task is missing")?.generation;
    store.begin_page_commit_generation(ACCOUNT, "task", generation, receipt)?;
    store.commit_page_generation(ACCOUNT, "task", generation, receipt)
}

fn encoded_page(shade: u8) -> Result<Vec<u8>> {
    let image = image::DynamicImage::ImageLuma8(image::ImageBuffer::from_pixel(8, 12, image::Luma([shade])));
    let mut encoded = Cursor::new(Vec::new());
    image.write_to(&mut encoded, image::ImageFormat::Png)?;
    Ok(encoded.into_inner())
}

fn receipt_for(encoded: &[u8], page_index: usize) -> PageReceipt {
    let sha256 = format!("{:x}", Sha256::digest(encoded));
    PageReceipt {
        episode_id: "episode".to_owned(), revision: "revision".to_owned(),
        page_id: format!("page-{page_index}"), page_index,
        file_name: format!("{sha256}.png"), sha256, bytes: encoded.len() as u64,
        width: 8, height: 12, verified_at: 20,
        ..Default::default()
    }
}

fn verified_receipt_for(encoded: &[u8], page_index: usize) -> Result<(PageReceipt, DecodedImageProof)> {
    let proof = validate_image(encoded, None)?;
    let receipt = PageReceipt {
        decode_validation_version: proof.validation_version(),
        ..receipt_for(encoded, page_index)
    };
    Ok((receipt, proof))
}

fn publish_verified(store: &mut Store, receipt: &PageReceipt, proof: &DecodedImageProof) -> Result<()> {
    let generation = store.load_download(ACCOUNT, "task")?.context("The fixture task is missing")?.generation;
    store.begin_verified_page_commit_generation(ACCOUNT, "task", generation, receipt, proof)?;
    store.commit_verified_page_generation(ACCOUNT, "task", generation, receipt, proof)
}

fn history(store: &Store, page_id: &str) -> Result<Option<(String, i64, i64)>> {
    Ok(store.connection.query_row(
        "SELECT sha256,bytes,decode_validation_version FROM page_identity_history
             WHERE account_key=?1 AND episode_id='episode' AND revision='revision' AND page_id=?2",
        params![ACCOUNT,page_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?)
}

fn corrupt_encoded_page() -> Vec<u8> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
            }
        }
        !crc
    }
    fn append_chunk(encoded: &mut Vec<u8>, kind: &[u8; 4], bytes: &[u8]) {
        encoded.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        let start = encoded.len();
        encoded.extend_from_slice(kind);
        encoded.extend_from_slice(bytes);
        let checksum = crc32(&encoded[start..]);
        encoded.extend_from_slice(&checksum.to_be_bytes());
    }
    let mut encoded = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&8_u32.to_be_bytes());
    header.extend_from_slice(&12_u32.to_be_bytes());
    header.extend_from_slice(&[8, 0, 0, 0, 0]);
    append_chunk(&mut encoded, b"IHDR", &header);
    // The container and CRCs are intact, but the deflate block type is forbidden.
    append_chunk(&mut encoded, b"IDAT", &[0x78, 0x9c, 0x07, 0, 0, 0, 0, 0]);
    append_chunk(&mut encoded, b"IEND", &[]);
    encoded
}

#[test]
fn completion_requires_explicit_confirmation_of_every_strict_retained_page() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut store = Store::open(directory.path().join("metadata.sqlite3"))?;
    store.save_index(ACCOUNT, &manifest(2))?;
    store.save_download(ACCOUNT, &DownloadRecord { retained: true, ..running_task(2) })?;
    let first_bytes = encoded_page(31)?;
    let final_bytes = encoded_page(67)?;
    let first = PageReceipt { retained: true, ..receipt_for(&first_bytes, 0) };
    let final_page = receipt_for(&final_bytes, 1);
    publish_metadata(&mut store, &first)?;
    publish_metadata(&mut store, &final_page)?;
    let mut completed = store.load_download(ACCOUNT, "task")?.unwrap();
    assert_eq!(completed.status, DownloadStatus::Running);
    assert_eq!((completed.completed_pages, completed.total_pages), (2, 2));
    completed.status = DownloadStatus::Completed;
    assert!(store.save_download(ACCOUNT, &completed).is_err());
    store.promote_page_validation(ACCOUNT, &first, &validate_image(&first_bytes, None)?, 30)?.unwrap();
    assert!(store.save_download(ACCOUNT, &completed).is_err());
    store.promote_page_validation(ACCOUNT, &final_page, &validate_image(&final_bytes, None)?, 30)?.unwrap();
    assert!(store.save_download(ACCOUNT, &completed).is_err());
    assert_eq!(store.load_download(ACCOUNT, "task")?.unwrap().status, DownloadStatus::Running);
    store.retain_page(ACCOUNT, "episode", "revision", "page-1")?;
    store.save_download(ACCOUNT, &completed)?;
    assert_eq!(store.load_download(ACCOUNT, "task")?.unwrap().status, DownloadStatus::Completed);
    store.invalidate_page(ACCOUNT, "episode", "revision", "page-1")?;
    assert!(store.save_download(ACCOUNT, &completed).is_err());
    let invalidated = store.load_download(ACCOUNT, "task")?.unwrap();
    assert_eq!(invalidated.status, DownloadStatus::Failed);
    assert_eq!(invalidated.completed_pages, 1);
    Ok(())
}

#[test]
fn caller_supplied_versions_cannot_replace_an_opaque_decode_proof() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut store = Store::open(directory.path().join("metadata.sqlite3"))?;
    store.save_index(ACCOUNT, &manifest(1))?;
    store.save_download(ACCOUNT, &running_task(1))?;
    let (receipt, proof) = verified_receipt_for(&encoded_page(89)?, 0)?;
    assert!(store.begin_page_commit_generation(ACCOUNT, "task", 1, &receipt).is_err());
    assert!(store.pending_page_commits(ACCOUNT)?.is_empty());
    let different_proof = validate_image(&encoded_page(90)?, None)?;
    assert!(store.begin_verified_page_commit_generation(ACCOUNT, "task", 1, &receipt, &different_proof).is_err());
    store.begin_verified_page_commit_generation(ACCOUNT, "task", 1, &receipt, &proof)?;
    assert!(store.commit_page_generation(ACCOUNT, "task", 1, &receipt).is_err());
    assert!(store.commit_verified_page_generation(ACCOUNT, "task", 1, &receipt, &different_proof).is_err());
    assert!(store.page_receipt(ACCOUNT, "episode", "revision", "page-0")?.is_none());
    store.commit_verified_page_generation(ACCOUNT, "task", 1, &receipt, &proof)?;
    assert_eq!(store.page_receipt(ACCOUNT, "episode", "revision", "page-0")?.unwrap().decode_validation_version, VALIDATION_VERSION);
    assert_eq!(store.load_download(ACCOUNT, "task")?.unwrap().status, DownloadStatus::Running);
    Ok(())
}

#[test]
fn a_legacy_proof_upgrade_preserves_a_concurrent_retain_and_survives_reopen() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("metadata.sqlite3");
    let mut store = Store::open(&database)?;
    store.save_index(ACCOUNT, &manifest(1))?;
    store.save_download(ACCOUNT, &running_task(1))?;
    let encoded = encoded_page(101)?;
    let receipt = receipt_for(&encoded, 0);
    publish_metadata(&mut store, &receipt)?;
    Store::open(&database)?.retain_page(ACCOUNT, "episode", "revision", "page-0")?;
    let promoted = store.promote_page_validation(ACCOUNT, &receipt, &validate_image(&encoded, None)?, 40)?.unwrap();
    assert!(promoted.retained);
    assert_eq!(promoted.decode_validation_version, VALIDATION_VERSION);
    assert_eq!(history(&store, "page-0")?, Some((receipt.sha256.clone(), receipt.bytes as i64, VALIDATION_VERSION as i64)));
    let reopened = Store::open(&database)?;
    assert_eq!(reopened.page_receipt(ACCOUNT, "episode", "revision", "page-0")?, Some(promoted));
    Ok(())
}

#[test]
fn a_proof_upgrade_cannot_resurrect_an_invalidated_receipt() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut store = Store::open(directory.path().join("metadata.sqlite3"))?;
    store.save_index(ACCOUNT, &manifest(1))?;
    store.save_download(ACCOUNT, &running_task(1))?;
    let encoded = encoded_page(109)?;
    let receipt = receipt_for(&encoded, 0);
    publish_metadata(&mut store, &receipt)?;
    store.invalidate_page(ACCOUNT, "episode", "revision", "page-0")?;
    assert!(store.promote_page_validation(ACCOUNT, &receipt, &validate_image(&encoded, None)?, 40)?.is_none());
    assert_eq!(history(&store, "page-0")?, Some((receipt.sha256, receipt.bytes as i64, 0)));
    assert!(store.page_receipt(ACCOUNT, "episode", "revision", "page-0")?.is_none());
    Ok(())
}

#[test]
fn exact_legacy_bad_encoding_can_be_replaced_after_its_untrusted_identity_is_revoked() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut store = Store::open(directory.path().join("metadata.sqlite3"))?;
    store.save_index(ACCOUNT, &manifest(1))?;
    store.save_download(ACCOUNT, &running_task(1))?;
    let encoded = corrupt_encoded_page();
    let original = receipt_for(&encoded, 0);
    publish_metadata(&mut store, &original)?;
    let error = validate_image(&encoded, None).unwrap_err();
    let evidence = error.encoding_evidence().context("The fixture must have invalid image encoding")?;
    assert_eq!(evidence.sha256(), original.sha256);
    assert_eq!(evidence.byte_len(), original.bytes);
    let (corrected, proof) = verified_receipt_for(&encoded_page(127)?, 0)?;
    assert!(publish_verified(&mut store, &corrected, &proof).is_err());
    assert!(store.reject_legacy_invalid_page(ACCOUNT, &original, evidence)?);
    assert!(store.page_receipt(ACCOUNT, "episode", "revision", "page-0")?.is_none());
    assert!(history(&store, "page-0")?.is_none());
    assert!(store.pending_page_commits(ACCOUNT)?.is_empty());
    assert_eq!(store.load_download(ACCOUNT, "task")?.unwrap().completed_pages, 0);
    publish_verified(&mut store, &corrected, &proof)?;
    assert_eq!(history(&store, "page-0")?, Some((corrected.sha256.clone(), corrected.bytes as i64, VALIDATION_VERSION as i64)));
    assert!(!store.reject_legacy_invalid_page(ACCOUNT, &original, evidence)?);
    assert_eq!(store.page_receipt(ACCOUNT, "episode", "revision", "page-0")?, Some(corrected));
    Ok(())
}

#[test]
fn different_or_missing_disk_bytes_do_not_revoke_legacy_or_strict_content_history() -> Result<()> {
    for strict in [false, true] {
        let directory = tempfile::tempdir()?;
        let mut store = Store::open(directory.path().join("metadata.sqlite3"))?;
        store.save_index(ACCOUNT, &manifest(1))?;
        store.save_download(ACCOUNT, &running_task(1))?;
        let original_bytes = encoded_page(149)?;
        let (strict_original, proof) = verified_receipt_for(&original_bytes, 0)?;
        let original = if strict { strict_original } else { receipt_for(&original_bytes, 0) };
        if strict { publish_verified(&mut store, &original, &proof)?; }
        else { publish_metadata(&mut store, &original)?; }
        let previous_history = history(&store, "page-0")?;
        let damaged = corrupt_encoded_page();
        let error = validate_image(&damaged, None).unwrap_err();
        let evidence = error.encoding_evidence().context("The disk damage fixture must be corrupt")?;
        assert!(!store.reject_legacy_invalid_page(ACCOUNT, &original, evidence)?);
        assert_eq!(history(&store, "page-0")?, previous_history);
        store.invalidate_page(ACCOUNT, "episode", "revision", "page-0")?;
        assert_eq!(history(&store, "page-0")?, previous_history);
        let (different, different_proof) = verified_receipt_for(&encoded_page(150)?, 0)?;
        assert!(publish_verified(&mut store, &different, &different_proof).is_err());
        assert_eq!(history(&store, "page-0")?, previous_history);
        assert!(store.page_receipt(ACCOUNT, "episode", "revision", "page-0")?.is_none());
        publish_verified(&mut store, &PageReceipt { decode_validation_version: proof.validation_version(), ..original }, &proof)?;
    }
    Ok(())
}

#[test]
fn a_legacy_replay_of_trusted_bytes_cannot_downgrade_the_historical_proof() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut store = Store::open(directory.path().join("metadata.sqlite3"))?;
    store.save_index(ACCOUNT, &manifest(1))?;
    store.save_download(ACCOUNT, &running_task(1))?;
    let encoded = encoded_page(173)?;
    let (strict, proof) = verified_receipt_for(&encoded, 0)?;
    publish_verified(&mut store, &strict, &proof)?;
    store.invalidate_page(ACCOUNT, "episode", "revision", "page-0")?;
    publish_metadata(&mut store, &receipt_for(&encoded, 0))?;
    let restored = store.page_receipt(ACCOUNT, "episode", "revision", "page-0")?.unwrap();
    assert_eq!(restored.decode_validation_version, VALIDATION_VERSION);
    assert_eq!(history(&store, "page-0")?.unwrap().2, VALIDATION_VERSION as i64);
    Ok(())
}

#[test]
fn the_same_revision_cannot_shrink_or_change_any_page_descriptor() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path().join("metadata.sqlite3"))?;
    let original = manifest(45);
    store.save_index(ACCOUNT, &original)?;

    let mut shortened = original.clone();
    shortened.images.truncate(40);
    assert!(store.save_index(ACCOUNT, &shortened).is_err());
    let mut renamed = original.clone();
    renamed.images[44].id = "different-final-page".to_owned();
    assert!(store.save_index(ACCOUNT, &renamed).is_err());
    let mut resized = original.clone();
    resized.images[44].height += 1;
    assert!(store.save_index(ACCOUNT, &resized).is_err());
    let mut reordered = original.clone();
    reordered.images.swap(0, 1);
    reordered.images[0].index = 0;
    reordered.images[1].index = 1;
    assert!(store.save_index(ACCOUNT, &reordered).is_err());
    assert_eq!(store.load_index(ACCOUNT, "episode", Some("revision"))?.unwrap().images, original.images);

    let mut hydrated = original.clone();
    hydrated.paths = vec!["https://temporary.invalid/new-authorization".to_owned(); 45];
    hydrated.extra = serde_json::json!({"fixture_transport_token": "transient"});
    store.save_index(ACCOUNT, &hydrated)?;
    let stored = store.load_index(ACCOUNT, "episode", Some("revision"))?.unwrap();
    assert_eq!(stored.images, original.images);
    assert!(stored.paths.is_empty());
    assert!(stored.extra.is_null());
    store.save_index("another-account", &shortened)?;
    shortened.revision = "new-revision".to_owned();
    store.save_index(ACCOUNT, &shortened)?;
    Ok(())
}

fn downgrade_fixture_schema(database: &Path, version: i64) -> Result<()> {
    let connection = Connection::open(database)?;
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         ALTER TABLE page_identity_history RENAME TO strict_history;
         CREATE TABLE page_identity_history (
             account_key TEXT NOT NULL, episode_id TEXT NOT NULL, revision TEXT NOT NULL,
             page_id TEXT NOT NULL, sha256 TEXT NOT NULL, bytes INTEGER NOT NULL,
             PRIMARY KEY(account_key,episode_id,revision,page_id));
         INSERT INTO page_identity_history(account_key,episode_id,revision,page_id,sha256,bytes)
             SELECT account_key,episode_id,revision,page_id,sha256,bytes FROM strict_history;
         DROP TABLE strict_history;
         UPDATE page_receipts SET payload=json_remove(payload,'$.decode_validation_version');
         COMMIT;"
    )?;
    if version == 1 {
        connection.execute_batch("DROP TABLE page_identity_history;")?;
    }
    connection.pragma_update(None, "user_version", version)?;
    Ok(())
}

#[test]
fn schema_one_and_two_receipts_migrate_as_unproven_images() -> Result<()> {
    for version in [1, 2] {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("metadata.sqlite3");
        let mut store = Store::open(&database)?;
        store.save_index(ACCOUNT, &manifest(1))?;
        store.save_download(ACCOUNT, &running_task(1))?;
        publish_metadata(&mut store, &metadata_receipt())?;
        drop(store);
        downgrade_fixture_schema(&database, version)?;

        let migrated = Store::open(&database)?;
        assert_eq!(migrated.connection.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?, SCHEMA_VERSION);
        let receipt = migrated.page_receipt(ACCOUNT, "episode", "revision", "page-0")?.unwrap();
        assert_eq!(receipt.decode_validation_version, 0);
        let proof: i64 = migrated.connection.query_row(
            "SELECT decode_validation_version FROM page_identity_history WHERE account_key=?1", [ACCOUNT], |row| row.get(0))?;
        assert_eq!(proof, 0);
        let task = migrated.load_download(ACCOUNT, "task")?.unwrap();
        assert!(migrated.save_download(ACCOUNT, &DownloadRecord { status: DownloadStatus::Completed, ..task }).is_err());
    }
    Ok(())
}

#[test]
fn concurrent_opens_migrate_each_legacy_schema_only_once() -> Result<()> {
    for version in [1, 2] {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("metadata.sqlite3");
        let mut store = Store::open(&database)?;
        store.save_index(ACCOUNT, &manifest(1))?;
        store.save_download(ACCOUNT, &running_task(1))?;
        publish_metadata(&mut store, &metadata_receipt())?;
        drop(store);
        downgrade_fixture_schema(&database, version)?;

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let workers: Vec<_> = (0..4).map(|_| {
            let database = database.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || -> Result<()> {
                barrier.wait();
                let store = Store::open(database)?;
                let version: i64 = store.connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
                ensure!(version == SCHEMA_VERSION, "A concurrent migration did not reach the current schema");
                ensure!(store.page_receipt(ACCOUNT, "episode", "revision", "page-0")?.is_some(), "Migration lost the legacy receipt");
                Ok(())
            })
        }).collect();
        for worker in workers {
            worker.join().map_err(|_| anyhow::anyhow!("A concurrent migration worker panicked"))??;
        }
    }
    Ok(())
}

#[test]
fn a_pending_source_refresh_is_durable_and_cannot_complete_from_its_old_cache() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("metadata.sqlite3");
    let mut store = Store::open(&database)?;
    store.save_index(ACCOUNT, &manifest(1))?;
    store.save_download(ACCOUNT, &DownloadRecord { retained: true, ..running_task(1) })?;
    let (page, proof) = verified_receipt_for(&encoded_page(181)?, 0)?;
    let page = PageReceipt { retained: true, ..page };
    publish_verified(&mut store, &page, &proof)?;
    store.begin_verified_page_commit_generation(ACCOUNT, "task", 1, &page, &proof)?;
    let completed = DownloadRecord {
        status: DownloadStatus::Completed,
        ..store.load_download(ACCOUNT, "task")?.unwrap()
    };
    store.save_download(ACCOUNT, &completed)?;
    let mut legacy_json = serde_json::to_value(&completed)?;
    legacy_json.as_object_mut().unwrap().remove("refresh_source");
    assert!(!serde_json::from_value::<DownloadRecord>(legacy_json)?.refresh_source);

    let mut refresh = DownloadRecord { status: DownloadStatus::Queued, refresh_source: true, ..completed };
    assert!(store.save_download(ACCOUNT, &refresh).is_err());
    refresh.generation += 1;
    store.save_download(ACCOUNT, &refresh)?;
    assert!(store.pending_page_commits(ACCOUNT)?.is_empty());
    drop(store);
    let mut reopened = Store::open(&database)?;
    let pending = reopened.load_download(ACCOUNT, "task")?.unwrap();
    assert!(pending.refresh_source);
    assert_eq!(pending.status, DownloadStatus::Queued);
    assert!(reopened.save_download(ACCOUNT, &DownloadRecord {
        status: DownloadStatus::Completed, ..pending.clone()
    }).is_err());
    assert!(reopened.save_download(ACCOUNT, &DownloadRecord {
        status: DownloadStatus::Completed, refresh_source: false, ..pending.clone()
    }).is_err());
    assert!(reopened.save_download(ACCOUNT, &DownloadRecord {
        status: DownloadStatus::Paused, refresh_source: false, ..pending.clone()
    }).is_err());
    let running = DownloadRecord { status: DownloadStatus::Running, ..pending };
    reopened.save_download(ACCOUNT, &running)?;
    assert!(reopened.begin_verified_page_commit_generation(ACCOUNT, "task", running.generation, &page, &proof).is_err());
    assert!(reopened.commit_verified_page_generation(ACCOUNT, "task", running.generation, &page, &proof).is_err());
    reopened.save_download(ACCOUNT, &DownloadRecord { status: DownloadStatus::Failed, ..running })?;
    assert!(Store::open(&database)?.load_download(ACCOUNT, "task")?.unwrap().refresh_source);
    assert_eq!(reopened.page_receipt(ACCOUNT, "episode", "revision", "page-0")?, Some(page.clone()));
    assert_eq!(history(&reopened, "page-0")?, Some((page.sha256, page.bytes as i64, VALIDATION_VERSION as i64)));
    Ok(())
}

#[test]
fn only_a_checked_refresh_can_bind_a_new_revision_and_old_revision_content_remains_intact() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut store = Store::open(directory.path().join("metadata.sqlite3"))?;
    let old_index = manifest(1);
    store.save_index(ACCOUNT, &old_index)?;
    store.save_download(ACCOUNT, &DownloadRecord { retained: true, ..running_task(1) })?;
    let (page, proof) = verified_receipt_for(&encoded_page(191)?, 0)?;
    let page = PageReceipt { retained: true, ..page };
    publish_verified(&mut store, &page, &proof)?;
    let original = store.load_download(ACCOUNT, "task")?.unwrap();
    let mut target = manifest(2);
    target.revision = "expanded-revision".to_owned();
    let mut checked = DownloadRecord {
        revision: target.revision.clone(), generation: original.generation + 1,
        total_pages: 2, completed_pages: 0, bytes: 0,
        ..original.clone()
    };
    assert!(store.save_download(ACCOUNT, &checked).is_err());
    let refresh = DownloadRecord {
        status: DownloadStatus::Queued, refresh_source: true, generation: checked.generation,
        ..original
    };
    store.save_download(ACCOUNT, &refresh)?;
    assert!(store.save_download(ACCOUNT, &checked).is_err());
    store.save_index(ACCOUNT, &target)?;
    assert!(store.save_download(ACCOUNT, &DownloadRecord {
        total_pages: refresh.total_pages, completed_pages: refresh.completed_pages, bytes: refresh.bytes,
        ..checked.clone()
    }).is_err());
    assert!(store.save_download(ACCOUNT, &DownloadRecord {
        generation: checked.generation + 1, ..checked.clone()
    }).is_err());
    checked.status = DownloadStatus::Running;
    store.save_download(ACCOUNT, &checked)?;
    let rebound = store.load_download(ACCOUNT, "task")?.unwrap();
    assert_eq!(rebound.revision, target.revision);
    assert!(!rebound.refresh_source);
    assert_eq!((rebound.total_pages, rebound.completed_pages, rebound.bytes), (2, 0, 0));
    assert_eq!(store.load_index(ACCOUNT, "episode", Some("revision"))?.unwrap().images, old_index.images);
    assert_eq!(store.page_receipt(ACCOUNT, "episode", "revision", "page-0")?, Some(page.clone()));
    assert_eq!(history(&store, "page-0")?, Some((page.sha256, page.bytes as i64, VALIDATION_VERSION as i64)));
    assert!(store.page_receipts(ACCOUNT, "episode", &target.revision)?.is_empty());
    assert_eq!(store.load_index(ACCOUNT, "episode", Some(&target.revision))?.unwrap().images, target.images);
    Ok(())
}
