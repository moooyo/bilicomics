//! Offline service fixtures exercise the production request and acquisition pipeline.

use super::*;
use super::crypto::{ImageKey, NativeCrypto, ProtocolCrypto, RequestContext, Signature};
use super::transport::{HttpRequest, HttpResponse, ResponseFuture, Transport};
use serde_json::{Value, json};
use std::{collections::VecDeque, io::Cursor, path::Path, sync::{Arc, Mutex}};

struct Step {
    inspect: Box<dyn FnOnce(&HttpRequest) + Send>,
    response: HttpResponse,
}

struct FixtureTransport { steps: Mutex<VecDeque<Step>> }

impl Transport for FixtureTransport {
    fn send(&self, request: HttpRequest) -> ResponseFuture<'_> {
        let step = self.steps.lock().unwrap().pop_front().expect("Unexpected fixture request");
        Box::pin(async move { (step.inspect)(&request); Ok(step.response) })
    }
}

struct FixtureCrypto;

impl ProtocolCrypto for FixtureCrypto {
    fn sign(&self, context: &RequestContext) -> Result<Signature> {
        assert_eq!(context.sign_query, "device=pc&platform=web&nov=27&eot=812");
        assert_eq!(context.platform, "web");
        assert!(serde_json::from_str::<Value>(&context.body).unwrap().is_object());
        Ok(Signature { ultra_sign: "fixture-sign".into(), data_sn: "fixture-sn".into() })
    }
    fn decode_response(&self, _: &RequestContext, _: Value) -> Result<Value> {
        Err(SourceError::protocol("unexpected_fixture_response_crypto"))
    }
    fn new_image_key(&self) -> Result<ImageKey> { NativeCrypto::new(Path::new("unused-fixture-assets")).new_image_key() }
    fn decrypt_image(&self, _: &ImageKey, _: &str, _: &[u8]) -> Result<Vec<u8>> {
        Err(SourceError::protocol("unexpected_fixture_image_crypto"))
    }
}

fn fixture_session() -> AuthSession {
    AuthSession::from_cookie_text("SESSDATA=fixture-session; bili_jct=fixture-csrf; buvid3=fixture-buvid; XSRF-TOKEN=fixture%2Bxsrf").unwrap()
}

fn header<'a>(request: &'a HttpRequest, name: &str) -> Option<&'a str> {
    request.headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_,value)| value.as_str())
}

fn step(inspect: impl FnOnce(&HttpRequest) + Send + 'static, data: Value) -> Step {
    Step { inspect: Box::new(inspect), response: HttpResponse { status: 200, headers: Vec::new(),
        body: serde_json::to_vec(&json!({"code":0,"data":data})).unwrap() } }
}

fn fixture(steps: Vec<Step>) -> (BiliClient, Arc<FixtureTransport>) {
    let transport = Arc::new(FixtureTransport { steps: Mutex::new(steps.into()) });
    let client = BiliClient::with_adapters(Some(fixture_session()), Path::new("unused-fixture-data"), transport.clone(), Arc::new(FixtureCrypto));
    (client, transport)
}

#[tokio::test]
async fn catalog_serialization_and_noncredential_cookie_updates_use_the_real_client() {
    let mut favorite = step(|request| {
        assert!(request.url.contains("/bookshelf.v1.Bookshelf/ListFavorite?device=pc&platform=web&nov=27&a=810"));
        assert_eq!(header(request,"x-xsrf-token"), Some("fixture+xsrf"));
        assert!(header(request,"cookie").unwrap().contains("SESSDATA=fixture-session"));
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body, json!({"page_num":1,"page_size":20,"order":1,"wait_free":0,"time_limit_free":0,"type":0,"from":"web","source":"web"}));
    }, json!({"list":[{"comic_id":12,"title":"Fixture Comic","vertical_cover":"https://i0.hdslb.com/cover.jpg"}]}));
    favorite.response.headers.push(("Set-Cookie".into(),"XSRF-TOKEN=rotated%2Bxsrf; Domain=manga.bilibili.com; Path=/; Secure".into()));
    let (client, transport) = fixture(vec![favorite]);
    let comics = client.list_favorites(1,20).await.unwrap();
    assert_eq!(comics[0].id,"12"); assert!(comics[0].favorite);
    assert_eq!(client.session().unwrap().cookie_value("XSRF-TOKEN"),Some("rotated%2Bxsrf"));
    assert!(transport.steps.lock().unwrap().is_empty());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn ordered_index_tokens_and_plain_image_flag_complete_without_cookie_leakage() {
    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::new_luma8(2,3).write_to(&mut png,image::ImageFormat::Png).unwrap();
    let image_bytes = png.into_inner();
    let index = step(|request| {
        assert!(request.url.contains("a=810&ultra_sign=fixture-sign"));
        assert_eq!(header(request,"x-bili-data-sn"),Some("fixture-sn"));
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["ep_id"],21); assert!(body["m2"].as_str().unwrap().starts_with("error:"));
    },json!({"images":[{"path":"/fixture/page","x":2,"y":3}]}));
    let tokens = step(|request| {
        assert!(request.url.contains("/ImageToken?"));
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["urls"],"[\"/fixture/page\"]");
        assert_eq!(body["m1"].as_str().unwrap().len(),88);
    },json!([{"complete_url":"https://i0.hdslb.com/page?token=fixture%2Bvalue","hit_encrpyt":true}]));
    let image = Step { inspect: Box::new(|request| {
        assert_eq!(request.url,"https://i0.hdslb.com/page?token=fixture%2Bvalue&code=DanmakuInfo");
        assert!(header(request,"cookie").is_none()); assert!(header(request,"x-xsrf-token").is_none());
        assert_eq!(request.max_bytes,16*1024*1024);
    }), response: HttpResponse { status:200, headers:vec![("Content-Type".into(),"image/png".into())], body:image_bytes } };
    let (client, transport) = fixture(vec![index,tokens,image]);
    let index = client.image_index("21").await.unwrap();
    let persisted = serde_json::to_string(&index).unwrap();
    assert!(!persisted.contains("/fixture/page")); assert!(!format!("{index:?}").contains("/fixture/page"));
    let result = client.download_page(&index,0).await.unwrap();
    assert_eq!((result.width,result.height,result.verification),(2,3,"decoded_v1"));
    assert!(transport.steps.lock().unwrap().is_empty());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn an_account_switch_cancels_a_prepared_batch_before_any_cdn_request() {
    let tokens = step(|_|{},json!([{"url":"https://i0.hdslb.com/page","token":"fixture"}]));
    let (client, transport) = fixture(vec![tokens]);
    let batch = client.prepare_image_batch(&["/fixture/page".into()]).await.unwrap();
    client.replace_session(None).unwrap();
    assert_eq!(client.download_image(&batch,0).await.unwrap_err().kind,ErrorKind::Cancelled);
    assert!(transport.steps.lock().unwrap().is_empty());
}

#[tokio::test]
async fn category_metadata_remains_anonymous_even_with_an_active_session() {
    let metadata = step(|request| {
        assert!(request.url.contains("/AllLabel?")); assert!(header(request,"cookie").is_none());
        assert!(header(request,"x-xsrf-token").is_none());
    },json!({"styles":[{"id":1,"name":"Fixture"}],"orders":[{"id":0,"name":"Popular"},{"id":3,"name":"Latest"}]}));
    let (client, transport) = fixture(vec![metadata]);
    let metadata = client.categories().await.unwrap();
    assert_eq!(metadata.orders[1].id,3);
    assert!(transport.steps.lock().unwrap().is_empty());
}

#[tokio::test]
async fn offline_client_blocks_read_write_and_protocol_paths_without_creating_assets() {
    let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let data_dir = std::env::temp_dir().join(format!("bilicomics-offline-{}-{unique}", std::process::id()));
    let client = BiliClient::offline(&data_dir).unwrap();
    assert!(client.session().is_none());
    let public_read = client.search("fixture", 1, 20).await.unwrap_err();
    client.replace_session(Some(fixture_session())).unwrap();
    let purchase = PurchaseRequest { scope: PurchaseScope::Single { episode_id: "21".into(), comic_id: Some("12".into()) },
        method: PurchaseMethod::Currency, pay_amount: Some(10.0), coupon_id: None, coupon_ids: Vec::new(),
        free_gold_card_id: None, free_gold_amount: None };
    let errors = vec![public_read,
        client.begin_qr().await.unwrap_err(),
        client.wallet().await.unwrap_err(),
        client.list_favorites(1, 20).await.unwrap_err(),
        client.comic_detail("12").await.unwrap_err(),
        client.image_index("21").await.unwrap_err(),
        client.prepare_image_batch(&["/fixture/page".into()]).await.unwrap_err(),
        client.download_cover("https://i0.hdslb.com/fixture.jpg").await.unwrap_err(),
        client.set_favorite("12", true).await.unwrap_err(),
        client.add_history("12", "21").await.unwrap_err(),
        client.buy_episode(&purchase).await.unwrap_err(),
        client.create_recharge_order(100, None).await.unwrap_err(),
    ];
    for error in errors {
        assert_eq!(error.code, "network_disabled");
        assert_eq!(error.kind, ErrorKind::Network);
        assert_eq!(error.transmitted, Some(false));
        assert!(error.definitive);
        assert!(!error.retryable);
    }
    assert!(!data_dir.exists(), "Offline source operations must not acquire or stage assets");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn an_account_bound_clone_rejects_queued_and_in_flight_acquisition_after_a_switch() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let sent = Arc::new(AtomicUsize::new(0));
    let count = sent.clone();
    let favorites = step(move |request| {
        count.fetch_add(1, Ordering::SeqCst);
        assert!(header(request, "cookie").unwrap().contains("SESSDATA=rotated-a"));
    }, json!({"list":[]}));
    let count = sent.clone();
    let tokens = step(move |request| {
        count.fetch_add(1, Ordering::SeqCst);
        assert!(header(request, "cookie").unwrap().contains("SESSDATA=rotated-a"));
    }, json!([{"url":"https://i0.hdslb.com/fixture.jpg","token":"fixture-a-token"}]));
    let (client, transport) = fixture(vec![favorites, tokens]);
    let account_a = fixture_session().with_identity(AccountIdentity { id:"42".into(), name:"Fixture A".into(), avatar_url:String::new() }, 1).unwrap();
    client.replace_session(Some(account_a.clone())).unwrap();
    let bound = client.bound_to_account("bili_42").unwrap();

    // Binding follows legitimate credentials for the same account rather than freezing them.
    let mut rotated = account_a.clone();
    rotated.cookies.insert("SESSDATA".into(), "rotated-a".into());
    rotated.credential_generation += 1;
    client.replace_session(Some(rotated.clone())).unwrap();
    assert!(bound.list_favorites(1,20).await.unwrap().is_empty());
    let paths = vec!["/fixture/page".into()];
    let batch = bound.prepare_image_batch(&paths).await.unwrap();
    assert_eq!(sent.load(Ordering::SeqCst), 2);

    let catalog = bound.comic_detail("12");
    let index = bound.image_index("21");
    let tokens = bound.prepare_image_batch(&paths);
    let cdn = bound.download_image(&batch,0);
    let cover = bound.download_cover("https://i0.hdslb.com/fixture.jpg");
    let replacement = AuthSession::from_cookie_text("SESSDATA=fixture-b; bili_jct=fixture-b-csrf; DedeUserID=43")
        .unwrap().with_identity(AccountIdentity { id:"43".into(), name:"Fixture B".into(), avatar_url:String::new() },1).unwrap();
    client.replace_session(Some(replacement.clone())).unwrap();
    assert!(bound.session().is_none());
    assert!(bound.credential_context().is_none());
    assert_eq!(bound.replace_session(Some(account_a)).unwrap_err().kind,ErrorKind::AccountMismatch);
    assert_eq!(bound.replace_session(None).unwrap_err().kind,ErrorKind::AccountMismatch);
    let errors = vec![catalog.await.unwrap_err(), index.await.unwrap_err(), tokens.await.unwrap_err(),
        cdn.await.unwrap_err(), cover.await.unwrap_err(), bound.list_history(1,20).await.unwrap_err(),
        bound.validate_session().await.unwrap_err(), bound.bound_to_account("bili_43").unwrap_err()];
    for error in errors {
        assert!(matches!(error.kind,ErrorKind::AccountMismatch | ErrorKind::Cancelled));
        assert_eq!(error.transmitted,Some(false));
        assert!(!error.retryable);
    }
    assert_eq!(client.session().unwrap(),replacement);
    assert_eq!(sent.load(Ordering::SeqCst),2);
    assert!(transport.steps.lock().unwrap().is_empty());
    client.replace_session(None).unwrap();
    assert_eq!(bound.list_favorites(1,20).await.unwrap_err().kind,ErrorKind::AccountMismatch);
    assert_eq!(bound.download_cover("https://i0.hdslb.com/fixture.jpg").await.unwrap_err().kind,ErrorKind::AccountMismatch);

    let disabled = BiliClient::offline(Path::new("unused-fixture-data")).unwrap();
    disabled.replace_session(Some(rotated.clone())).unwrap();
    let disabled_bound = disabled.bound_to_account("bili_42").unwrap();
    disabled.replace_session(Some(replacement.clone())).unwrap();
    assert_eq!(disabled_bound.prepare_image_batch(&paths).await.unwrap_err().kind,ErrorKind::AccountMismatch);

    // An A response already in flight cannot be returned for committing after activation of B.
    struct PausedTransport {
        base: Arc<FixtureTransport>, entered: tokio::sync::Notify, release: tokio::sync::Notify,
    }
    impl Transport for PausedTransport {
        fn send(&self, request:HttpRequest)->ResponseFuture<'_> {
            Box::pin(async move {
                self.entered.notify_one();
                self.release.notified().await;
                self.base.send(request).await
            })
        }
    }
    let (_, base) = fixture(vec![step(|request| {
        assert!(header(request,"cookie").unwrap().contains("SESSDATA=rotated-a"));
    },json!({"list":[{"comic_id":12,"title":"Account A Result"}]}))]);
    let paused = Arc::new(PausedTransport { base, entered:tokio::sync::Notify::new(), release:tokio::sync::Notify::new() });
    let root = BiliClient::with_adapters(Some(rotated),Path::new("unused-fixture-data"),paused.clone(),Arc::new(FixtureCrypto));
    let scoped = root.bound_to_account("bili_42").unwrap();
    let acquiring = tokio::spawn(async move { scoped.list_favorites(1,20).await });
    paused.entered.notified().await;
    root.replace_session(Some(replacement)).unwrap();
    paused.release.notify_one();
    let error = acquiring.await.unwrap().unwrap_err();
    assert_eq!(error.kind,ErrorKind::AccountMismatch);
    assert_eq!(error.transmitted,Some(true));
    assert!(!error.definitive);
    assert!(paused.base.steps.lock().unwrap().is_empty());
}
