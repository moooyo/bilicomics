use super::*;
use crate::domain::{Episode, PageDescriptor};
use std::collections::BTreeSet;

const ACCOUNT: &str = "catalog-retention-fixture";

fn comic(id: &str) -> Comic {
    Comic { id: id.into(), title: format!("Fictional comic {id}"), ..Default::default() }
}

fn detail(id: &str, episode_id: &str) -> ComicDetail {
    ComicDetail { comic: comic(id), episodes: vec![Episode {
        id: episode_id.into(), comic_id: id.into(), ..Default::default()
    }], ..Default::default() }
}

fn displace_recent_comics(store: &mut Store) -> Result<()> {
    let newer: Vec<_> = (0..CATALOG_COMIC_CACHE_LIMIT)
        .map(|index| comic(&format!("recent-{index:04}"))).collect();
    store.save_catalog(ACCOUNT, "search:recent", &newer)
}

fn clear_recent_details(store: &Store) -> Result<()> {
    let mut recent = catalog_cache_recency(&store.connection, ACCOUNT)?;
    recent.details.clear();
    save_catalog_cache_recency(&store.connection, ACCOUNT, &recent)
}

fn committed_maintenance_writes(trace: &CatalogMaintenanceTrace) -> [usize; 5] {
    assert_eq!(trace.outcome, "committed");
    assert!(trace.committed);
    assert_eq!(trace.timeout_restored, Some(true));
    assert_eq!(trace.phases.iter().map(|phase| phase.stage).collect::<Vec<_>>(), [
        "timeout_setup", "begin_immediate", "recency", "durable", "delete_scopes", "eviction_candidates",
        "delete_details", "delete_members", "delete_comics", "reconcile", "recency_upsert", "commit",
    ]);
    assert!(trace.phases.iter().all(|phase| phase.ok));
    ["delete_scopes", "delete_details", "delete_members", "delete_comics", "recency_upsert"]
        .map(|stage| trace.phases.iter().find(|phase| phase.stage == stage).unwrap().rows.unwrap())
}

fn assert_uncommitted_at(trace: &CatalogMaintenanceTrace, outcome: &str, stage: &str) {
    assert_eq!(trace.outcome, outcome);
    assert!(!trace.committed);
    assert_eq!(trace.timeout_restored, Some(true));
    let (failed, preceding) = trace.phases.split_last().unwrap();
    assert!(preceding.iter().all(|phase| phase.ok));
    assert_eq!((failed.stage, failed.ok, failed.rows), (stage, false, None));
}

fn anchor(episode_id: &str) -> ReadingAnchor {
    ReadingAnchor { episode_id: episode_id.into(), revision: "revision".into(),
        page_id: "page".into(), zoom_ratio: 1.0, ..Default::default() }
}

fn manifest(episode_id: &str) -> ImageIndex {
    ImageIndex { episode_id: episode_id.into(), revision: "revision".into(),
        images: vec![PageDescriptor { id: "page".into(), index: 0, width: 1, height: 1 }],
        ..Default::default() }
}

fn legacy_page(episode_id: &str) -> PageReceipt {
    PageReceipt { episode_id: episode_id.into(), revision: "revision".into(),
        page_id: "page".into(), file_name: "a".repeat(64), sha256: "a".repeat(64),
        bytes: 1, width: 1, height: 1, ..Default::default() }
}

fn separate_shelf_reads(store: &Store, account: &str) -> Result<(Vec<Comic>, BTreeSet<String>)> {
    let comics = store.load_comics(account)?;
    let members = store.load_catalog(account, "shelf")?.into_iter().map(|comic| comic.id).collect();
    Ok((comics, members))
}

#[test]
fn combined_shelf_read_preserves_order_progress_payload_ids_and_account_membership() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut store = Store::open(directory.path().join("state.sqlite3"))?;
    store.save_catalog(ACCOUNT, "shelf", &[comic("member"), comic("stored-id")])?;
    let favorite = Comic { favorite: true, last_read_at: 7, ..comic("favorite") };
    store.save_catalog(ACCOUNT, "history", std::slice::from_ref(&favorite))?;
    store.save_catalog("other-account", "shelf", &[favorite, comic("orphan")])?;
    store.connection.execute("INSERT INTO catalog_members(account_key,scope,comic_id,position)
        VALUES(?1,'shelf','orphan',2)", [ACCOUNT])?;
    store.connection.execute("UPDATE comics SET payload=?2 WHERE account_key=?1 AND comic_id='stored-id'",
        params![ACCOUNT, to_json(&comic("payload-id"))?])?;
    let mut index = manifest("episode");
    index.images.push(PageDescriptor { id: "second-page".into(), index: 1, width: 1, height: 1 });
    store.save_index(ACCOUNT, &index)?;
    store.save_anchor(ACCOUNT, "payload-id", &ReadingAnchor { page_index: 1, page_id: "second-page".into(),
        updated_at: 30, ..anchor("episode") })?;

    let actual = store.load_comics_with_shelf(ACCOUNT)?;
    assert_eq!(actual, separate_shelf_reads(&store, ACCOUNT)?);
    let (comics, members) = actual;
    assert_eq!(comics.iter().map(|comic| comic.id.as_str()).collect::<Vec<_>>(), ["payload-id", "favorite", "member"]);
    assert_eq!((comics[0].last_episode_id.as_deref(), comics[0].last_read_at, comics[0].progress), (Some("episode"), 30, 50));
    assert_eq!(members, BTreeSet::from(["member".to_owned(), "payload-id".to_owned()]));
    assert_eq!(store.load_comics_with_shelf("unused-account")?, (Vec::new(), BTreeSet::new()));
    Ok(())
}

#[test]
fn combined_shelf_read_propagates_corrupt_comics_anchors_and_indices() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut store = Store::open(directory.path().join("state.sqlite3"))?;
    store.save_comics(ACCOUNT, &[comic("outside")])?;
    store.save_catalog(ACCOUNT, "shelf", &[comic("member")])?;
    store.save_index(ACCOUNT, &manifest("episode"))?;
    store.save_anchor(ACCOUNT, "member", &anchor("episode"))?;
    for update in [
        "UPDATE comics SET payload='invalid' WHERE comic_id='outside'",
        "UPDATE reading_anchors SET payload='invalid' WHERE comic_id='member'",
        "UPDATE image_indices SET payload='invalid' WHERE episode_id='episode'",
    ] {
        let transaction = store.connection.unchecked_transaction()?;
        assert_eq!(transaction.execute(update, [])?, 1);
        assert!(separate_shelf_reads(&store, ACCOUNT).is_err());
        assert!(store.load_comics_with_shelf(ACCOUNT).is_err());
        transaction.rollback()?;
    }
    Ok(())
}

#[test]
#[ignore = "Compare warmed synthetic catalog reads on the verification worker"]
fn compare_warm_catalog_shelf_reads() -> Result<()> {
    use std::time::Instant;

    let directory = tempfile::tempdir()?;
    let mut store = Store::open(directory.path().join("state.sqlite3"))?;
    let comics: Vec<_> = (0..256).map(|position| comic(&format!("comic-{position:03}"))).collect();
    store.save_comics(ACCOUNT, &comics)?;
    for (position, comic) in comics.iter().enumerate().step_by(4) {
        let episode = format!("episode-{position}");
        store.save_index(ACCOUNT, &manifest(&episode))?;
        store.save_anchor(ACCOUNT, &comic.id, &ReadingAnchor { updated_at: position as i64 + 1, ..anchor(&episode) })?;
    }
    for member_count in [0, 8, comics.len()] {
        store.save_catalog(ACCOUNT, "shelf", &comics[..member_count])?;
        let expected = separate_shelf_reads(&store, ACCOUNT)?;
        assert_eq!((expected.0.len(), expected.1.len()), (comics.len(), member_count));
        assert_eq!(store.load_comics_with_shelf(ACCOUNT)?, expected);
        let measure = |combined: bool| -> Result<u128> {
            let started = Instant::now();
            let result = if combined { store.load_comics_with_shelf(ACCOUNT)? } else { separate_shelf_reads(&store, ACCOUNT)? };
            let elapsed = started.elapsed().as_micros();
            assert_eq!(result, expected);
            Ok(elapsed)
        };
        for pair in 1..=6 {
            let (order, separate_us, combined_us) = if pair % 2 == 1 {
                ("AB", measure(false)?, measure(true)?)
            } else {
                let combined_us = measure(true)?;
                ("BA", measure(false)?, combined_us)
            };
            eprintln!("catalog_shelf_read comics={} shelf={member_count} pair={pair} order={order} separate_us={separate_us} combined_us={combined_us}", comics.len());
        }
    }
    Ok(())
}

#[test]
fn transient_scopes_are_bounded_even_when_a_favorite_appears_in_every_search() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut store = Store::open(directory.path().join("state.sqlite3"))?;
    let favorite = Comic { favorite: true, ..comic("favorite") };
    store.save_catalog(ACCOUNT, "shelf", std::slice::from_ref(&favorite))?;
    store.save_catalog(ACCOUNT, "history", &[comic("history")])?;
    store.save_catalog("other-account", "search:foreign", &[comic("foreign")])?;
    for index in 0..CATALOG_SCOPE_CACHE_LIMIT + 7 {
        store.save_catalog(ACCOUNT, &format!("search:{index:04}"), std::slice::from_ref(&favorite))?;
        store.prune_catalog_cache(ACCOUNT, std::slice::from_ref(&favorite.id))?;
    }
    let scopes: i64 = store.connection.query_row(
        "SELECT COUNT(DISTINCT scope) FROM catalog_members WHERE account_key=?1 AND scope GLOB 'search:*'",
        [ACCOUNT], |row| row.get(0))?;
    assert_eq!(scopes, CATALOG_SCOPE_CACHE_LIMIT as i64);
    assert!(store.load_catalog(ACCOUNT, "search:0000")?.is_empty());
    assert_eq!(store.load_catalog(ACCOUNT, "shelf")?.len(), 1);
    assert_eq!(store.load_catalog(ACCOUNT, "history")?.len(), 1);
    assert_eq!(store.load_catalog("other-account", "search:foreign")?.len(), 1);
    Ok(())
}

#[test]
fn evicted_comic_ids_exclude_active_results_and_all_durable_owners() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut store = Store::open(directory.path().join("state.sqlite3"))?;
    let durable = ["favorite", "history", "shelf", "anchor", "download", "index", "receipt", "identity",
        "journal", "purchase", "queued", "reader-settings", "recently-read"];
    for id in durable.iter().chain(["active", "evict"].iter()) {
        store.save_detail(ACCOUNT, &detail(id, &format!("episode-{id}")))?;
    }
    store.save_comic(ACCOUNT, &Comic { favorite: true, ..comic("favorite") })?;
    store.save_comic(ACCOUNT, &Comic { last_read_at: 20, ..comic("recently-read") })?;
    store.save_catalog(ACCOUNT, "shelf", &[comic("shelf")])?;
    store.save_catalog(ACCOUNT, "history", &[comic("history")])?;
    store.save_anchor(ACCOUNT, "anchor", &anchor("episode-anchor"))?;
    store.save_download(ACCOUNT, &DownloadRecord { id: "download-task".into(), comic_id: "download".into(),
        episode_id: "episode-download".into(), revision: PENDING_REVISION.into(), ..Default::default() })?;
    store.save_index(ACCOUNT, &manifest("episode-index"))?;
    let receipt = legacy_page("episode-receipt");
    store.connection.execute(
        "INSERT INTO page_receipts(account_key,episode_id,revision,page_id,page_index,payload) VALUES(?1,?2,?3,?4,0,?5)",
        params![ACCOUNT,receipt.episode_id,receipt.revision,receipt.page_id,to_json(&receipt)?])?;
    store.connection.execute(
        "INSERT INTO page_identity_history(account_key,episode_id,revision,page_id,sha256,bytes) VALUES(?1,'episode-identity','revision','page',?2,1)",
        params![ACCOUNT,"a".repeat(64)])?;
    let pending = legacy_page("episode-journal");
    store.connection.execute(
        "INSERT INTO page_commit_journal(account_key,task_id,page_id,generation,payload) VALUES(?1,'pending-task','page',1,?2)",
        params![ACCOUNT,to_json(&pending)?])?;
    let purchase = serde_json::json!([{"id":"unknown-purchase","state":"unknown","quote":{"comic_id":"purchase"}}]);
    store.set_preference(ACCOUNT, "purchase_journal", &purchase)?;
    let queued = serde_json::json!([{"comic_id":"queued","episode_id":"episode-queued","retain":true}]);
    store.set_preference(ACCOUNT, "queued_download_intents", &queued)?;
    let recharge = serde_json::json!({"id":"uncertain-recharge","state":"unknown"});
    store.set_preference(ACCOUNT, "recharge_intent", &recharge)?;
    store.set_preference(ACCOUNT, "reader_preferences_reader-settings", &serde_json::json!({"mode":"strip"}))?;
    store.save_detail("other-account", &detail("evict", "foreign-episode"))?;

    displace_recent_comics(&mut store)?;
    clear_recent_details(&store)?;
    let mut trace = CatalogMaintenanceTrace::default();
    let evicted = store.try_prune_catalog_cache_observed(ACCOUNT, &["active".into()], &mut trace)?.unwrap();
    assert_eq!(evicted, vec!["evict"]);
    assert_eq!(committed_maintenance_writes(&trace)[3], 1);
    assert_eq!(trace.phases.iter().find(|phase| phase.stage == "durable").unwrap().rows, Some(durable.len()));
    for id in durable.iter().chain(["active"].iter()) {
        assert!(store.comic(ACCOUNT, id)?.is_some(), "Protected comic {id} was removed");
        assert!(store.load_detail(ACCOUNT, id)?.is_some(), "Protected detail {id} was removed");
    }
    assert!(store.load_detail(ACCOUNT, "evict")?.is_none());
    assert!(store.load_detail("other-account", "evict")?.is_some());
    assert_eq!(store.preference::<serde_json::Value>(ACCOUNT, "purchase_journal")?, Some(purchase));
    assert_eq!(store.preference::<serde_json::Value>(ACCOUNT, "queued_download_intents")?, Some(queued));
    assert_eq!(store.preference::<serde_json::Value>(ACCOUNT, "recharge_intent")?, Some(recharge));
    assert_eq!(store.page_receipts(ACCOUNT, "episode-receipt", "revision")?, vec![receipt]);
    assert_eq!(store.pending_page_commits(ACCOUNT)?.len(), 1);
    assert!(store.load_index(ACCOUNT, "episode-index", None)?.is_some());
    Ok(())
}

#[test]
fn detail_retention_preserves_complete_active_chapter_lists() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut store = Store::open(directory.path().join("state.sqlite3"))?;
    let large = ComicDetail { comic: comic("active-large"), episodes: (0..1025)
        .map(|index| Episode { id: format!("large-{index}"), comic_id: "active-large".into(),
            ..Default::default() }).collect(), ..Default::default() };
    store.save_detail(ACCOUNT, &large)?;
    for index in 0..CATALOG_DETAIL_CACHE_LIMIT + 9 {
        store.save_detail(ACCOUNT, &detail(&format!("detail-{index:04}"), &format!("episode-{index}")))?;
    }
    assert!(store.prune_catalog_cache(ACCOUNT, &["active-large".into()])?.is_empty());
    assert_eq!(store.load_detail(ACCOUNT, "active-large")?, Some(large));
    assert!(store.load_detail(ACCOUNT, "detail-0000")?.is_none());
    assert!(store.comic(ACCOUNT, "detail-0000")?.is_some());
    let count: i64 = store.connection.query_row(
        "SELECT COUNT(*) FROM comic_details WHERE account_key=?1", [ACCOUNT], |row| row.get(0))?;
    assert_eq!(count, CATALOG_DETAIL_CACHE_LIMIT as i64 + 1);
    Ok(())
}

#[test]
fn replacing_an_active_query_reclaims_its_old_excess_without_truncating_the_current_query() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut store = Store::open(directory.path().join("state.sqlite3"))?;
    let results: Vec<_> = (0..CATALOG_COMIC_CACHE_LIMIT + 100)
        .map(|index| comic(&format!("old-{index:04}"))).collect();
    let active: Vec<_> = results.iter().map(|comic| comic.id.clone()).collect();
    store.save_catalog(ACCOUNT, "search:active", &results)?;
    assert!(store.prune_catalog_cache(ACCOUNT, &active)?.is_empty());
    assert_eq!(store.load_catalog(ACCOUNT, "search:active")?.len(), results.len());
    store.save_comic(ACCOUNT, &comic("old-without-membership"))?;
    displace_recent_comics(&mut store)?;
    let mut trace = CatalogMaintenanceTrace::default();
    let evicted = store.try_prune_catalog_cache_observed(ACCOUNT, &[], &mut trace)?.unwrap();
    assert_eq!(evicted.len(), results.len() + 1);
    assert!(evicted.iter().any(|id| id == "old-without-membership"));
    assert_eq!(committed_maintenance_writes(&trace), [0, 0, results.len(), results.len() + 1, 1]);
    assert_eq!(trace.phases.iter().find(|phase| phase.stage == "eviction_candidates").unwrap().rows, Some(evicted.len()));
    assert!(store.load_catalog(ACCOUNT, "search:active")?.is_empty());
    assert_eq!(store.load_comics(ACCOUNT)?.len(), CATALOG_COMIC_CACHE_LIMIT);
    Ok(())
}

#[test]
fn legacy_cache_cleanup_is_bounded_and_keeps_noncache_namespaces() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut store = Store::open(directory.path().join("state.sqlite3"))?;
    for index in 0..CATALOG_SCOPE_CACHE_LIMIT + 9 {
        let mut members = vec![comic(&format!("scope-{index:04}"))];
        if index == CATALOG_SCOPE_CACHE_LIMIT + 8 { members.push(comic("extra-in-last-scope")); }
        store.save_catalog(ACCOUNT, &format!("store:{index:04}"), &members)?;
    }
    store.save_catalog(ACCOUNT, "Search:user-owned-namespace", &[comic("noncache")])?;
    store.connection.execute("DELETE FROM preferences WHERE account_key=?1 AND key=?2",
        params![ACCOUNT,CATALOG_CACHE_RECENCY_KEY])?;
    assert!(store.preference::<serde_json::Value>(ACCOUNT, CATALOG_CACHE_RECENCY_KEY)?.is_none());
    let mut trace = CatalogMaintenanceTrace::default();
    assert!(store.try_prune_catalog_cache_observed(ACCOUNT, &[], &mut trace)?.unwrap().is_empty());
    assert_eq!(trace.recency_present, Some(false));
    assert_eq!(committed_maintenance_writes(&trace), [10, 0, 0, 0, 1]);
    let recent = catalog_cache_recency(&store.connection, ACCOUNT)?;
    assert_eq!(recent.scopes.len(), CATALOG_SCOPE_CACHE_LIMIT);
    assert!(recent.comics.len() <= CATALOG_COMIC_CACHE_LIMIT);
    assert!(recent.details.len() <= CATALOG_DETAIL_CACHE_LIMIT);
    assert_eq!(store.load_catalog(ACCOUNT, "Search:user-owned-namespace")?.len(), 1);
    let version: i64 = store.connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(version, 4);
    Ok(())
}

#[test]
fn observed_maintenance_distinguishes_existing_recency_rewrite_noop_and_invalid_payload() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut store = Store::open(directory.path().join("state.sqlite3"))?;
    let spaced = r#"{"scopes": ["store:all"], "comics": ["study"], "details": []}"#;
    let transaction = store.connection.transaction()?;
    upsert_comic(&transaction, ACCOUNT, &comic("study"))?;
    for scope in ["shelf", "store:all"] {
        transaction.execute("INSERT INTO catalog_members(account_key,scope,comic_id,position) VALUES(?1,?2,'study',0)",
            params![ACCOUNT,scope])?;
    }
    transaction.execute("INSERT INTO preferences(account_key,key,payload) VALUES(?1,?2,?3)",
        params![ACCOUNT,CATALOG_CACHE_RECENCY_KEY,spaced])?;
    transaction.commit()?;

    let mut trace = CatalogMaintenanceTrace::default();
    let payload = || -> rusqlite::Result<String> { store.connection.query_row(
        "SELECT payload FROM preferences WHERE account_key=?1 AND key=?2", params![ACCOUNT,CATALOG_CACHE_RECENCY_KEY], |row| row.get(0)) };
    assert_eq!(payload()?, spaced);
    for written in [1, 0] {
        let before = store.connection.total_changes();
        assert!(store.try_prune_catalog_cache_observed(ACCOUNT, &[], &mut trace)?.unwrap().is_empty());
        assert_eq!(trace.recency_present, Some(true));
        assert_eq!(committed_maintenance_writes(&trace), [0, 0, 0, 0, written]);
        assert_eq!(store.connection.total_changes() - before, written as u64);
        let canonical = payload()?;
        assert_ne!(canonical, spaced);
        assert_eq!(from_json::<serde_json::Value>(&canonical)?, from_json::<serde_json::Value>(spaced)?);
    }
    store.connection.execute("UPDATE preferences SET payload='invalid' WHERE account_key=?1 AND key=?2",
        params![ACCOUNT,CATALOG_CACHE_RECENCY_KEY])?;
    assert!(store.try_prune_catalog_cache_observed(ACCOUNT, &[], &mut trace).is_err());
    assert_uncommitted_at(&trace, "error", "recency");
    assert_eq!(trace.recency_present, Some(true));
    assert_eq!(payload()?, "invalid");
    Ok(())
}

#[test]
fn cache_cleanup_rolls_back_all_metadata_when_storage_rejects_a_delete() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut store = Store::open(directory.path().join("state.sqlite3"))?;
    store.save_detail(ACCOUNT, &detail("old", "old-episode"))?;
    store.save_catalog(ACCOUNT, "search:old", &[comic("old")])?;
    displace_recent_comics(&mut store)?;
    clear_recent_details(&store)?;
    let before = store.preference::<serde_json::Value>(ACCOUNT, CATALOG_CACHE_RECENCY_KEY)?;
    store.connection.execute_batch(
        "CREATE TRIGGER reject_catalog_delete BEFORE DELETE ON comics
           BEGIN SELECT RAISE(FAIL,'Controlled catalog metadata failure'); END;")?;
    let mut trace = CatalogMaintenanceTrace::default();
    assert!(store.try_prune_catalog_cache_observed(ACCOUNT, &[], &mut trace).is_err());
    assert_uncommitted_at(&trace, "error", "delete_comics");
    for stage in ["delete_details", "delete_members"] {
        assert_eq!(trace.phases.iter().find(|phase| phase.stage == stage).unwrap().rows, Some(1));
    }
    assert!(store.comic(ACCOUNT, "old")?.is_some());
    assert!(store.load_detail(ACCOUNT, "old")?.is_some());
    assert_eq!(store.load_catalog(ACCOUNT, "search:old")?.len(), 1);
    assert_eq!(store.preference::<serde_json::Value>(ACCOUNT, CATALOG_CACHE_RECENCY_KEY)?, before);
    store.connection.execute_batch("DROP TRIGGER reject_catalog_delete;")?;
    assert_eq!(store.try_prune_catalog_cache_observed(ACCOUNT, &[], &mut trace)?.unwrap(), vec!["old"]);
    assert_eq!(committed_maintenance_writes(&trace), [0, 1, 1, 1, 1]);
    Ok(())
}

#[test]
fn optional_cache_maintenance_never_waits_for_a_writer_and_restores_connection_policy() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut store = Store::open(directory.path().join("state.sqlite3"))?;
    store.save_detail(ACCOUNT, &detail("old", "old-episode"))?;
    displace_recent_comics(&mut store)?;
    clear_recent_details(&store)?;
    let timeout: u64 = store.connection.pragma_query_value(None, "busy_timeout", |row| row.get(0))?;
    let synchronous: i64 = store.connection.pragma_query_value(None, "synchronous", |row| row.get(0))?;
    assert_eq!(timeout, 5000);
    assert_eq!(synchronous, 2);
    let writer = Connection::open(store.path())?;
    writer.execute_batch("BEGIN IMMEDIATE;")?;
    let mut trace = CatalogMaintenanceTrace::default();
    let started = std::time::Instant::now();
    assert_eq!(store.try_prune_catalog_cache_observed(ACCOUNT, &[], &mut trace)?, None);
    assert!(started.elapsed() < Duration::from_secs(1), "Optional cleanup waited on the unrelated writer");
    assert_uncommitted_at(&trace, "busy", "begin_immediate");
    assert_eq!(trace.phases.len(), 2);
    assert_eq!(trace.recency_present, None);
    assert!(store.comic(ACCOUNT, "old")?.is_some());
    assert_eq!(store.connection.pragma_query_value::<u64, _>(None, "busy_timeout", |row| row.get(0))?, timeout);
    assert_eq!(store.connection.pragma_query_value::<i64, _>(None, "synchronous", |row| row.get(0))?, synchronous);
    writer.execute_batch("ROLLBACK;")?;

    store.connection.execute_batch("CREATE TRIGGER reject_optional_cache_delete BEFORE DELETE ON comics
        BEGIN SELECT RAISE(FAIL,'Controlled optional maintenance failure'); END;")?;
    assert!(store.try_prune_catalog_cache(ACCOUNT, &[]).is_err());
    assert_eq!(store.connection.pragma_query_value::<u64, _>(None, "busy_timeout", |row| row.get(0))?, timeout);
    assert!(store.comic(ACCOUNT, "old")?.is_some());
    store.connection.execute_batch("DROP TRIGGER reject_optional_cache_delete;")?;
    assert_eq!(store.try_prune_catalog_cache(ACCOUNT, &[])?, Some(vec!["old".to_owned()]));
    assert_eq!(store.connection.pragma_query_value::<u64, _>(None, "busy_timeout", |row| row.get(0))?, timeout);
    assert_eq!(store.connection.pragma_query_value::<i64, _>(None, "synchronous", |row| row.get(0))?, synchronous);
    Ok(())
}
