//! Synthetic production-adapter coverage; no account or network service is used.

use super::*;
use crate::{
    domain::PageDescriptor,
    source::{
        AccountIdentity, AuthSession,
        crypto::{ImageKey, NativeCrypto, ProtocolCrypto, RequestContext, Signature},
        transport::{HttpRequest, HttpResponse, ResponseFuture, Transport},
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, io::Cursor, path::Path, sync::Mutex as StdMutex};
use tokio::sync::Semaphore;

#[derive(Clone, Copy, Debug, Default)]
enum TokenMode {
    #[default]
    Ordered,
    Truncated,
    Reversed,
    InvalidUrl,
    NonObject,
}

#[derive(Clone, Debug)]
struct TokenCall {
    paths: Vec<String>,
    m1: String,
}

#[derive(Clone)]
struct CdnPacket {
    batch: usize,
    page: usize,
    bytes: Vec<u8>,
}

struct RequestGate {
    batch: usize,
    page: Option<usize>,
    entered: Semaphore,
    release: Semaphore,
}

impl RequestGate {
    fn image(batch: usize, page: usize) -> Arc<Self> {
        Self::new(batch, Some(page))
    }

    fn token(batch: usize) -> Arc<Self> { Self::new(batch, None) }

    fn new(batch: usize, page: Option<usize>) -> Arc<Self> {
        Arc::new(Self { batch, page, entered: Semaphore::new(0), release: Semaphore::new(0) })
    }

    async fn wait_until_entered(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.entered.acquire()).await
            .expect("The synthetic request must enter its gate").unwrap().forget();
    }
}

struct FixtureState {
    catalog_episode: Value,
    catalog_calls: usize,
    index_calls: usize,
    token_calls: Vec<TokenCall>,
    downloads: Vec<(usize, usize)>,
    packets: HashMap<String, CdnPacket>,
    token_mode: TokenMode,
    token_headers: Vec<(String, String)>,
    cdn_status: u16,
    cdn_gate: Option<Arc<RequestGate>>,
    token_gate: Option<Arc<RequestGate>>,
}

struct FixtureTransport {
    index: ImageIndex,
    images: Vec<Vec<u8>>,
    state: StdMutex<FixtureState>,
}

impl FixtureTransport {
    fn token_calls(&self) -> Vec<TokenCall> { self.state.lock().unwrap().token_calls.clone() }
    fn downloads(&self) -> Vec<(usize, usize)> { self.state.lock().unwrap().downloads.clone() }
    fn access_requests(&self) -> (usize, usize) {
        let state = self.state.lock().unwrap();
        (state.catalog_calls, state.index_calls)
    }
}

impl Transport for FixtureTransport {
    fn send(&self, request: HttpRequest) -> ResponseFuture<'_> {
        Box::pin(async move {
            let url = url::Url::parse(&request.url).unwrap();
            match (request.method.as_str(), url.host_str().unwrap(), url.path()) {
                ("POST", "manga.bilibili.com", "/twirp/comic.v1.Comic/ComicDetail") => {
                    assert!(header(&request, "cookie").unwrap().contains("SESSDATA=synthetic-session"));
                    let body: Value = serde_json::from_slice(&request.body).unwrap();
                    assert_eq!(body["comic_id"], 710001);
                    assert!(body.get("page_num").is_none());
                    assert!(body.get("page_size").is_none());
                    let episode = {
                        let mut state = self.state.lock().unwrap();
                        state.catalog_calls += 1;
                        state.catalog_episode.clone()
                    };
                    Ok(envelope(json!({"id":710001,"title":"Synthetic access catalog","ep_list":[episode]})))
                }
                ("POST", "manga.bilibili.com", "/twirp/comic.v1.Comic/GetImageIndex") => {
                    let body: Value = serde_json::from_slice(&request.body).unwrap();
                    assert_eq!(body["ep_id"], 720001);
                    self.state.lock().unwrap().index_calls += 1;
                    Ok(envelope(json!({"images":self.index.images.iter().zip(&self.index.paths)
                        .map(|(page, path)| json!({"path":path,"x":page.width,"y":page.height})).collect::<Vec<_>>()})))
                }
                ("POST", "manga.bilibili.com", "/twirp/comic.v1.Comic/ImageToken") => {
                    assert!(header(&request, "cookie").unwrap().contains("SESSDATA=synthetic-session"));
                    let body: Value = serde_json::from_slice(&request.body).unwrap();
                    let paths: Vec<String> = serde_json::from_str(body["urls"].as_str().unwrap()).unwrap();
                    let m1 = body["m1"].as_str().unwrap().to_owned();
                    assert!((1..=IMAGE_BATCH_SIZE).contains(&paths.len()));
                    assert_eq!(m1.len(), 88);
                    let (response, gate) = {
                        let mut state = self.state.lock().unwrap();
                        let batch = state.token_calls.len();
                        let mut tokens = Vec::with_capacity(paths.len());
                        for (token_index, path) in paths.iter().enumerate() {
                            let page = self.index.paths.iter().position(|candidate| candidate == path)
                                .expect("Only selected synthetic paths may be tokenized");
                            let resource = format!("/synthetic-page-batch/{batch}/{token_index}");
                            // The synthetic cipher binds image bytes to the exact
                            // public key requested by this token batch.
                            let mut bytes = m1.as_bytes().to_vec();
                            bytes.extend_from_slice(&self.images[page]);
                            state.packets.insert(resource.clone(), CdnPacket { batch, page, bytes });
                            tokens.push(json!({"complete_url":format!("https://i0.hdslb.com{resource}?token=synthetic"),
                                "hit_encrpyt":true}));
                        }
                        state.token_calls.push(TokenCall { paths, m1 });
                        match state.token_mode {
                            TokenMode::Ordered => {},
                            TokenMode::Truncated => { tokens.pop(); },
                            TokenMode::Reversed => tokens.reverse(),
                            TokenMode::InvalidUrl => tokens[0]["complete_url"] = json!("https://example.invalid/synthetic"),
                            TokenMode::NonObject => tokens[0] = Value::Null,
                        }
                        let mut response = envelope(json!(tokens));
                        response.headers.extend(state.token_headers.clone());
                        let gate = state.token_gate.as_ref().filter(|gate| gate.batch == batch).cloned();
                        (response, gate)
                    };
                    if let Some(gate) = gate {
                        gate.entered.add_permits(1);
                        gate.release.acquire().await.unwrap().forget();
                    }
                    Ok(response)
                }
                ("GET", "i0.hdslb.com", path) => {
                    assert!(header(&request, "cookie").is_none());
                    assert!(header(&request, "x-xsrf-token").is_none());
                    assert_eq!(request.max_bytes, 16 * 1024 * 1024);
                    let (packet, status, gate) = {
                        let mut state = self.state.lock().unwrap();
                        let packet = state.packets.get(path).expect("Only issued synthetic resources may be read").clone();
                        state.downloads.push((packet.batch, packet.page));
                        let gate = state.cdn_gate.as_ref().filter(|gate| gate.batch == packet.batch && gate.page == Some(packet.page)).cloned();
                        (packet, state.cdn_status, gate)
                    };
                    if let Some(gate) = gate {
                        gate.entered.add_permits(1);
                        gate.release.acquire().await.unwrap().forget();
                    }
                    Ok(HttpResponse { status, headers: vec![("content-type".into(), "application/octet-stream".into())], body: packet.bytes })
                }
                _ => Err(SourceError::protocol("unexpected_synthetic_page_batch_endpoint")),
            }
        })
    }
}

struct FixtureCrypto;

impl ProtocolCrypto for FixtureCrypto {
    fn sign(&self, context: &RequestContext) -> crate::source::Result<Signature> {
        assert_eq!(context.sign_query, "device=pc&platform=web&nov=27&eot=812");
        assert!(serde_json::from_str::<Value>(&context.body).unwrap().is_object());
        Ok(Signature { ultra_sign: "synthetic-page-batch-sign".into(), data_sn: "synthetic-page-batch-sn".into() })
    }

    fn decode_response(&self, _: &RequestContext, _: Value) -> crate::source::Result<Value> {
        Err(SourceError::protocol("unexpected_synthetic_page_batch_response_cipher"))
    }

    fn new_image_key(&self) -> crate::source::Result<ImageKey> {
        NativeCrypto::new(Path::new("unused-page-batch-assets")).new_image_key()
    }

    fn decrypt_image(&self, key: &ImageKey, _: &str, bytes: &[u8]) -> crate::source::Result<Vec<u8>> {
        let public = key.m1();
        let image = bytes.strip_prefix(public.as_bytes())
            .ok_or_else(|| SourceError::protocol("synthetic_image_key_mismatch"))?;
        Ok(image.to_vec())
    }
}

struct Fixture {
    client: BiliClient,
    bound: BiliClient,
    index: ImageIndex,
    transport: Arc<FixtureTransport>,
}

fn session(account: &str) -> AuthSession {
    AuthSession::from_cookie_text(&format!(
        "SESSDATA=synthetic-session; bili_jct=synthetic-csrf; buvid3=synthetic-device; DedeUserID={account}"))
        .unwrap().with_identity(AccountIdentity {
            id: account.into(), name: "Synthetic batch account".into(), avatar_url: String::new(),
        }, 1).unwrap()
}

fn fixture(count: usize) -> Fixture {
    let index = ImageIndex {
        episode_id: "720001".into(), revision: "synthetic-page-batch-revision".into(),
        images: (0..count).map(|page| PageDescriptor {
            id: format!("synthetic-page-{page}"), index: page, width: 7 + page as u32, height: 11 + page as u32,
        }).collect(),
        paths: (0..count).map(|page| format!("/synthetic-index/page-{page}")).collect(),
        ..Default::default()
    };
    let images = index.images.iter().map(|page| {
        let pixels = image::GrayImage::from_pixel(page.width, page.height, image::Luma([(page.index * 29 + 17) as u8]));
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(pixels).write_to(&mut output, image::ImageFormat::Png).unwrap();
        output.into_inner()
    }).collect();
    let transport = Arc::new(FixtureTransport {
        index: index.clone(), images,
        state: StdMutex::new(FixtureState {
            catalog_episode: json!({"id":720001,"comic_id":710001,"ord":1,"pay_mode":0}),
            catalog_calls: 0, index_calls: 0,
            token_calls: Vec::new(), downloads: Vec::new(), packets: HashMap::new(),
            token_mode: TokenMode::Ordered, token_headers: Vec::new(), cdn_status: 200,
            cdn_gate: None, token_gate: None,
        }),
    });
    let client = BiliClient::with_adapters(Some(session("42")), Path::new("unused-page-batch-data"),
        transport.clone(), Arc::new(FixtureCrypto));
    let bound = client.bound_to_account("bili_42").unwrap();
    Fixture { client, bound, index, transport }
}

fn header<'a>(request: &'a HttpRequest, name: &str) -> Option<&'a str> {
    request.headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
}

fn envelope(data: Value) -> HttpResponse {
    HttpResponse { status: 200, headers: Vec::new(), body: serde_json::to_vec(&json!({"code":0,"data":data})).unwrap() }
}

fn assert_page(fixture: &Fixture, original: usize, acquired: AcquiredPage) {
    let page = &fixture.index.images[original];
    assert_eq!((acquired.width, acquired.height), (page.width, page.height));
    assert_eq!(acquired.bytes, fixture.transport.images[original]);
    assert_eq!(acquired.sha256.as_deref(), Some(format!("{:x}", Sha256::digest(&acquired.bytes)).as_str()));
}

async fn age(batch: &BiliPageBatch) {
    batch.state.lock().await.created_at = Instant::now()
        .checked_sub(IMAGE_BATCH_FRESHNESS_BUDGET + Duration::from_secs(1)).unwrap();
}

#[tokio::test]
async fn one_token_request_preserves_an_arbitrary_missing_page_subset_and_original_mapping() {
    let fixture = fixture(6);
    assert_eq!(AcquisitionSource::page_batch_size(&fixture.bound), IMAGE_BATCH_SIZE);
    let batch = AcquisitionSource::prepare_page_batch(&fixture.bound, &fixture.index, &[4, 1, 3]).await.unwrap();
    let calls = fixture.transport.token_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].paths, [4, 1, 3].map(|page| fixture.index.paths[page].clone()));
    let (one, four, three) = tokio::join!(batch.download_page(1), batch.download_page(4), batch.download_page(3));
    assert_page(&fixture, 1, one.unwrap());
    assert_page(&fixture, 4, four.unwrap());
    assert_page(&fixture, 3, three.unwrap());
    assert_eq!(fixture.transport.token_calls().len(), 1);
    let mut downloaded = fixture.transport.downloads().iter().map(|(_, page)| *page).collect::<Vec<_>>();
    downloaded.sort_unstable();
    assert_eq!(downloaded, [1, 3, 4]);
    assert_eq!(batch.download_page(0).await.err().unwrap().code, "page_index_invalid");
    assert_eq!(fixture.transport.downloads().len(), 3);
}

#[tokio::test]
async fn invalid_batch_positions_and_missing_paths_fail_before_token_or_cdn_traffic() {
    let fixture = fixture(4);
    for (positions, expected) in [
        (Vec::new(), "invalid_image_path_batch"),
        ((0..21).collect(), "invalid_image_path_batch"),
        (vec![1, 1], "duplicate_page_index"),
        (vec![4], "page_index_invalid"),
    ] {
        let error = AcquisitionSource::prepare_page_batch(&fixture.bound, &fixture.index, &positions)
            .await.err().expect("An invalid page selection must fail");
        assert_eq!(error.code, expected);
        assert!(!error.retryable);
    }
    let mut index = fixture.index.clone();
    index.paths.truncate(1);
    let error = AcquisitionSource::prepare_page_batch(&fixture.bound, &index, &[2]).await.err().unwrap();
    assert_eq!(error.code, "fresh_image_index_required");
    index = fixture.index.clone();
    index.paths[0] = "\n".into();
    let error = AcquisitionSource::prepare_page_batch(&fixture.bound, &index, &[0]).await.err().unwrap();
    assert_eq!(error.code, "invalid_image_path_batch");
    assert!(fixture.transport.token_calls().is_empty());
    assert!(fixture.transport.downloads().is_empty());
}

#[tokio::test]
async fn overlapping_batches_keep_their_own_p256_keys_when_image_requests_interleave() {
    let fixture = fixture(4);
    let first = AcquisitionSource::prepare_page_batch(&fixture.bound, &fixture.index, &[3, 1]).await.unwrap();
    let second = AcquisitionSource::prepare_page_batch(&fixture.bound, &fixture.index, &[1, 2]).await.unwrap();
    let calls = fixture.transport.token_calls();
    assert_eq!(calls.len(), 2);
    assert_ne!(calls[0].m1, calls[1].m1);
    assert_page(&fixture, 2, second.download_page(2).await.unwrap());
    assert_page(&fixture, 3, first.download_page(3).await.unwrap());
    assert_page(&fixture, 1, second.download_page(1).await.unwrap());
    assert_page(&fixture, 1, first.download_page(1).await.unwrap());
    assert_eq!(fixture.transport.downloads(), [(1, 2), (0, 3), (1, 1), (0, 1)]);
}

#[tokio::test]
async fn stale_logout_and_account_switch_reject_old_grants_before_cdn_traffic() {
    for replacement in [Some(session("42")), None, Some(session("43"))] {
        for aged in [false, true] {
            let fixture = fixture(2);
            let batch = BiliPageBatch::prepare(&fixture.bound, &fixture.index, &[0, 1]).await.unwrap();
            fixture.client.replace_session(replacement.clone()).unwrap();
            if aged { age(&batch).await; }
            let error = batch.download_page(0).await.err().unwrap();
            assert_eq!(error.code, "account_changed");
            assert!(!error.retryable);
            assert!(fixture.transport.downloads().is_empty());
            if fixture.client.session().as_ref().and_then(AuthSession::account_key) != Some("bili_42") {
                let error = AcquisitionSource::prepare_page_batch(&fixture.bound, &fixture.index, &[1]).await.err().unwrap();
                assert_eq!(error.code, "bound_account_mismatch");
            }
            assert_eq!(fixture.transport.token_calls().len(), 1);
        }
    }
}

#[tokio::test]
async fn legitimate_cookie_publication_does_not_invalidate_an_account_grant() {
    let fixture = fixture(1);
    fixture.transport.state.lock().unwrap().token_headers.push(("set-cookie".into(),
        "XSRF-TOKEN=synthetic%2Brenewal; Domain=.bilibili.com; Path=/; Secure".into()));
    let previous_session_epoch = fixture.client.credential_epoch();
    let batch = AcquisitionSource::prepare_page_batch(&fixture.bound, &fixture.index, &[0]).await.unwrap();
    assert!(fixture.client.credential_epoch() > previous_session_epoch);
    assert_page(&fixture, 0, batch.download_page(0).await.unwrap());
}

#[tokio::test]
async fn malformed_token_responses_never_start_image_downloads() {
    for (mode, expected) in [
        (TokenMode::Truncated, "image_token_count_mismatch"),
        (TokenMode::InvalidUrl, "image_resource_invalid"),
        (TokenMode::NonObject, "image_token_invalid"),
    ] {
        let fixture = fixture(2);
        fixture.transport.state.lock().unwrap().token_mode = mode;
        let error = AcquisitionSource::prepare_page_batch(&fixture.bound, &fixture.index, &[1, 0]).await.err().unwrap();
        assert_eq!(error.code, expected, "Token mode {mode:?}");
        assert_eq!(fixture.transport.token_calls().len(), 1);
        assert!(fixture.transport.downloads().is_empty());
    }
}

#[tokio::test]
async fn reordered_tokens_with_different_page_geometry_are_rejected() {
    let fixture = fixture(3);
    fixture.transport.state.lock().unwrap().token_mode = TokenMode::Reversed;
    let batch = AcquisitionSource::prepare_page_batch(&fixture.bound, &fixture.index, &[0, 2]).await.unwrap();
    let error = batch.download_page(0).await.err().unwrap();
    assert_eq!(error.code, "image_geometry_changed");
    assert!(!error.retryable);
    assert_eq!(fixture.transport.downloads(), [(0, 2)]);
    assert_eq!(fixture.transport.token_calls().len(), 1);
}

#[tokio::test]
async fn image_http_403_neither_refreshes_tokens_nor_retries_the_image() {
    let fixture = fixture(2);
    fixture.transport.state.lock().unwrap().cdn_status = 403;
    let batch = AcquisitionSource::prepare_page_batch(&fixture.bound, &fixture.index, &[0, 1]).await.unwrap();
    let error = batch.download_page(0).await.err().unwrap();
    assert_eq!(error.code, "image_http_rejected");
    assert!(!error.retryable);
    assert_eq!(fixture.transport.token_calls().len(), 1);
    assert_eq!(fixture.transport.downloads(), [(0, 0)]);
}

#[tokio::test]
async fn queued_age_refresh_is_coalesced_and_excludes_successfully_returned_pages() {
    let fixture = fixture(5);
    let batch = BiliPageBatch::prepare(&fixture.bound, &fixture.index, &[4, 1, 3]).await.unwrap();
    assert_page(&fixture, 1, batch.download_page(1).await.unwrap());
    age(&batch).await;
    let (three, four) = tokio::join!(batch.download_page(3), batch.download_page(4));
    assert_page(&fixture, 3, three.unwrap());
    assert_page(&fixture, 4, four.unwrap());
    let calls = fixture.transport.token_calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].paths, [4, 3].map(|page| fixture.index.paths[page].clone()));
    assert_ne!(calls[0].m1, calls[1].m1);
    assert_eq!(batch.download_page(1).await.err().unwrap().code, "page_batch_page_completed");
    assert_eq!(fixture.transport.downloads().len(), 3);
}

#[tokio::test]
async fn progressing_slow_downloads_can_refresh_each_remaining_page_with_a_finite_bound() {
    let fixture = fixture(4);
    let batch = BiliPageBatch::prepare(&fixture.bound, &fixture.index, &[0, 1, 2, 3]).await.unwrap();
    for page in 0..4 {
        age(&batch).await;
        assert_page(&fixture, page, batch.download_page(page).await.unwrap());
    }
    let calls = fixture.transport.token_calls();
    assert_eq!(calls.len(), 5);
    for (refresh, call) in calls.iter().skip(1).enumerate() {
        assert_eq!(call.paths, fixture.index.paths[refresh..].to_vec());
    }
    assert_eq!(batch.state.lock().await.refreshes, 4);
    assert_eq!(fixture.transport.downloads().len(), 4);
}

#[tokio::test]
async fn repeated_age_without_progress_is_blocked_until_an_in_flight_page_completes() {
    let fixture = fixture(2);
    let batch = Arc::new(BiliPageBatch::prepare(&fixture.bound, &fixture.index, &[0, 1]).await.unwrap());
    let gate = RequestGate::image(1, 0);
    fixture.transport.state.lock().unwrap().cdn_gate = Some(gate.clone());
    age(&batch).await;
    let pending = batch.clone();
    let first = tokio::spawn(async move { pending.download_page(0).await });
    gate.wait_until_entered().await;
    age(&batch).await;
    let error = batch.download_page(1).await.err().unwrap();
    assert_eq!(error.code, "image_batch_expired");
    assert!(error.retryable);
    assert_eq!(fixture.transport.token_calls().len(), 2);
    gate.release.add_permits(1);
    assert_page(&fixture, 0, first.await.unwrap().unwrap());
    assert_page(&fixture, 1, batch.download_page(1).await.unwrap());
    let calls = fixture.transport.token_calls();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[2].paths, [fixture.index.paths[1].clone()]);
}

#[tokio::test]
async fn refreshing_a_grant_preserves_an_older_in_flight_image_key() {
    let fixture = fixture(2);
    let batch = Arc::new(BiliPageBatch::prepare(&fixture.bound, &fixture.index, &[0, 1]).await.unwrap());
    let gate = RequestGate::image(0, 0);
    fixture.transport.state.lock().unwrap().cdn_gate = Some(gate.clone());
    let pending = batch.clone();
    let first = tokio::spawn(async move { pending.download_page(0).await });
    gate.wait_until_entered().await;
    age(&batch).await;
    let second = tokio::time::timeout(Duration::from_secs(5), batch.download_page(1)).await
        .expect("Token refresh must not retain a lock held by an in-flight image").unwrap();
    assert_page(&fixture, 1, second);
    gate.release.add_permits(1);
    assert_page(&fixture, 0, first.await.unwrap().unwrap());
    let calls = fixture.transport.token_calls();
    assert_eq!(calls.len(), 2);
    assert_ne!(calls[0].m1, calls[1].m1);
    assert_eq!(fixture.transport.downloads(), [(0, 0), (1, 1)]);
}

#[tokio::test]
async fn failed_age_refresh_is_shared_by_concurrent_callers_without_reissuing_tokens() {
    let fixture = fixture(2);
    let batch = BiliPageBatch::prepare(&fixture.bound, &fixture.index, &[0, 1]).await.unwrap();
    fixture.transport.state.lock().unwrap().token_mode = TokenMode::Truncated;
    age(&batch).await;
    let (first, second) = tokio::join!(batch.download_page(0), batch.download_page(1));
    assert_eq!(first.err().unwrap().code, "image_token_count_mismatch");
    assert_eq!(second.err().unwrap().code, "image_token_count_mismatch");
    assert_eq!(fixture.transport.token_calls().len(), 2);
    assert!(fixture.transport.downloads().is_empty());
}

#[tokio::test]
async fn cancelling_token_refresh_does_not_allow_another_refresh_without_progress() {
    let fixture = fixture(2);
    let batch = Arc::new(BiliPageBatch::prepare(&fixture.bound, &fixture.index, &[0, 1]).await.unwrap());
    let gate = RequestGate::token(1);
    fixture.transport.state.lock().unwrap().token_gate = Some(gate.clone());
    age(&batch).await;
    let pending = batch.clone();
    let image = tokio::spawn(async move { pending.download_page(0).await });
    gate.wait_until_entered().await;
    image.abort();
    assert!(image.await.err().unwrap().is_cancelled());
    let error = batch.download_page(1).await.err().unwrap();
    assert_eq!(error.code, "image_batch_expired");
    assert!(error.retryable);
    assert_eq!(fixture.transport.token_calls().len(), 2);
    assert!(fixture.transport.downloads().is_empty());
}

#[tokio::test]
async fn account_replacement_during_token_refresh_cannot_publish_a_new_grant() {
    for replacement in [Some(session("42")), None, Some(session("43"))] {
        let fixture = fixture(2);
        let batch = Arc::new(BiliPageBatch::prepare(&fixture.bound, &fixture.index, &[0, 1]).await.unwrap());
        let gate = RequestGate::token(1);
        fixture.transport.state.lock().unwrap().token_gate = Some(gate.clone());
        age(&batch).await;
        let pending = batch.clone();
        let image = tokio::spawn(async move { pending.download_page(0).await });
        gate.wait_until_entered().await;
        fixture.client.replace_session(replacement).unwrap();
        gate.release.add_permits(1);
        let error = image.await.unwrap().err().unwrap();
        assert!(matches!(error.code.as_str(), "account_changed" | "bound_account_mismatch"));
        assert!(!error.retryable);
        assert_eq!(batch.download_page(1).await.err().unwrap().code, "account_changed");
        assert_eq!(fixture.transport.token_calls().len(), 2);
        assert!(fixture.transport.downloads().is_empty());
        assert!(batch.state.lock().await.completed.iter().all(|completed| !*completed));
    }
}

#[tokio::test]
async fn logout_during_an_image_request_discards_the_in_flight_page() {
    let fixture = fixture(1);
    let batch = Arc::new(BiliPageBatch::prepare(&fixture.bound, &fixture.index, &[0]).await.unwrap());
    let gate = RequestGate::image(0, 0);
    fixture.transport.state.lock().unwrap().cdn_gate = Some(gate.clone());
    let pending = batch.clone();
    let image = tokio::spawn(async move { pending.download_page(0).await });
    gate.wait_until_entered().await;
    fixture.client.replace_session(None).unwrap();
    gate.release.add_permits(1);
    let error = image.await.unwrap().err().unwrap();
    assert!(matches!(error.code.as_str(), "account_changed" | "bound_account_mismatch"));
    assert!(!error.retryable);
    assert!(!batch.state.lock().await.completed[0]);
}

#[tokio::test]
async fn image_index_and_single_page_compatibility_use_the_existing_source_contract() {
    let fixture = fixture(3);
    let index = AcquisitionSource::image_index(&fixture.bound, "720001").await.unwrap();
    assert_eq!(index.images.len(), 3);
    assert_eq!(index.paths, fixture.index.paths);
    let page = AcquisitionSource::download_page(&fixture.bound, &index, 2).await.unwrap();
    assert_page(&fixture, 2, page);
    assert_eq!(fixture.transport.token_calls()[0].paths, [fixture.index.paths[2].clone()]);
    assert_eq!(fixture.transport.downloads(), [(0, 2)]);
}

#[tokio::test]
async fn fresh_access_rejects_revocation_expiry_and_unknown_grants_before_a_successful_short_index() {
    for (name, episode, code) in [
        ("revoked ownership", json!({"id":720001,"pay_mode":0,"unlock_type":1,"is_locked":true}), "episode_locked"),
        ("expired wait-free", json!({"id":720001,"unlock_type":2,"unlock_expire_at":1}), "episode_locked"),
        ("expired limited-free", json!({"id":720001,"unlock_type":3,"unlock_expire_at":1}), "episode_locked"),
        ("unknown access", json!({"id":720001,"pay_mode":1,"is_locked":false}), "episode_access_unknown"),
        ("unparsed temporary expiry", json!({"id":720001,"pay_mode":1,"is_in_free":true,
            "unlock_expire_at":"unknown"}), "episode_access_unknown"),
        ("withdrawn chapter", json!({"id":720001,"unavailable":true}), "episode_unavailable"),
        ("missing chapter", json!({"id":720002,"pay_mode":0}), "episode_access_missing"),
    ] {
        // The two-image index endpoint would return code=0. A previously
        // readable catalog must not allow it to bypass the fresh access check.
        let fixture = fixture(2);
        let previous = fixture.bound.comic_detail("710001").await.unwrap();
        assert!(previous.episodes[0].readable_at(chrono::Utc::now().timestamp()));
        fixture.transport.state.lock().unwrap().catalog_episode = episode;
        let error = AcquisitionSource::image_index_for(&fixture.bound, "710001", "720001")
            .await.err().expect("Current unreadable access must reject acquisition");
        assert_eq!(error.code, code, "{name}");
        assert!(!error.retryable, "{name}");
        assert_eq!(fixture.transport.access_requests(), (2, 0), "{name}");
        assert!(fixture.transport.token_calls().is_empty(), "{name}");
        assert!(fixture.transport.downloads().is_empty(), "{name}");
    }
}

#[tokio::test]
async fn fresh_readable_grants_allow_the_entire_returned_index() {
    let future = chrono::Utc::now().timestamp() + 3600;
    for episode in [
        json!({"id":720001,"pay_mode":0}),
        json!({"id":720001,"pay_mode":1,"unlock_type":1,"unlock_expire_at":0}),
        json!({"id":720001,"unlock_type":2,"unlock_expire_at":future}),
        json!({"id":720001,"unlock_type":3,"unlock_expire_at":future}),
    ] {
        let fixture = fixture(45);
        fixture.transport.state.lock().unwrap().catalog_episode = episode;
        let index = AcquisitionSource::image_index_for(&fixture.bound, "710001", "720001").await.unwrap();
        assert_eq!(index.images.len(), 45);
        assert_eq!(index.paths, fixture.index.paths);
        assert_eq!((index.images[44].width, index.images[44].height), (51, 55));
        assert_eq!(fixture.transport.access_requests(), (1, 1));
        assert!(fixture.transport.token_calls().is_empty());
        assert!(fixture.transport.downloads().is_empty());
    }
}

#[tokio::test]
async fn fresh_access_requires_the_confirmed_current_account_before_any_catalog_request() {
    for replacement in [None, Some(session("43"))] {
        let fixture = fixture(2);
        fixture.client.replace_session(replacement).unwrap();
        let error = AcquisitionSource::image_index_for(&fixture.bound, "710001", "720001")
            .await.err().expect("A stale bound account must not fetch permissions or images");
        assert_eq!(error.code, "account_identity_unconfirmed");
        assert!(!error.retryable);
        assert_eq!(fixture.transport.access_requests(), (0, 0));
        assert!(fixture.transport.token_calls().is_empty());
        assert!(fixture.transport.downloads().is_empty());
    }
}
