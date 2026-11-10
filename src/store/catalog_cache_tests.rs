use super::*;
use crate::domain::{Episode, PageDescriptor};

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
    let evicted = store.prune_catalog_cache(ACCOUNT, &["active".into()])?;
    assert_eq!(evicted, vec!["evict"]);
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
    displace_recent_comics(&mut store)?;
    let evicted = store.prune_catalog_cache(ACCOUNT, &[])?;
    assert_eq!(evicted.len(), results.len());
    assert!(store.load_catalog(ACCOUNT, "search:active")?.is_empty());
    assert_eq!(store.load_comics(ACCOUNT)?.len(), CATALOG_COMIC_CACHE_LIMIT);
    Ok(())
}

#[test]
fn legacy_cache_cleanup_is_bounded_and_keeps_noncache_namespaces() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut store = Store::open(directory.path().join("state.sqlite3"))?;
    for index in 0..CATALOG_SCOPE_CACHE_LIMIT + 9 {
        store.save_catalog(ACCOUNT, &format!("store:{index:04}"), &[comic(&format!("scope-{index:04}"))])?;
    }
    store.save_catalog(ACCOUNT, "Search:user-owned-namespace", &[comic("noncache")])?;
    store.connection.execute("DELETE FROM preferences WHERE account_key=?1 AND key=?2",
        params![ACCOUNT,CATALOG_CACHE_RECENCY_KEY])?;
    store.prune_catalog_cache(ACCOUNT, &[])?;
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
    assert!(store.prune_catalog_cache(ACCOUNT, &[]).is_err());
    assert!(store.comic(ACCOUNT, "old")?.is_some());
    assert!(store.load_detail(ACCOUNT, "old")?.is_some());
    assert_eq!(store.load_catalog(ACCOUNT, "search:old")?.len(), 1);
    assert_eq!(store.preference::<serde_json::Value>(ACCOUNT, CATALOG_CACHE_RECENCY_KEY)?, before);
    store.connection.execute_batch("DROP TRIGGER reject_catalog_delete;")?;
    assert_eq!(store.prune_catalog_cache(ACCOUNT, &[])?, vec!["old"]);
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
    let started = std::time::Instant::now();
    assert_eq!(store.try_prune_catalog_cache(ACCOUNT, &[])?, None);
    assert!(started.elapsed() < Duration::from_secs(1), "Optional cleanup waited on the unrelated writer");
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
