//! Catalog and selection completeness through the production application intents.

use super::*;
use crate::{
    platform::DeviceProfile,
    source::{self, AccountIdentity, AuthSession, BiliClient, SourceError,
        crypto::{ImageKey, ProtocolCrypto, RequestContext, Signature},
        transport::{HttpRequest, HttpResponse, ResponseFuture, Transport}},
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}},
};

struct CatalogTransport {
    responses: Mutex<VecDeque<Value>>,
    requests: AtomicUsize,
}

impl Transport for CatalogTransport {
    fn send(&self, request: HttpRequest) -> ResponseFuture<'_> {
        Box::pin(async move {
            assert!(request.url.contains("/comic.v1.Comic/ComicDetail?"));
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["comic_id"], 1);
            assert!(body.get("page_num").is_none());
            assert!(body.get("page_size").is_none());
            self.requests.fetch_add(1, Ordering::SeqCst);
            let data = self.responses.lock().unwrap().pop_front().expect("An expected catalog response");
            Ok(HttpResponse { status: 200, headers: Vec::new(),
                body: serde_json::to_vec(&json!({"code": 0, "data": data})).unwrap() })
        })
    }
}

struct CatalogCrypto;

impl ProtocolCrypto for CatalogCrypto {
    fn sign(&self, context: &RequestContext) -> source::Result<Signature> {
        assert_eq!(context.endpoint, "ComicDetail");
        Ok(Signature { ultra_sign: "fixture-sign".into(), data_sn: "fixture-sn".into() })
    }
    fn decode_response(&self, _: &RequestContext, _: Value) -> source::Result<Value> {
        Err(SourceError::protocol("unexpected_catalog_fixture_encryption"))
    }
    fn new_image_key(&self) -> source::Result<ImageKey> {
        Err(SourceError::protocol("unexpected_catalog_fixture_image_key"))
    }
    fn decrypt_image(&self, _: &ImageKey, _: &str, _: &[u8]) -> source::Result<Vec<u8>> {
        Err(SourceError::protocol("unexpected_catalog_fixture_image"))
    }
}

fn session(account: &str) -> source::Result<AuthSession> {
    AuthSession::from_cookie_text(&format!("SESSDATA=fixture-session; bili_jct=fixture-csrf; DedeUserID={account}"))?
        .with_identity(AccountIdentity { id: account.into(), name: "Synthetic Catalog Account".into(),
            avatar_url: String::new() }, 1)
}

fn catalog_client(data_dir: &Path, responses: Vec<Value>) -> Result<(BiliClient, Arc<CatalogTransport>)> {
    let transport = Arc::new(CatalogTransport { responses: Mutex::new(responses.into()), requests: AtomicUsize::new(0) });
    let client = BiliClient::with_adapters(Some(session("11")?), data_dir, transport.clone(), Arc::new(CatalogCrypto));
    Ok((client, transport))
}

fn catalog_app(data_dir: &Path, profile: DeviceProfile, client: BiliClient, handle: tokio::runtime::Handle) -> Result<App> {
    super::super::test_support::app(data_dir, profile, client, handle)
}

fn directory(readable: usize, locked: usize) -> ComicDetail {
    let episodes = (0..readable + locked).map(|position| Episode {
        id: (10_001 + position).to_string(), comic_id: "1".into(), title: format!("Synthetic episode {}", position + 1),
        order: (position + 1) as f64, read: position % 2 == 0,
        access: if position >= readable { EpisodeAccess::Locked }
            else if position % 3 == 0 { EpisodeAccess::Owned } else { EpisodeAccess::Free },
        ..Default::default()
    }).collect();
    ComicDetail { comic: Comic { id: "1".into(), title: "Synthetic Catalog".into(),
        chapter_count: (readable + locked) as u32, ..Default::default() }, episodes, ..Default::default() }
}

fn wire_directory(detail: &ComicDetail) -> Value {
    let rows: Vec<_> = detail.episodes.iter().rev().map(|episode| json!({
        "id": episode.id.parse::<u64>().unwrap(), "comic_id": 1,
        "title": episode.title, "ord": episode.order, "is_read": episode.read,
        "pay_mode": if episode.access == EpisodeAccess::Free { 0 } else { 1 },
        "unlock_type": if episode.access == EpisodeAccess::Owned { 1 } else { 0 },
        "is_locked": episode.access == EpisodeAccess::Locked,
    })).collect();
    json!({"id": 1, "title": detail.comic.title, "total": detail.episodes.len(), "ep_list": rows})
}

fn apply_detail_job(app: &mut App) -> Result<()> {
    let event = super::super::test_support::next_job(app, Duration::from_secs(10))?;
    anyhow::ensure!(event.key == "catalog:detail" && event.epoch == app.epoch
        && app.pending.get(&event.key) == Some(&event.sequence), "Only the current catalog response may be applied");
    app.pending.remove(&event.key);
    (event.apply)(app);
    anyhow::ensure!(!app.catalog.detail_loading, "The production catalog callback must finish the request");
    Ok(())
}

fn select_visible_pages(app: &mut App) -> Result<()> {
    let detail = app.catalog.detail.as_ref().context("A loaded directory")?;
    let per_page = if app.is_scribe() { 9 } else { 7 };
    let pages = page_count(app.filtered_episodes(detail)?.len(), per_page);
    for page in 1..=pages {
        app.dispatch("catalog", "detail.select-page", "")?;
        if page < pages { app.dispatch("catalog", "episode.page", "1")?; }
    }
    Ok(())
}

#[test]
fn six_hundred_chapter_choices_survive_pages_filters_and_account_return_until_confirmation() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_time().build()?;
    for profile in [DeviceProfile::Paperwhite6, DeviceProfile::Scribe] {
        let temporary = tempfile::tempdir()?;
        let detail = directory(600, 12);
        let mut refreshed = wire_directory(&detail);
        let mut rows = refreshed.as_object_mut().unwrap().remove("ep_list").unwrap();
        for row in rows.as_array_mut().unwrap() { row.as_object_mut().unwrap().remove("comic_id"); }
        refreshed["episodes"] = rows;
        let (client, transport) = catalog_client(temporary.path(), vec![wire_directory(&detail), refreshed])?;
        let mut app = catalog_app(temporary.path(), profile, client, runtime.handle().clone())?;
        app.dispatch("catalog", "comic.open", "1")?;
        apply_detail_job(&mut app)?;
        assert_eq!(app.catalog.detail.as_ref().unwrap().episodes.len(), 612);
        assert_eq!(app.catalog.detail.as_ref().unwrap().episodes[0].id, "10001");
        let expected: BTreeSet<_> = detail.episodes.iter().filter(|episode| episode.readable_at(now()))
            .map(|episode| episode.id.clone()).collect();
        assert_eq!(expected.len(), 600);

        app.dispatch("catalog", "detail.download", "enter")?;
        app.dispatch("catalog", "episode.filter", "unread")?;
        app.dispatch("catalog", "detail.select-page", "")?;
        assert_eq!(app.catalog.selected.len(), if profile.is_scribe() { 9 } else { 7 },
            "The current-page action must not silently select the entire directory");
        select_visible_pages(&mut app)?;
        let unread: BTreeSet<_> = detail.episodes.iter().filter(|episode| !episode.read && episode.readable_at(now()))
            .map(|episode| episode.id.clone()).collect();
        assert_eq!(app.catalog.selected, unread);

        app.dispatch("catalog", "episode.filter", "all")?;
        app.dispatch("catalog", "episode.sort", "")?;
        select_visible_pages(&mut app)?;
        assert_eq!(app.catalog.selected, expected);
        app.dispatch("catalog", "episode.filter", "downloaded")?;
        assert!(app.filtered_episodes(app.catalog.detail.as_ref().unwrap())?.is_empty());
        assert_eq!(app.catalog.selected, expected, "Filtering must preserve the cross-page selection");

        let intent = app.capture_catalog_auth_return().context("Capture every selected chapter")?;
        assert_eq!(intent.selected, expected);
        // Exercise the catalog boundary used by account activation without
        // starting unrelated wallet, recovery, or device-status jobs.
        app.client.replace_session(Some(session("12")?))?;
        app.epoch += 1;
        app.account = "bili_12".into();
        app.clear_catalog_auth_context();
        app.stage_catalog_auth_return(Some(intent));
        app.route = "login".into();
        assert!(app.finish_catalog_auth_return()?);
        assert!(app.catalog.selected.is_empty());
        assert!(!app.catalog.selecting, "Old account access must not be used before the fresh response");
        apply_detail_job(&mut app)?;
        assert_eq!(app.catalog.selected, expected);
        assert!(app.catalog.selecting);
        assert!(app.catalog.auth_detail_pending.is_none());
        assert_eq!(app.route, "detail");
        assert!(app.toast_until.is_some(), "The restored choices must require explicit reconfirmation");
        assert!(app.store.downloads(&app.account)?.is_empty());
        assert!(app.store.preference::<Vec<Value>>(&app.account, "queued_download_intents")?.is_none());

        app.dispatch("catalog", "detail.download", "confirm")?;
        let queued: Vec<Value> = app.store.preference(&app.account, "queued_download_intents")?
            .context("Persist every explicitly confirmed chapter while the manager is unavailable")?;
        let queued_ids: BTreeSet<_> = queued.iter().map(|intent| intent["episode_id"].as_str().unwrap().to_owned()).collect();
        assert_eq!(queued.len(), 600);
        assert_eq!(queued_ids, expected);
        assert!(queued.iter().all(|intent| intent["retain"] == true && intent["comic_id"] == "1"));
        assert!(app.catalog.selected.is_empty());
        assert_eq!(app.route, "downloads");
        assert_eq!(transport.requests.load(Ordering::SeqCst), 2);
        assert!(transport.responses.lock().unwrap().is_empty());
    }
    Ok(())
}

#[test]
fn malformed_directories_cannot_replace_the_complete_cached_catalog() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_time().build()?;
    let temporary = tempfile::tempdir()?;
    let original = directory(600, 0);
    let good = wire_directory(&original);
    let mut cases = Vec::new();
    for (name, row) in [
        ("null row", Value::Null), ("string row", json!("invalid")), ("numeric row", json!(2)),
        ("array row", json!([])), ("missing identity", json!({"title": "No identity"})),
        ("zero identity", json!({"id": 0})), ("noncanonical identity", json!({"id": "010001"})),
        ("wrong comic", json!({"id": 10001, "comic_id": 2})),
        ("malformed comic identity", json!({"id": 10001, "comic_id": false})),
        ("empty comic identity", json!({"id": 10001, "comic_id": ""})),
    ] {
        let mut response = good.clone();
        response["ep_list"][300] = row;
        cases.push((name, response));
    }
    let mut duplicate = good.clone();
    duplicate["ep_list"][300] = duplicate["ep_list"][0].clone();
    cases.push(("duplicate identity", duplicate));
    cases.push(("missing directory", json!({"id": 1, "title": "Missing directory"})));
    cases.push(("nonarray directory", json!({"id": 1, "ep_list": {"id": 10001}})));
    let (client, transport) = catalog_client(temporary.path(), cases.iter().map(|(_, response)| response.clone()).collect())?;
    let mut app = catalog_app(temporary.path(), DeviceProfile::Paperwhite6, client, runtime.handle().clone())?;
    app.store.save_detail(&app.account, &original)?;
    for (name, _) in &cases {
        app.dispatch("catalog", "comic.open", "1")?;
        assert!(app.catalog.detail_loading);
        apply_detail_job(&mut app)?;
        assert_eq!(app.catalog.status, i18n::text("cat_detail_failed_cached"), "{name}");
        assert_eq!(app.catalog.detail.as_ref(), Some(&original), "The visible cached catalog must survive {name}");
        assert_eq!(app.store.load_detail(&app.account, "1")?, Some(original.clone()),
            "The persisted complete catalog must survive {name}");
    }
    assert_eq!(transport.requests.load(Ordering::SeqCst), cases.len());
    assert!(transport.responses.lock().unwrap().is_empty());
    Ok(())
}
