//! Synthetic transport regressions for request cookies and durable response updates.

use super::*;
use super::crypto::{ImageKey, NativeCrypto, ProtocolCrypto, RequestContext, Signature};
use super::transport::{HttpRequest, HttpResponse, ResponseFuture, Transport};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    path::Path,
    sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}},
};
use tokio::sync::Notify;

const MANGA_HOST: &str = "manga.bilibili.com";

#[derive(Default)]
struct ResponseGate {
    entered: Notify,
    release: Notify,
}

struct Step {
    inspect: Box<dyn FnOnce(&HttpRequest) + Send>,
    response: HttpResponse,
    gate: Option<Arc<ResponseGate>>,
}

struct FixtureTransport {
    steps: Mutex<VecDeque<Step>>,
    calls: AtomicUsize,
}

impl Transport for FixtureTransport {
    fn send(&self, request: HttpRequest) -> ResponseFuture<'_> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let step = self.steps.lock().unwrap().pop_front().expect("Unexpected cookie fixture request");
        Box::pin(async move {
            if let Some(gate) = &step.gate { gate.entered.notify_one(); }
            (step.inspect)(&request);
            if let Some(gate) = &step.gate { gate.release.notified().await; }
            Ok(step.response)
        })
    }
}

#[derive(Default)]
struct FixtureCrypto {
    signed_devices: Mutex<Vec<(String, String)>>,
    decode_failure: Option<SourceError>,
    decode_calls: AtomicUsize,
}

impl ProtocolCrypto for FixtureCrypto {
    fn sign(&self, context: &RequestContext) -> Result<Signature> {
        self.signed_devices.lock().unwrap().push((context.endpoint.clone(), context.buvid.clone()));
        Ok(Signature { ultra_sign: "synthetic-cookie-signature".into(), data_sn: "synthetic-cookie-sn".into() })
    }

    fn decode_response(&self, _: &RequestContext, _: Value) -> Result<Value> {
        self.decode_calls.fetch_add(1, Ordering::SeqCst);
        Err(self.decode_failure.clone().unwrap_or_else(||
            SourceError::protocol("unexpected_cookie_fixture_response_crypto")))
    }

    fn new_image_key(&self) -> Result<ImageKey> {
        NativeCrypto::new(Path::new("unused-cookie-fixture-assets")).new_image_key()
    }

    fn decrypt_image(&self, _: &ImageKey, _: &str, _: &[u8]) -> Result<Vec<u8>> {
        Err(SourceError::protocol("unexpected_cookie_fixture_image_crypto"))
    }
}

fn fixture_session() -> AuthSession {
    AuthSession::from_cookie_text(concat!(
        "SESSDATA=synthetic-session; bili_jct=synthetic-csrf; DedeUserID=42; ",
        "DedeUserID__ckMd5=synthetic-user-md5; buvid3=synthetic-buvid3; ",
        "buvid4=synthetic-buvid4; buvid_fp=synthetic-fingerprint; ",
        "b_nut=synthetic-nut; b_lsid=synthetic-lsid; XSRF-TOKEN=synthetic%2Bxsrf+literal; ",
        "unrelated_cookie=must-not-be-sent"
    )).unwrap().with_identity(AccountIdentity {
        id: "42".into(), name: "Synthetic Cookie Fixture".into(), avatar_url: String::new(),
    }, 1).unwrap()
}

fn expected_cookies() -> BTreeMap<String, String> {
    [
        ("SESSDATA", "synthetic-session"), ("bili_jct", "synthetic-csrf"), ("DedeUserID", "42"),
        ("DedeUserID__ckMd5", "synthetic-user-md5"), ("buvid3", "synthetic-buvid3"),
        ("buvid4", "synthetic-buvid4"), ("buvid_fp", "synthetic-fingerprint"),
        ("b_nut", "synthetic-nut"), ("b_lsid", "synthetic-lsid"),
        ("XSRF-TOKEN", "synthetic%2Bxsrf+literal"),
    ].into_iter().map(|(name, value)| (name.to_owned(), value.to_owned())).collect()
}

fn header<'a>(request: &'a HttpRequest, name: &str) -> Option<&'a str> {
    let mut values = request.headers.iter().filter(|(key, _)| key.eq_ignore_ascii_case(name));
    let value = values.next();
    assert!(values.next().is_none(), "A request must not contain duplicate credential headers");
    value.map(|(_, value)| value.as_str())
}

fn request_cookies(request: &HttpRequest) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    if let Some(value) = header(request, "cookie") {
        for pair in value.split(';') {
            let (name, value) = pair.trim().split_once('=').expect("Cookie fixture header must contain name/value pairs");
            assert!(result.insert(name.to_owned(), value.to_owned()).is_none(), "Cookie names must be unique");
        }
    }
    result
}

fn assert_no_credentials(request: &HttpRequest) {
    assert!(header(request, "cookie").is_none());
    assert!(header(request, "x-xsrf-token").is_none());
}

fn response(status: u16, body: Value, cookies: &[&str]) -> HttpResponse {
    HttpResponse { status, headers: cookies.iter().map(|value| ("Set-Cookie".into(), (*value).into())).collect(),
        body: serde_json::to_vec(&body).unwrap() }
}

fn success(data: Value) -> HttpResponse {
    response(200, json!({"code": 0, "data": data}), &[])
}

fn step(inspect: impl FnOnce(&HttpRequest) + Send + 'static, response: HttpResponse) -> Step {
    Step { inspect: Box::new(inspect), response, gate: None }
}

fn business_step(endpoint: &'static str, data: Value) -> Step {
    step(move |request| {
        assert_eq!(request.method, "POST");
        let url = url::Url::parse(&request.url).unwrap();
        assert_eq!(url.host_str(), Some(MANGA_HOST));
        assert!(url.path().ends_with(&format!("/{endpoint}")));
        assert_eq!(request_cookies(request), expected_cookies());
        assert_eq!(header(request, "x-xsrf-token"), Some("synthetic+xsrf+literal"));
    }, success(data))
}

fn cookie_step(cookies: &[&str]) -> Step {
    step(|request| {
        assert_eq!(request_cookies(request), expected_cookies());
    }, response(200, json!({"code": 0, "data": {"list": []}}), cookies))
}

fn fixture(session: AuthSession, steps: Vec<Step>) -> (BiliClient, Arc<FixtureTransport>, Arc<FixtureCrypto>) {
    let transport = Arc::new(FixtureTransport { steps: Mutex::new(steps.into()), calls: AtomicUsize::new(0) });
    let crypto = Arc::new(FixtureCrypto::default());
    let client = BiliClient::with_adapters(Some(session), Path::new("unused-cookie-fixture-data"), transport.clone(), crypto.clone());
    (client, transport, crypto)
}

fn durable_fixture(directory: &Path, steps: Vec<Step>) -> (BiliClient, Arc<FixtureTransport>, Arc<FixtureCrypto>) {
    let session = fixture_session();
    let file = SessionFile::new(directory);
    file.save(&session).unwrap();
    let (client, transport, crypto) = fixture(session, steps);
    (client.with_session_file(file), transport, crypto)
}

fn assert_saved_session(directory: &Path, client: &BiliClient) -> Option<AuthSession> {
    let saved = SessionFile::new(directory).load().unwrap();
    assert_eq!(saved, client.session());
    saved
}

fn purchase() -> PurchaseRequest {
    PurchaseRequest { scope: PurchaseScope::Single { episode_id: "21".into(), comic_id: Some("12".into()) },
        method: PurchaseMethod::Currency, pay_amount: Some(2.0), coupon_id: None, coupon_ids: Vec::new(),
        free_gold_card_id: None, free_gold_amount: None }
}

#[tokio::test]
async fn business_reads_and_writes_send_the_exact_allowed_cookie_set() {
    let mut steps = vec![
        business_step("ListFavorite", json!({"list": []})),
        business_step("ListHistory", json!({"list": []})),
        business_step("Search", json!({"list": []})),
        business_step("ComicDetail", json!({"id": 12, "title": "Synthetic Comic", "ep_list": []})),
        business_step("GetImageIndex", json!({"images": [{"path": "/synthetic/page", "x": 1, "y": 1}]})),
        business_step("GetWallet", json!({})),
        business_step("GetEpisodeBuyInfo", json!({"ep_id": 21, "comic_id": 12})),
        business_step("GetDiscountList", json!({})),
        business_step("CalDiscountPrice", json!({})),
        business_step("GetComicFreeGoldCard", json!({})),
        business_step("AddFavorite", Value::Null),
        business_step("DeleteFavorite", Value::Null),
        business_step("AddHistory", Value::Null),
        business_step("BuyEpisode", Value::Null),
        business_step("GetPayConfig", json!({"pay_amount_ranges": []})),
        business_step("GetPayOrders", json!([])),
        business_step("GetPayConfig", json!({"pay_amount_ranges": [{"pay_amount": "5.00", "gold_amount": 500}]})),
        business_step("CreateOrder", json!({"pay_params": "{\"orderId\":\"9001\",\"codeUrl\":\"https://pay.bilibili.com/synthetic-fixture\"}"})),
    ];
    #[cfg(target_os = "linux")]
    steps.push(business_step("ImageToken", json!([{"url": "https://i0.hdslb.com/synthetic-page", "token": "synthetic-token"}])));
    let (client, transport, crypto) = fixture(fixture_session(), steps);

    client.list_favorites(1, 20).await.unwrap();
    client.list_history(1, 20).await.unwrap();
    client.search("synthetic", 1, 20).await.unwrap();
    client.comic_detail("12").await.unwrap();
    client.image_index("21").await.unwrap();
    client.wallet().await.unwrap();
    client.purchase_info("21", QuoteScope::Basic).await.unwrap();
    client.discount_list("12", 1, json!([2.0])).await.unwrap();
    client.discount_price("31", json!([2.0])).await.unwrap();
    client.free_gold_card_info("12", "21", 1, 0).await.unwrap();
    client.set_favorite("12", true).await.unwrap();
    client.set_favorite("12", false).await.unwrap();
    client.add_history("12", "21").await.unwrap();
    client.buy_episode(&purchase()).await.unwrap();
    client.recharge_config().await.unwrap();
    client.recharge_history_for(1, 20, 2026, 0).await.unwrap();
    client.create_recharge_order(500, None).await.unwrap();
    #[cfg(target_os = "linux")]
    client.prepare_image_batch(&["/synthetic/page".into()]).await.unwrap();

    assert!(transport.steps.lock().unwrap().is_empty());
    let signed = crypto.signed_devices.lock().unwrap();
    assert!(signed.iter().all(|(_, device)| device == "synthetic-buvid3"));
    assert!(signed.iter().any(|(endpoint, _)| endpoint == "ComicDetail"));
    assert!(signed.iter().any(|(endpoint, _)| endpoint == "GetImageIndex"));
}

#[tokio::test]
async fn request_headers_and_signing_ignore_cookies_scoped_to_other_hosts() {
    let session = AuthSession::from_cookie_text(&json!([
        {"name": "SESSDATA", "value": "synthetic-session", "domain": ".bilibili.com", "path": "/", "hostOnly": false},
        {"name": "bili_jct", "value": "synthetic-csrf", "domain": ".bilibili.com", "path": "/", "hostOnly": false},
        {"name": "buvid3", "value": "passport-device", "domain": "passport.bilibili.com", "path": "/", "hostOnly": true},
        {"name": "buvid4", "value": "manga-device", "domain": ".bilibili.com", "path": "/", "hostOnly": false},
        {"name": "XSRF-TOKEN", "value": "passport%2Bxsrf", "domain": "passport.bilibili.com", "path": "/", "hostOnly": true},
    ]).to_string()).unwrap();
    let expected: BTreeMap<_, _> = [
        ("SESSDATA".to_owned(), "synthetic-session".to_owned()),
        ("bili_jct".to_owned(), "synthetic-csrf".to_owned()),
        ("buvid4".to_owned(), "manga-device".to_owned()),
    ].into_iter().collect();
    let request = step(move |request| {
        assert_eq!(request_cookies(request), expected);
        assert!(header(request, "x-xsrf-token").is_none());
    }, success(json!({"id": 12, "ep_list": []})));
    let (client, _, crypto) = fixture(session, vec![request]);
    client.comic_detail("12").await.unwrap();
    assert_eq!(*crypto.signed_devices.lock().unwrap(), vec![("ComicDetail".into(), "manga-device".into())]);
}

#[tokio::test]
async fn authenticated_business_requests_require_a_session_cookie_for_the_destination() {
    let session = AuthSession::from_cookie_text(&json!([
        {"name": "SESSDATA", "value": "passport-only-session", "domain": "passport.bilibili.com", "path": "/", "hostOnly": true},
        {"name": "bili_jct", "value": "synthetic-csrf", "domain": ".bilibili.com", "path": "/", "hostOnly": false},
    ]).to_string()).unwrap();
    let (client, transport, _) = fixture(session, Vec::new());
    let error = client.list_favorites(1, 20).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::Authentication);
    assert_eq!(error.code, "session_required");
    assert_eq!(error.transmitted, Some(false));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn guest_category_requests_and_cover_downloads_exclude_account_credentials() {
    let device = step(|request| {
        assert_eq!(request.method, "GET");
        assert!(request.url.ends_with("/ductape/buvid"));
        assert_no_credentials(request);
    }, HttpResponse { status: 200, headers: vec![("Set-Cookie".into(), concat!(
        "XSRF-TOKEN=guest%2Bxsrf; Domain=manga.bilibili.com; Path=/, ",
        "buvid3=SYNTHETIC_GUEST; Domain=.bilibili.com; Path=/; Secure, ",
        "SESSDATA=unexpected-guest-session; Domain=.bilibili.com; Path=/"
    ).into())], body: Vec::new() });
    let category = step(|request| {
        assert!(request.url.contains("/ClassPage?"));
        assert_eq!(header(request, "cookie"), Some("buvid3=SYNTHETIC_GUEST"));
        assert!(header(request, "x-xsrf-token").is_none());
    }, success(json!({"list": []})));
    let cover = step(|request| {
        assert_eq!(request.method, "GET");
        assert_eq!(request.url, "https://i0.hdslb.com/synthetic-cover.png");
        assert_no_credentials(request);
    }, HttpResponse { status: 503, headers: Vec::new(), body: Vec::new() });
    let original = fixture_session();
    let (client, transport, crypto) = fixture(original.clone(), vec![device, category, cover]);
    client.category_page("1", 0, 1).await.unwrap();
    assert_eq!(client.download_cover("https://i0.hdslb.com/synthetic-cover.png").await.unwrap_err().kind, ErrorKind::Http);
    assert_eq!(*crypto.signed_devices.lock().unwrap(), vec![("ClassPage".into(), "SYNTHETIC_GUEST".into())]);
    assert_eq!(client.session().unwrap(), original);
    assert!(transport.steps.lock().unwrap().is_empty());
}

#[tokio::test]
async fn rejected_http_and_business_responses_still_persist_response_cookies() {
    let cases = [
        (200, json!({"code": 7, "data": {}}), ErrorKind::Business),
        (403, json!({"code": 0, "data": {}}), ErrorKind::Http),
        (503, json!({"code": 0, "data": {}}), ErrorKind::Http),
    ];
    for (status, body, expected_kind) in cases {
        let directory = tempfile::tempdir().unwrap();
        let request = step(|_| {}, response(status, body, &[
            "SESSDATA=rotated-synthetic-session; Domain=.bilibili.com; Path=/; Secure",
            "bili_jct=rotated-synthetic-csrf; Domain=.bilibili.com; Path=/; Secure",
            "XSRF-TOKEN=rotated%2Bxsrf; Domain=manga.bilibili.com; Path=/; Secure",
        ]));
        let (client, transport, _) = durable_fixture(directory.path(), vec![request]);
        assert_eq!(client.list_favorites(1, 20).await.unwrap_err().kind, expected_kind);
        let saved = assert_saved_session(directory.path(), &client).unwrap();
        assert_eq!(saved.cookie_value_for(MANGA_HOST, "SESSDATA"), Some("rotated-synthetic-session"));
        assert_eq!(saved.cookie_value_for(MANGA_HOST, "bili_jct"), Some("rotated-synthetic-csrf"));
        assert_eq!(saved.cookie_value_for(MANGA_HOST, "XSRF-TOKEN"), Some("rotated%2Bxsrf"));
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn ordinary_cookie_rotation_is_used_by_a_reconstructed_client() {
    let directory = tempfile::tempdir().unwrap();
    let rotated = cookie_step(&[
        "SESSDATA=restart-synthetic-session; Domain=.bilibili.com; Path=/; Secure",
        "XSRF-TOKEN=restart%2Bxsrf; Domain=manga.bilibili.com; Path=/; Secure",
    ]);
    let (client, _, _) = durable_fixture(directory.path(), vec![rotated]);
    client.list_favorites(1, 20).await.unwrap();
    let saved = assert_saved_session(directory.path(), &client).unwrap();
    drop(client);

    let restarted = step(|request| {
        let mut expected = expected_cookies();
        expected.insert("SESSDATA".into(), "restart-synthetic-session".into());
        expected.insert("XSRF-TOKEN".into(), "restart%2Bxsrf".into());
        assert_eq!(request_cookies(request), expected);
        assert_eq!(header(request, "x-xsrf-token"), Some("restart+xsrf"));
    }, success(json!({"list": []})));
    let (reconstructed, transport, _) = fixture(saved, vec![restarted]);
    let reconstructed = reconstructed.with_session_file(SessionFile::new(directory.path()));
    reconstructed.list_favorites(1, 20).await.unwrap();
    assert_saved_session(directory.path(), &reconstructed);
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn concurrent_disjoint_cookie_responses_merge_before_persistence() {
    let directory = tempfile::tempdir().unwrap();
    let first_gate = Arc::new(ResponseGate::default());
    let second_gate = Arc::new(ResponseGate::default());
    let mut first = cookie_step(&["XSRF-TOKEN=first%2Bxsrf; Domain=manga.bilibili.com; Path=/"]);
    first.gate = Some(first_gate.clone());
    let mut second = cookie_step(&["buvid_fp=second-fingerprint; Domain=.bilibili.com; Path=/"]);
    second.gate = Some(second_gate.clone());
    let (client, transport, _) = durable_fixture(directory.path(), vec![first, second]);
    let first_client = client.clone();
    let first_request = tokio::spawn(async move { first_client.list_favorites(1, 20).await });
    first_gate.entered.notified().await;
    let second_client = client.clone();
    let second_request = tokio::spawn(async move { second_client.list_history(1, 20).await });
    second_gate.entered.notified().await;

    first_gate.release.notify_one();
    first_request.await.unwrap().unwrap();
    second_gate.release.notify_one();
    second_request.await.unwrap().unwrap();
    let saved = assert_saved_session(directory.path(), &client).unwrap();
    assert_eq!(saved.cookie_value_for(MANGA_HOST, "XSRF-TOKEN"), Some("first%2Bxsrf"));
    assert_eq!(saved.cookie_value_for(MANGA_HOST, "buvid_fp"), Some("second-fingerprint"));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_late_response_cannot_overwrite_a_cookie_rotated_by_another_response() {
    let directory = tempfile::tempdir().unwrap();
    let gate = Arc::new(ResponseGate::default());
    let mut older = cookie_step(&["XSRF-TOKEN=stale%2Bxsrf; Domain=manga.bilibili.com; Path=/"]);
    older.gate = Some(gate.clone());
    let newer = cookie_step(&["XSRF-TOKEN=newer%2Bxsrf; Domain=manga.bilibili.com; Path=/"]);
    let (client, _, _) = durable_fixture(directory.path(), vec![older, newer]);
    let delayed_client = client.clone();
    let delayed = tokio::spawn(async move { delayed_client.list_favorites(1, 20).await });
    gate.entered.notified().await;
    client.list_history(1, 20).await.unwrap();
    let before_late_response = std::fs::read(SessionFile::new(directory.path()).path()).unwrap();
    gate.release.notify_one();
    delayed.await.unwrap().unwrap();

    let saved = assert_saved_session(directory.path(), &client).unwrap();
    assert_eq!(saved.cookie_value_for(MANGA_HOST, "XSRF-TOKEN"), Some("newer%2Bxsrf"));
    assert_eq!(std::fs::read(SessionFile::new(directory.path()).path()).unwrap(), before_late_response);
}

#[tokio::test]
async fn late_cookie_responses_cannot_restore_replaced_or_logged_out_credentials() {
    let original = fixture_session();
    let mut rotated = original.clone();
    rotated.apply_set_cookie(&[("Set-Cookie".into(), "SESSDATA=new-synthetic-session; Domain=.bilibili.com; Path=/".into())],
        MANGA_HOST, 1).unwrap();
    for replacement in [Some(rotated), None, Some(original)] {
        let directory = tempfile::tempdir().unwrap();
        let gate = Arc::new(ResponseGate::default());
        let mut stale = cookie_step(&[
            "SESSDATA=retired-synthetic-session; Domain=.bilibili.com; Path=/",
            "XSRF-TOKEN=retired%2Bxsrf; Domain=manga.bilibili.com; Path=/",
        ]);
        stale.gate = Some(gate.clone());
        let (client, transport, _) = durable_fixture(directory.path(), vec![stale]);
        let delayed_client = client.clone();
        let delayed = tokio::spawn(async move { delayed_client.list_favorites(1, 20).await });
        gate.entered.notified().await;
        client.replace_session(replacement.clone()).unwrap();
        gate.release.notify_one();
        let error = delayed.await.unwrap().unwrap_err();
        assert!(matches!(error.kind, ErrorKind::Cancelled | ErrorKind::AccountMismatch));
        assert_eq!(client.session(), replacement);
        assert_saved_session(directory.path(), &client);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn current_business_authentication_rejections_clear_memory_and_persistence() {
    for rejected in [
        response(401, json!({}), &[]),
        response(200, json!({"code": -101, "data": {}}), &[]),
        response(200, json!({"code": 401, "data": {}}), &[]),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let (client, transport, _) = durable_fixture(directory.path(), vec![step(|_| {}, rejected)]);
        let error = client.list_favorites(1, 20).await.unwrap_err();
        assert_eq!(error.kind, ErrorKind::Authentication);
        assert!(assert_saved_session(directory.path(), &client).is_none());
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn a_late_authentication_rejection_cannot_delete_rotated_credentials() {
    for rejected in [response(401, json!({}), &[]), response(200, json!({"code": -101, "data": {}}), &[])] {
        let directory = tempfile::tempdir().unwrap();
        let gate = Arc::new(ResponseGate::default());
        let mut stale = step(|_| {}, rejected);
        stale.gate = Some(gate.clone());
        let (client, _, _) = durable_fixture(directory.path(), vec![stale]);
        let delayed_client = client.clone();
        let delayed = tokio::spawn(async move { delayed_client.list_favorites(1, 20).await });
        gate.entered.notified().await;
        let mut rotated = fixture_session();
        rotated.apply_set_cookie(&[("Set-Cookie".into(), "SESSDATA=rotated-synthetic-session; Domain=.bilibili.com; Path=/".into())],
            MANGA_HOST, 1).unwrap();
        client.replace_session(Some(rotated.clone())).unwrap();
        gate.release.notify_one();
        delayed.await.unwrap().unwrap_err();
        assert_eq!(client.session(), Some(rotated));
        assert_saved_session(directory.path(), &client);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn failed_cookie_persistence_keeps_the_saved_session_and_does_not_replay_a_mutation() {
    use std::{fs, os::unix::fs::symlink};

    let directory = tempfile::tempdir().unwrap();
    let first = cookie_step(&["XSRF-TOKEN=unpublished-read; Domain=manga.bilibili.com; Path=/"]);
    let mutation = step(|request| {
        assert!(request.url.contains("/BuyEpisode?"));
        assert_eq!(request_cookies(request), expected_cookies());
    }, response(200, json!({"code": 0, "data": null}), &[
        "XSRF-TOKEN=unpublished-mutation; Domain=manga.bilibili.com; Path=/",
    ]));
    let (client, transport, _) = durable_fixture(directory.path(), vec![first, mutation]);
    let original = client.session().unwrap();
    let file = SessionFile::new(directory.path());
    let original_bytes = fs::read(file.path()).unwrap();
    let private = directory.path().join("private");
    let preserved = directory.path().join("preserved-private");
    fs::rename(&private, &preserved).unwrap();
    symlink(&preserved, &private).unwrap();

    let error = client.list_favorites(1, 20).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::Storage);
    assert!(!error.retryable);
    assert_eq!(client.session(), Some(original.clone()));
    assert_eq!(fs::read(preserved.join("session.json")).unwrap(), original_bytes);
    assert_eq!(client.buy_episode(&purchase()).await.unwrap(), json!({"accepted": true, "code": 0}));
    assert_eq!(client.session(), Some(original));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 2);
    assert!(transport.steps.lock().unwrap().is_empty());
    assert_eq!(fs::read(preserved.join("session.json")).unwrap(), original_bytes);

    fs::remove_file(&private).unwrap();
    fs::rename(&preserved, &private).unwrap();
    assert_saved_session(directory.path(), &client);
}

#[cfg(unix)]
#[tokio::test]
async fn published_cookie_sync_failure_hides_the_session_until_an_explicit_login() {
    let directory = tempfile::tempdir().unwrap();
    let original = fixture_session();
    let file = SessionFile::new(directory.path());
    file.save(&original).unwrap();
    let published = cookie_step(&[
        "SESSDATA=published-synthetic-session; Domain=.bilibili.com; Path=/",
        "XSRF-TOKEN=published%2Bxsrf; Domain=manga.bilibili.com; Path=/",
    ]);
    let recovered_request = step(|request| {
        let mut expected = expected_cookies();
        expected.insert("SESSDATA".into(), "recovered-synthetic-session".into());
        expected.insert("XSRF-TOKEN".into(), "recovered%2Bxsrf".into());
        assert_eq!(request_cookies(request), expected);
        assert_eq!(header(request, "x-xsrf-token"), Some("recovered+xsrf"));
    }, success(json!({"list": []})));
    let (client, transport, _) = fixture(original.clone(), vec![published, recovered_request]);
    let client = client.with_session_file(file.clone());
    let shared = client.clone();
    let previous_epoch = client.credential_epoch();
    let previous_account_epoch = client.account_epoch.load(Ordering::SeqCst);
    let queued_request = HttpRequest {
        method: "POST".into(), url: "https://manga.bilibili.com/twirp/bookshelf.v1.Bookshelf/ListHistory".into(),
        headers: vec![("cookie".into(), original.cookie_header(MANGA_HOST).unwrap())],
        body: Vec::new(), max_bytes: 1024,
    };

    file.fail_next_directory_sync();
    let error = shared.list_favorites(1, 20).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::Storage);
    assert_eq!(error.code, "session_publish_sync_unknown");
    let saved = file.load().unwrap().unwrap();
    assert_eq!(saved.cookie_value_for(MANGA_HOST, "SESSDATA"), Some("published-synthetic-session"));
    assert_eq!(saved.cookie_value_for(MANGA_HOST, "XSRF-TOKEN"), Some("published%2Bxsrf"));
    assert!(client.session().is_none());
    assert!(shared.session().is_none());
    assert!(client.credential_context().is_none());
    assert!(client.credential_epoch() > previous_epoch);
    assert!(client.account_epoch.load(Ordering::SeqCst) > previous_account_epoch);
    let published_bytes = std::fs::read(file.path()).unwrap();

    let error = client.list_history(1, 20).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::Authentication);
    assert_eq!(error.transmitted, Some(false));
    let error = shared.send(queued_request).await.unwrap_err();
    assert_eq!(error.code, "session_storage_outcome_unresolved");
    assert_eq!(error.transmitted, Some(false));
    let error = client.replace_session_if_epoch(client.credential_epoch(), Some(original)).unwrap_err();
    assert_eq!(error.code, "session_storage_outcome_unresolved");
    assert_eq!(std::fs::read(file.path()).unwrap(), published_bytes);
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);

    let mut recovered = saved;
    recovered.apply_set_cookie(&[
        ("Set-Cookie".into(), "SESSDATA=recovered-synthetic-session; Domain=.bilibili.com; Path=/".into()),
        ("Set-Cookie".into(), "XSRF-TOKEN=recovered%2Bxsrf; Domain=manga.bilibili.com; Path=/".into()),
    ], MANGA_HOST, 1).unwrap();
    client.replace_session(Some(recovered.clone())).unwrap();
    assert_eq!(shared.session(), Some(recovered));
    shared.list_favorites(1, 20).await.unwrap();
    assert_saved_session(directory.path(), &client);
    assert_eq!(transport.calls.load(Ordering::SeqCst), 2);
    assert!(transport.steps.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn cleared_session_sync_failure_cannot_be_undone_by_an_in_flight_response() {
    let directory = tempfile::tempdir().unwrap();
    let original = fixture_session();
    let file = SessionFile::new(directory.path());
    file.save(&original).unwrap();
    let gate = Arc::new(ResponseGate::default());
    let mut late = step(|request| {
        assert!(request.url.contains("/AddHistory?"));
        assert_eq!(request_cookies(request), expected_cookies());
    }, response(200, json!({"code": 0, "data": null}), &[
        "SESSDATA=retired-synthetic-session; Domain=.bilibili.com; Path=/",
        "XSRF-TOKEN=retired%2Bxsrf; Domain=manga.bilibili.com; Path=/",
    ]));
    late.gate = Some(gate.clone());
    let (client, transport, _) = fixture(original.clone(), vec![late]);
    let client = client.with_session_file(file.clone());
    let delayed_client = client.clone();
    let delayed = tokio::spawn(async move { delayed_client.add_history("12", "21").await });
    gate.entered.notified().await;
    let previous_epoch = client.credential_epoch();

    file.fail_next_directory_sync();
    let error = client.replace_session(None).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Storage);
    assert_eq!(error.code, "session_clear_sync_unknown");
    assert!(file.load().unwrap().is_none());
    assert!(client.session().is_none());
    assert!(client.credential_context().is_none());
    assert!(client.credential_epoch() > previous_epoch);
    gate.release.notify_one();
    delayed.await.unwrap().unwrap();
    assert!(file.load().unwrap().is_none());
    assert!(client.session().is_none());

    let error = client.replace_session_if_epoch(client.credential_epoch(), Some(original)).unwrap_err();
    assert_eq!(error.code, "session_storage_outcome_unresolved");
    let error = client.list_favorites(1, 20).await.unwrap_err();
    assert_eq!(error.transmitted, Some(false));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    assert!(file.load().unwrap().is_none());

    client.replace_session(None).unwrap();
    assert!(assert_saved_session(directory.path(), &client).is_none());
    assert!(client.replace_session_if_epoch(client.credential_epoch(), None).unwrap());
    assert!(file.load().unwrap().is_none());
    assert!(transport.steps.lock().unwrap().is_empty());
}

#[tokio::test]
async fn encrypted_mutation_decode_failures_remain_transmitted_unknown_outcomes() {
    for endpoint in ["BuyEpisode", "CreateOrder"] {
        let mut steps = Vec::new();
        if endpoint == "CreateOrder" {
            steps.push(business_step("GetPayConfig", json!({
                "pay_amount_ranges": [{"pay_amount": "5.00", "gold_amount": 500}],
            })));
        }
        steps.push(step(move |request| {
            assert!(request.url.contains(&format!("/{endpoint}?")));
            assert_eq!(request_cookies(request), expected_cookies());
        }, response(200, json!({"code": 0, "bytesData": "synthetic-encrypted-envelope"}), &[])));
        let (mut client, transport, _) = fixture(fixture_session(), steps);
        let crypto = Arc::new(FixtureCrypto {
            decode_failure: Some(SourceError::capability("synthetic_response_decoder_unavailable")),
            ..Default::default()
        });
        client.crypto = crypto.clone();

        let error = if endpoint == "BuyEpisode" { client.buy_episode(&purchase()).await.unwrap_err() }
            else { client.create_recharge_order(500, None).await.unwrap_err() };
        assert_eq!(error.kind, ErrorKind::PurchaseUnknown);
        assert_eq!(error.code, if endpoint == "BuyEpisode" { "purchase_outcome_unknown" } else { "recharge_outcome_unknown" });
        assert_eq!(error.transmitted, Some(true));
        assert!(!error.definitive);
        assert!(!error.retryable);
        assert!(error.business_code.is_none());
        assert_eq!(crypto.decode_calls.load(Ordering::SeqCst), 1);
        assert_eq!(transport.calls.load(Ordering::SeqCst), if endpoint == "CreateOrder" { 2 } else { 1 });
        assert!(transport.steps.lock().unwrap().is_empty());
    }
}
