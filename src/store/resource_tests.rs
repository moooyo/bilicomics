use super::*;
use crate::{domain::PageDescriptor, image_integrity::validate_image};
use std::io::Cursor;

const ACCOUNT: &str = "store-resource-fixture";
const EPISODE: &str = "episode";
const REVISION: &str = "revision";

#[test]
fn global_download_activity_counts_retained_comic_groups_and_accounts() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path().join("state.sqlite3"))?;
    let failed = DownloadRecord { status: DownloadStatus::Failed, retained: true,
        ..task("failed-first", REVISION, 0) };
    store.save_download(ACCOUNT, &failed)?;
    store.save_download(ACCOUNT, &DownloadRecord { id: "failed-second".into(), ..failed.clone() })?;
    store.save_download(ACCOUNT, &DownloadRecord { id: "failed-other-comic".into(), comic_id: "second-comic".into(), ..failed.clone() })?;
    store.save_download(ACCOUNT, &DownloadRecord { id: "prefetch-error".into(), comic_id: "prefetch-only".into(), retained: false, ..failed.clone() })?;
    store.save_download(ACCOUNT, &DownloadRecord { id: "cancelled".into(), comic_id: "cancelled-comic".into(), status: DownloadStatus::Cancelled, ..failed.clone() })?;
    store.save_download("other-account", &DownloadRecord { id: "foreign".into(), ..failed })?;
    assert_eq!(store.download_activity(ACCOUNT)?, (false, 2));
    store.save_download(ACCOUNT, &task("running-prefetch", REVISION, 0))?;
    assert_eq!(store.download_activity(ACCOUNT)?, (true, 2));
    store.update_download_state(ACCOUNT, "failed-first", DownloadStatus::Cancelled, None, 1)?;
    store.update_download_state(ACCOUNT, "failed-second", DownloadStatus::Cancelled, None, 1)?;
    assert_eq!(store.download_activity(ACCOUNT)?, (true, 1));
    assert_eq!(store.download_activity("other-account")?, (false, 1));
    assert_eq!(store.download_activity("empty-account")?, (false, 0));
    Ok(())
}

fn manifest(revision: &str, count: usize) -> ImageIndex {
    ImageIndex { episode_id: EPISODE.to_owned(), revision: revision.to_owned(),
        images: (0..count).map(|index| PageDescriptor {
            id: format!("page-{index}"), index, width: 2, height: 3,
        }).collect(), ..Default::default() }
}

fn task(id: &str, revision: &str, count: u32) -> DownloadRecord {
    DownloadRecord { id: id.to_owned(), comic_id: "comic".to_owned(), episode_id: EPISODE.to_owned(),
        revision: revision.to_owned(), total_pages: count, generation: 1,
        status: DownloadStatus::Running, ..Default::default() }
}

fn image_bytes(shade: u8) -> Result<Vec<u8>> {
    let image = image::DynamicImage::ImageLuma8(image::ImageBuffer::from_pixel(2, 3, image::Luma([shade])));
    let mut encoded = Cursor::new(Vec::new());
    image.write_to(&mut encoded, image::ImageFormat::Png)?;
    Ok(encoded.into_inner())
}

fn page(revision: &str, page_index: usize, encoded: &[u8], retained: bool) -> Result<(PageReceipt, DecodedImageProof)> {
    let proof = validate_image(encoded, None)?;
    let receipt = PageReceipt {
        episode_id: EPISODE.to_owned(), revision: revision.to_owned(), page_id: format!("page-{page_index}"), page_index,
        file_name: format!("{}.png", proof.sha256()), sha256: proof.sha256().to_owned(), bytes: proof.byte_len(),
        width: proof.width(), height: proof.height(), retained, verified_at: 20,
        decode_validation_version: proof.validation_version(),
    };
    Ok((receipt, proof))
}

fn publish(store: &mut Store, account: &str, task_id: &str, receipt: &PageReceipt, proof: &DecodedImageProof) -> Result<()> {
    let generation = store.load_download(account, task_id)?.context("The fixture task is missing")?.generation;
    store.begin_verified_page_commit_generation(account, task_id, generation, receipt, proof)?;
    store.commit_verified_page_generation(account, task_id, generation, receipt, proof)
}

#[test]
fn sql_progress_counts_matching_pages_once_even_when_their_image_bytes_repeat() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut store = Store::open(directory.path().join("metadata.sqlite3"))?;
    store.save_index(ACCOUNT, &manifest(REVISION, 57))?;
    store.save_download(ACCOUNT, &task("reader", REVISION, 57))?;
    store.save_download(ACCOUNT, &DownloadRecord { retained: true, ..task("retained", REVISION, 57) })?;
    let encoded = image_bytes(33)?;
    for (count, position) in [56, 0, 21, 40, 1].into_iter().enumerate() {
        let (receipt, proof) = page(REVISION, position, &encoded, false)?;
        publish(&mut store, ACCOUNT, "reader", &receipt, &proof)?;
        let current = store.load_download(ACCOUNT, "reader")?.unwrap();
        assert_eq!(current.completed_pages as usize, count + 1);
        assert_eq!(current.bytes, encoded.len() as u64 * (count + 1) as u64);
        assert_eq!(current.status, DownloadStatus::Running);
    }
    let (duplicate, proof) = page(REVISION, 0, &encoded, false)?;
    publish(&mut store, ACCOUNT, "reader", &duplicate, &proof)?;
    assert_eq!(store.load_download(ACCOUNT, "reader")?.unwrap().completed_pages, 5);

    let mut orphan = duplicate.clone();
    orphan.page_id = "obsolete-page-at-the-same-position".to_owned();
    orphan.decode_validation_version = 0;
    store.connection.execute(
        "INSERT INTO page_receipts(account_key,episode_id,revision,page_id,page_index,payload) VALUES(?1,?2,?3,?4,0,?5)",
        params![ACCOUNT,EPISODE,REVISION,orphan.page_id,to_json(&orphan)?])?;
    store.connection.execute(
        "INSERT INTO page_identity_history(account_key,episode_id,revision,page_id,sha256,bytes) VALUES(?1,?2,?3,?4,?5,?6)",
        params![ACCOUNT,EPISODE,REVISION,orphan.page_id,orphan.sha256,orphan.bytes as i64])?;
    store.save_index("other-account", &manifest(REVISION, 57))?;
    store.save_download("other-account", &task("other-account-task", REVISION, 57))?;
    let (other, other_proof) = page(REVISION, 7, &encoded, false)?;
    publish(&mut store, "other-account", "other-account-task", &other, &other_proof)?;
    store.save_index(ACCOUNT, &manifest("other-revision", 57))?;
    store.save_download(ACCOUNT, &task("other-revision-task", "other-revision", 57))?;
    let (other, other_proof) = page("other-revision", 7, &encoded, false)?;
    publish(&mut store, ACCOUNT, "other-revision-task", &other, &other_proof)?;

    let (next, proof) = page(REVISION, 2, &encoded, true)?;
    publish(&mut store, ACCOUNT, "retained", &next, &proof)?;
    let retained = store.load_download(ACCOUNT, "retained")?.unwrap();
    assert_eq!((retained.completed_pages, retained.total_pages), (6, 57));
    assert_eq!(retained.bytes, encoded.len() as u64 * 6);
    assert_eq!(store.storage_usage(ACCOUNT)?, encoded.len() as u64);
    assert!(store.save_download(ACCOUNT, &DownloadRecord { status: DownloadStatus::Completed,
        completed_pages: 57, ..retained }).is_err());
    Ok(())
}

#[test]
fn partial_sql_progress_survives_journal_recovery_replay_and_invalidation() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("metadata.sqlite3");
    let mut store = Store::open(&database)?;
    store.save_index(ACCOUNT, &manifest(REVISION, 41))?;
    store.save_download(ACCOUNT, &task("reader", REVISION, 41))?;
    let encoded = image_bytes(55)?;
    for position in [0, 40, 20] {
        let (receipt, proof) = page(REVISION, position, &encoded, false)?;
        publish(&mut store, ACCOUNT, "reader", &receipt, &proof)?;
    }
    let (pending, proof) = page(REVISION, 39, &encoded, false)?;
    store.begin_verified_page_commit_generation(ACCOUNT, "reader", 1, &pending, &proof)?;
    assert_eq!(store.load_download(ACCOUNT, "reader")?.unwrap().completed_pages, 3);
    drop(store);

    let mut reopened = Store::open(&database)?;
    let journal = reopened.pending_page_commits(ACCOUNT)?.pop().context("The pending publication was lost")?;
    assert_eq!(journal.receipt, pending);
    let recovered_proof = validate_image(&encoded, None)?;
    reopened.begin_verified_page_commit_generation(ACCOUNT, "reader", 1, &journal.receipt, &recovered_proof)?;
    reopened.commit_verified_page_generation(ACCOUNT, "reader", 1, &journal.receipt, &recovered_proof)?;
    publish(&mut reopened, ACCOUNT, "reader", &pending, &recovered_proof)?;
    let current = reopened.load_download(ACCOUNT, "reader")?.unwrap();
    assert_eq!((current.completed_pages, current.bytes), (4, encoded.len() as u64 * 4));
    assert!(reopened.pending_page_commits(ACCOUNT)?.is_empty());
    reopened.invalidate_page(ACCOUNT, EPISODE, REVISION, "page-40")?;
    let current = reopened.load_download(ACCOUNT, "reader")?.unwrap();
    assert_eq!((current.completed_pages, current.bytes), (3, encoded.len() as u64 * 3));
    let (missing, proof) = page(REVISION, 40, &encoded, false)?;
    publish(&mut reopened, ACCOUNT, "reader", &missing, &proof)?;
    let current = reopened.load_download(ACCOUNT, "reader")?.unwrap();
    assert_eq!((current.completed_pages, current.bytes), (4, encoded.len() as u64 * 4));
    reopened.save_download(ACCOUNT, &DownloadRecord { status: DownloadStatus::Paused, generation: 2, ..current.clone() })?;
    assert!(reopened.begin_verified_page_commit_generation(ACCOUNT, "reader", 1, &pending, &recovered_proof).is_err());
    assert!(reopened.commit_verified_page_generation(ACCOUNT, "reader", 1, &pending, &recovered_proof).is_err());
    assert_eq!(Store::open(&database)?.load_download(ACCOUNT, "reader")?.unwrap().bytes, current.bytes);
    Ok(())
}

#[test]
fn missing_aggregate_history_rolls_back_the_new_page_but_preserves_its_publication_journal() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut store = Store::open(directory.path().join("metadata.sqlite3"))?;
    store.save_index(ACCOUNT, &manifest(REVISION, 2))?;
    store.save_download(ACCOUNT, &task("reader", REVISION, 2))?;
    let encoded = image_bytes(77)?;
    let (first, proof) = page(REVISION, 0, &encoded, false)?;
    publish(&mut store, ACCOUNT, "reader", &first, &proof)?;
    let before = store.load_download(ACCOUNT, "reader")?.unwrap();
    store.connection.execute(
        "DELETE FROM page_identity_history WHERE account_key=?1 AND episode_id=?2 AND revision=?3 AND page_id='page-0'",
        params![ACCOUNT,EPISODE,REVISION])?;
    let (next, proof) = page(REVISION, 1, &encoded, false)?;
    store.begin_verified_page_commit_generation(ACCOUNT, "reader", 1, &next, &proof)?;
    assert!(store.commit_verified_page_generation(ACCOUNT, "reader", 1, &next, &proof).is_err());
    assert_eq!(store.load_download(ACCOUNT, "reader")?, Some(before));
    assert!(store.page_receipt(ACCOUNT, EPISODE, REVISION, "page-1")?.is_none());
    let history_present: bool = store.connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM page_identity_history WHERE account_key=?1 AND episode_id=?2 AND revision=?3 AND page_id='page-1')",
        params![ACCOUNT,EPISODE,REVISION], |row| row.get(0))?;
    assert!(!history_present);
    assert_eq!(store.pending_page_commits(ACCOUNT)?.len(), 1);
    Ok(())
}

#[test]
fn reader_metadata_gc_preserves_live_retained_and_unresolved_shared_content() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut store = Store::open(directory.path().join("metadata.sqlite3"))?;
    store.save_index(ACCOUNT, &manifest(REVISION, 1))?;
    store.save_download(ACCOUNT, &DownloadRecord { retained: true, ..task("retained", REVISION, 1) })?;
    let (receipt, proof) = page(REVISION, 0, &image_bytes(99)?, true)?;
    publish(&mut store, ACCOUNT, "retained", &receipt, &proof)?;
    let complete = DownloadRecord { status: DownloadStatus::Completed,
        terminal_event: Some(serde_json::json!({"kind":"completed"})),
        ..store.load_download(ACCOUNT, "retained")?.unwrap() };
    store.save_download(ACCOUNT, &complete)?;
    store.record_download_event(ACCOUNT, "retained", 1, &serde_json::json!({"kind":"completed"}))?;
    for (id, status, kind) in [
        ("old-completed", DownloadStatus::Completed, "completed"),
        ("old-paused", DownloadStatus::Paused, "paused"),
        ("old-failed", DownloadStatus::Failed, "failed"),
        ("old-cancelled", DownloadStatus::Cancelled, "cancelled"),
    ] {
        let payload = if kind == "failed" { serde_json::json!({"kind":kind,"code":"fixture_failure","retryable":false}) }
            else { serde_json::json!({"kind":kind}) };
        store.save_download(ACCOUNT, &DownloadRecord { id: id.to_owned(), retained: false, status,
            terminal_event: Some(payload.clone()), ..complete.clone() })?;
        store.record_download_event(ACCOUNT, id, 1, &payload)?;
    }
    for (id, status) in [("live-paused",DownloadStatus::Paused),("live-running",DownloadStatus::Running),("live-queued",DownloadStatus::Queued)] {
        store.save_download(ACCOUNT, &DownloadRecord { id: id.to_owned(), retained: false, status,
            terminal_event: (status == DownloadStatus::Paused).then(|| serde_json::json!({"kind":"prefetch_ready"})),
            ..complete.clone() })?;
    }
    store.record_download_event(ACCOUNT, "live-paused", 1, &serde_json::json!({"kind":"prefetch_ready"}))?;
    let mut unresolved = DownloadRecord { id: "unresolved-reader".to_owned(), retained: false,
        status: DownloadStatus::Running, terminal_event: None, ..complete.clone() };
    store.save_download(ACCOUNT, &unresolved)?;
    store.begin_verified_page_commit_generation(ACCOUNT, &unresolved.id, 1, &receipt, &proof)?;
    unresolved.status = DownloadStatus::Failed;
    unresolved.generation = 2;
    unresolved.terminal_event = Some(serde_json::json!({"kind":"failed","code":"image_encoding_unsupported","retryable":false}));
    store.save_download(ACCOUNT, &unresolved)?;
    store.record_download_event(ACCOUNT, &unresolved.id, 2,
        &serde_json::json!({"kind":"failed","code":"image_encoding_unsupported","retryable":false}))?;
    store.save_download("other-account", &DownloadRecord { retained: false, status: DownloadStatus::Cancelled,
        terminal_event: None, ..complete.clone() })?;
    let receipts_before = store.page_receipts(ACCOUNT, EPISODE, REVISION)?;
    let history_before: (String, i64, i64) = store.connection.query_row(
        "SELECT sha256,bytes,decode_validation_version FROM page_identity_history WHERE account_key=?1", [ACCOUNT],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
    assert_eq!(store.prune_reader_downloads(ACCOUNT, &["live-paused".to_owned()])?, 4);
    assert_eq!(store.prune_reader_downloads(ACCOUNT, &["live-paused".to_owned()])?, 0);
    for id in ["old-completed","old-paused","old-failed","old-cancelled"] {
        assert!(store.load_download(ACCOUNT, id)?.is_none());
    }
    for id in ["retained","live-paused","live-running","live-queued","unresolved-reader"] {
        assert!(store.load_download(ACCOUNT, id)?.is_some());
    }
    assert!(store.load_download("other-account", "retained")?.is_some());
    assert_eq!(store.page_receipts(ACCOUNT, EPISODE, REVISION)?, receipts_before);
    let history_after: (String, i64, i64) = store.connection.query_row(
        "SELECT sha256,bytes,decode_validation_version FROM page_identity_history WHERE account_key=?1", [ACCOUNT],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
    assert_eq!(history_after, history_before);
    assert_eq!(store.pending_page_commits(ACCOUNT)?.len(), 1);
    assert!(store.load_index(ACCOUNT, EPISODE, Some(REVISION))?.is_some());
    assert!(store.page_object_referenced(ACCOUNT, &receipt.file_name)?);
    let events = store.pending_download_events(ACCOUNT, usize::MAX)?;
    assert_eq!(events.len(), 3);
    assert!(events.iter().all(|event| ["retained","live-paused","unresolved-reader"].contains(&event.task_id.as_str())));
    Ok(())
}

#[test]
fn terminal_outbox_replacement_acknowledgement_and_generation_are_account_scoped() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("metadata.sqlite3");
    let store = Store::open(&database)?;
    let paused = DownloadRecord { status: DownloadStatus::Paused,
        terminal_event: Some(serde_json::json!({"kind":"paused"})), ..task("reader", REVISION, 0) };
    store.save_download(ACCOUNT, &paused)?;
    store.save_download("other-account", &paused)?;
    store.record_download_event(ACCOUNT, "reader", 1, &serde_json::json!({"kind":"paused"}))?;
    let original = store.pending_download_events(ACCOUNT, 1)?.pop().unwrap();
    store.record_download_event(ACCOUNT, "reader", 1, &serde_json::json!({"kind":"paused"}))?;
    assert_eq!(store.pending_download_events(ACCOUNT, 1)?, vec![original.clone()]);
    store.record_download_event("other-account", "reader", 1, &serde_json::json!({"kind":"paused"}))?;
    store.save_download(ACCOUNT, &DownloadRecord {
        terminal_event: Some(serde_json::json!({"kind":"prefetch_ready"})), ..paused.clone()
    })?;
    store.record_download_event(ACCOUNT, "reader", 1, &serde_json::json!({"kind":"prefetch_ready"}))?;
    let replacement = Store::open(&database)?.pending_download_events(ACCOUNT, 1)?.pop().unwrap();
    assert!(replacement.id > original.id);
    assert_eq!(replacement.payload, serde_json::json!({"kind":"prefetch_ready"}));
    store.acknowledge_download_event(ACCOUNT, original.id)?;
    store.acknowledge_download_event("other-account", replacement.id)?;
    assert_eq!(store.pending_download_events(ACCOUNT, 1)?, vec![replacement.clone()]);
    let failed = DownloadRecord { status: DownloadStatus::Failed, generation: 2,
        error_code: Some("fixture_failure".to_owned()),
        terminal_event: Some(serde_json::json!({"kind":"failed","code":"fixture_failure","retryable":false})), ..paused };
    store.save_download(ACCOUNT, &failed)?;
    assert!(store.pending_download_events(ACCOUNT, 1)?.is_empty());
    assert!(store.record_download_event(ACCOUNT, "reader", 1, &serde_json::json!({"kind":"paused"})).is_err());
    store.record_download_event(ACCOUNT, "reader", 2,
        &serde_json::json!({"kind":"failed","code":"fixture_failure","retryable":false}))?;
    let current = store.pending_download_events(ACCOUNT, 1)?.pop().unwrap();
    assert!(current.id > replacement.id);
    assert_eq!(current.generation, 2);
    store.acknowledge_download_event(ACCOUNT, replacement.id)?;
    assert_eq!(store.pending_download_events(ACCOUNT, 1)?, vec![current.clone()]);
    assert!(store.pending_download_events(ACCOUNT, 0)?.is_empty());
    assert_eq!(store.pending_download_events("other-account", 1)?.len(), 1);
    store.acknowledge_download_event(ACCOUNT, current.id)?;
    assert!(store.pending_download_events(ACCOUNT, 1)?.is_empty());
    Ok(())
}

#[test]
fn invalid_or_failed_outbox_writes_preserve_the_replayable_terminal_intent() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path().join("metadata.sqlite3"))?;
    let paused = DownloadRecord { status: DownloadStatus::Paused,
        terminal_event: Some(serde_json::json!({"kind":"paused"})), ..task("reader", REVISION, 0) };
    store.save_download(ACCOUNT, &paused)?;
    store.record_download_event(ACCOUNT, "reader", 1, &serde_json::json!({"kind":"paused"}))?;
    let original = store.pending_download_events(ACCOUNT, 1)?;
    for payload in [
        serde_json::json!({"kind":"index_ready","index":{"paths":["https://temporary.invalid/signed"]}}),
        serde_json::json!({"kind":"paused","url":"https://temporary.invalid/signed"}),
        serde_json::json!({"kind":"failed","code":"https://temporary.invalid/signed","retryable":false}),
        serde_json::json!({"kind":"failed","code":"fixture_failure","retryable":"false"}),
        serde_json::json!({"kind":"completed"}),
    ] {
        assert!(store.record_download_event(ACCOUNT, "reader", 1, &payload).is_err());
    }
    assert_eq!(store.pending_download_events(ACCOUNT, 1)?, original);
    let pending = DownloadRecord { terminal_event: Some(serde_json::json!({"kind":"prefetch_ready"})), ..paused };
    store.save_download(ACCOUNT, &pending)?;
    store.connection.execute_batch(
        "CREATE TRIGGER reject_outbox_fixture BEFORE INSERT ON download_event_outbox
             BEGIN SELECT RAISE(ABORT,'outbox failure fixture'); END;"
    )?;
    assert!(store.record_download_event(ACCOUNT, "reader", 1, &serde_json::json!({"kind":"prefetch_ready"})).is_err());
    assert!(store.pending_download_events(ACCOUNT, 1)?.is_empty());
    assert_eq!(store.load_download(ACCOUNT, "reader")?, Some(pending.clone()));
    assert_eq!(store.terminal_downloads_page(ACCOUNT, None, 1)?, vec![pending]);
    store.connection.execute_batch("DROP TRIGGER reject_outbox_fixture;")?;
    store.record_download_event(ACCOUNT, "reader", 1, &serde_json::json!({"kind":"prefetch_ready"}))?;
    assert_eq!(store.pending_download_events(ACCOUNT, 1)?.len(), 1);
    Ok(())
}

#[test]
fn schema_three_resource_migration_preserves_content_and_installs_bounded_outbox_indexes() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("metadata.sqlite3");
    let mut store = Store::open(&database)?;
    store.save_index(ACCOUNT, &manifest(REVISION, 1))?;
    store.save_download(ACCOUNT, &task("reader", REVISION, 1))?;
    let (receipt, proof) = page(REVISION, 0, &image_bytes(121)?, false)?;
    publish(&mut store, ACCOUNT, "reader", &receipt, &proof)?;
    let task_before = store.load_download(ACCOUNT, "reader")?;
    store.connection.execute_batch(
        "BEGIN IMMEDIATE;
         DROP INDEX page_objects_by_name;
         DROP INDEX pending_objects_by_name;
         DROP INDEX reader_downloads_terminal;
         DROP INDEX download_terminal_reconcile;
         DROP TABLE download_event_outbox;
         PRAGMA user_version=3;
         COMMIT;"
    )?;
    drop(store);
    let migrated = Store::open(&database)?;
    let version: i64 = migrated.connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(version, SCHEMA_VERSION);
    assert_eq!(migrated.load_download(ACCOUNT, "reader")?, task_before);
    assert_eq!(migrated.page_receipt(ACCOUNT, EPISODE, REVISION, "page-0")?, Some(receipt.clone()));
    assert_eq!(migrated.load_index(ACCOUNT, EPISODE, Some(REVISION))?.unwrap().images, manifest(REVISION, 1).images);
    let indexes: i64 = migrated.connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name IN
             ('page_objects_by_name','pending_objects_by_name','reader_downloads_terminal','download_terminal_reconcile','download_events_order')",
        [], |row| row.get(0))?;
    assert_eq!(indexes, 5);
    let history: (String, i64) = migrated.connection.query_row(
        "SELECT sha256,bytes FROM page_identity_history WHERE account_key=?1 AND page_id='page-0'", [ACCOUNT],
        |row| Ok((row.get(0)?,row.get(1)?)))?;
    assert_eq!(history, (receipt.sha256, receipt.bytes as i64));
    assert!(migrated.pending_download_events(ACCOUNT, 1)?.is_empty());
    assert!(migrated.page_object_referenced(ACCOUNT, &receipt.file_name)?);
    Ok(())
}

#[test]
fn terminal_intents_survive_the_state_to_outbox_gap_and_replay_through_bounded_seek_pages() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("metadata.sqlite3");
    let store = Store::open(&database)?;
    for (id, status, payload) in [
        ("a-prefetch",DownloadStatus::Paused,serde_json::json!({"kind":"prefetch_ready"})),
        ("b-failed",DownloadStatus::Failed,serde_json::json!({"kind":"failed","code":"fixture_failure","retryable":true})),
        ("c-cancelled",DownloadStatus::Cancelled,serde_json::json!({"kind":"cancelled"})),
        ("d-paused",DownloadStatus::Paused,serde_json::json!({"kind":"paused"})),
    ] {
        store.save_download(ACCOUNT, &DownloadRecord { status, terminal_event: Some(payload), ..task(id, REVISION, 0) })?;
    }
    store.save_download(ACCOUNT, &DownloadRecord { status: DownloadStatus::Paused, ..task("e-legacy", REVISION, 0) })?;
    store.save_download(ACCOUNT, &DownloadRecord { status: DownloadStatus::Queued, ..task("aa-active", REVISION, 0) })?;
    store.save_download("other-account", &DownloadRecord { status: DownloadStatus::Paused,
        terminal_event: Some(serde_json::json!({"kind":"paused"})), ..task("a-foreign", REVISION, 0) })?;
    for status in [DownloadStatus::Queued,DownloadStatus::Running] {
        assert!(store.save_download(ACCOUNT, &DownloadRecord { status,
            terminal_event: Some(serde_json::json!({"kind":"paused"})), ..task("invalid-active", REVISION, 0) }).is_err());
    }
    drop(store);

    let reopened = Store::open(&database)?;
    assert!(reopened.pending_download_events(ACCOUNT, 10)?.is_empty());
    assert!(reopened.terminal_downloads_page(ACCOUNT, None, 0)?.is_empty());
    let first = reopened.terminal_downloads_page(ACCOUNT, None, 2)?;
    assert_eq!(first.iter().map(|record| record.id.as_str()).collect::<Vec<_>>(), ["a-prefetch","b-failed"]);
    assert_eq!(first[0].terminal_event, Some(serde_json::json!({"kind":"prefetch_ready"})));
    assert_eq!(first[1].terminal_event.as_ref().unwrap()["retryable"], true);
    let second = reopened.terminal_downloads_page(ACCOUNT, Some(&first[1].id), 2)?;
    assert_eq!(second.iter().map(|record| record.id.as_str()).collect::<Vec<_>>(), ["c-cancelled","d-paused"]);
    assert!(reopened.terminal_downloads_page(ACCOUNT, Some(&second[1].id), 2)?.is_empty());
    for record in first.into_iter().chain(second) {
        reopened.record_download_event(ACCOUNT, &record.id, record.generation, record.terminal_event.as_ref().unwrap())?;
    }
    let events = reopened.pending_download_events(ACCOUNT, usize::MAX)?;
    assert_eq!(events.len(), 4);
    reopened.acknowledge_download_event(ACCOUNT, events[0].id)?;
    assert!(reopened.load_download(ACCOUNT, &events[0].task_id)?.unwrap().terminal_event.is_none());
    assert_eq!(reopened.terminal_downloads_page(ACCOUNT, None, usize::MAX)?.len(), 3);
    assert_eq!(reopened.terminal_downloads_page("other-account", None, 2)?.len(), 1);
    Ok(())
}

#[test]
fn stale_retries_and_acknowledgements_cannot_clear_a_new_terminal_meaning() -> Result<()> {
    for (status, original, changed) in [
        (DownloadStatus::Paused,serde_json::json!({"kind":"paused"}),serde_json::json!({"kind":"prefetch_ready"})),
        (DownloadStatus::Failed,serde_json::json!({"kind":"failed","code":"fixture_failure","retryable":false}),
            serde_json::json!({"kind":"failed","code":"fixture_failure","retryable":true})),
    ] {
        let directory = tempfile::tempdir()?;
        let store = Store::open(directory.path().join("metadata.sqlite3"))?;
        let mut current = DownloadRecord { status, terminal_event: Some(original.clone()), ..task("reader", REVISION, 0) };
        store.save_download(ACCOUNT, &current)?;
        store.record_download_event(ACCOUNT, "reader", 1, &original)?;
        let old = store.pending_download_events(ACCOUNT, 1)?.pop().unwrap();
        current.terminal_event = Some(changed.clone());
        store.save_download(ACCOUNT, &current)?;
        assert!(store.pending_download_events(ACCOUNT, 1)?.is_empty());
        assert!(store.record_download_event(ACCOUNT, "reader", 1, &original).is_err());
        store.acknowledge_download_event(ACCOUNT, old.id)?;
        assert_eq!(store.terminal_downloads_page(ACCOUNT, None, 1)?, vec![current.clone()]);
        store.record_download_event(ACCOUNT, "reader", 1, &changed)?;
        let new = store.pending_download_events(ACCOUNT, 1)?.pop().unwrap();
        assert!(new.id > old.id);
        store.record_download_event(ACCOUNT, "reader", 1, &changed)?;
        assert_eq!(store.pending_download_events(ACCOUNT, 1)?, vec![new.clone()]);
        store.acknowledge_download_event(ACCOUNT, old.id)?;
        assert_eq!(store.pending_download_events(ACCOUNT, 1)?, vec![new.clone()]);
        store.acknowledge_download_event(ACCOUNT, new.id)?;
        assert!(store.load_download(ACCOUNT, "reader")?.unwrap().terminal_event.is_none());
        assert!(store.terminal_downloads_page(ACCOUNT, None, 1)?.is_empty());
    }
    Ok(())
}

#[test]
fn deferred_state_controls_replace_old_terminal_intents_instead_of_reusing_them() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path().join("metadata.sqlite3"))?;
    let failed = DownloadRecord { status: DownloadStatus::Failed, error_code: Some("fixture_failure".to_owned()),
        terminal_event: Some(serde_json::json!({"kind":"failed","code":"fixture_failure","retryable":false})),
        ..task("reader", REVISION, 0) };
    store.save_download(ACCOUNT, &failed)?;
    store.record_download_event(ACCOUNT, "reader", 1, failed.terminal_event.as_ref().unwrap())?;
    store.update_download_state(ACCOUNT, "reader", DownloadStatus::Queued, None, 21)?;
    let queued = store.load_download(ACCOUNT, "reader")?.unwrap();
    assert_eq!(queued.status, DownloadStatus::Queued);
    assert!(queued.terminal_event.is_none());
    assert!(store.pending_download_events(ACCOUNT, 1)?.is_empty());
    store.update_download_state(ACCOUNT, "reader", DownloadStatus::Paused, None, 22)?;
    assert_eq!(store.load_download(ACCOUNT, "reader")?.unwrap().terminal_event, Some(serde_json::json!({"kind":"paused"})));
    store.record_download_event(ACCOUNT, "reader", 1, &serde_json::json!({"kind":"paused"}))?;
    store.update_download_state(ACCOUNT, "reader", DownloadStatus::Cancelled, None, 23)?;
    assert_eq!(store.load_download(ACCOUNT, "reader")?.unwrap().terminal_event, Some(serde_json::json!({"kind":"cancelled"})));
    assert!(store.pending_download_events(ACCOUNT, 1)?.is_empty());
    store.update_download_state(ACCOUNT, "reader", DownloadStatus::Failed, Some("new_failure"), 24)?;
    assert!(store.load_download(ACCOUNT, "reader")?.unwrap().terminal_event.is_none());
    store.save_download(ACCOUNT, &failed)?;
    store.update_download_state(ACCOUNT, "reader", DownloadStatus::Failed, Some("fixture_failure"), 25)?;
    assert_eq!(store.load_download(ACCOUNT, "reader")?.unwrap().terminal_event, failed.terminal_event);
    Ok(())
}
