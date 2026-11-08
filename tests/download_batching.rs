//! Controlled measurements of production acquisition batching and durable cache reuse.
//!
//! The transport cannot contact a service. Every page and account is synthetic.
//! Native mode executes the reviewed public signing WASM, without live requests.
//! Use scripts/verification/download_batching.py on the designated remote host.

#![cfg(target_os = "linux")]

use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::Cursor,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use bilicomics::{
    domain::ImageIndex,
    downloads::{
        AcquiredPage, AcquisitionFuture, AcquisitionSource, DownloadEventKind, DownloadManager,
        DownloadRequest,
    },
    source::{
        AccountIdentity, AuthSession, BiliClient, SourceError,
        crypto::{ImageKey, NativeCrypto, ProtocolCrypto, RequestContext, Signature},
        transport::{HttpRequest, HttpResponse, ResponseFuture, Transport},
    },
    store::{DownloadStatus, Store},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const ACCOUNT: &str = "bili_424244";
const COMIC: &str = "910001";
const EPISODE: &str = "910002";
const PAGE_COUNT: usize = 45;
const IMAGE_WIDTH: u32 = 96;
const IMAGE_HEIGHT: u32 = 144;
const CONCURRENCY: usize = 2;
const CDN_LATENCY_MS: u64 = 5;
const ASSET_FILE: &str = "efae82c96a7eef44bee5.wasm";
const ASSET_SHA256: &str = "39bc0676953752c461197df592e1f5894f1a7492a29400c946e560fc109a8e2e";

#[derive(Clone, Copy)]
enum Mode { Legacy, Batched }

impl Mode {
    fn label(self) -> &'static str {
        match self { Self::Legacy => "legacy_single_page", Self::Batched => "batched_twenty_page" }
    }
}

struct Configuration {
    signer: String,
    native_assets: Option<PathBuf>,
    token_latency: Duration,
    repetitions: usize,
}

impl Configuration {
    fn read() -> Self {
        let signer = std::env::var("BILICOMICS_BATCH_SIGNER").unwrap_or_else(|_| "fixture".into());
        assert!(matches!(signer.as_str(), "fixture" | "native"));
        let native_assets = std::env::var_os("BILICOMICS_BATCH_NATIVE_ASSETS").map(PathBuf::from);
        if signer == "native" {
            let root = native_assets.as_ref().expect("Native mode requires a preprovisioned public signing asset");
            assert_eq!(digest(&fs::read(root.join(ASSET_FILE)).unwrap()), ASSET_SHA256,
                "The public signing module must match the reviewed pin");
        } else {
            assert!(native_assets.is_none(), "Fixture mode must not consume signing assets");
        }
        let token_latency_ms = std::env::var("BILICOMICS_BATCH_TOKEN_LATENCY_MS")
            .unwrap_or_else(|_| "20".into()).parse::<u64>().unwrap();
        assert!(token_latency_ms <= 10_000);
        let repetitions = std::env::var("BILICOMICS_BATCH_REPETITIONS")
            .unwrap_or_else(|_| "4".into()).parse::<usize>().unwrap();
        assert!((2..=20).contains(&repetitions) && repetitions % 2 == 0,
            "Use an even repetition count so the execution order is balanced");
        Self { signer, native_assets, token_latency: Duration::from_millis(token_latency_ms), repetitions }
    }
}

struct FixturePage {
    bytes: Vec<u8>,
    sha256: String,
    pixels_sha256: String,
}

fn digest(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }

fn fixture_pages() -> Arc<Vec<FixturePage>> {
    let pages = (0..PAGE_COUNT).map(|page| {
        let image = image::GrayImage::from_fn(IMAGE_WIDTH, IMAGE_HEIGHT, |x, y| {
            image::Luma([((x / 6 * 13 + y / 9 * 3 + page as u32 * 7) % 256) as u8])
        });
        let pixels_sha256 = digest(image.as_raw());
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(image).write_to(&mut output, image::ImageFormat::Png).unwrap();
        let bytes = output.into_inner();
        FixturePage { sha256: digest(&bytes), bytes, pixels_sha256 }
    }).collect::<Vec<_>>();
    assert_eq!(pages.iter().map(|page| page.sha256.as_str()).collect::<HashSet<_>>().len(), PAGE_COUNT);
    Arc::new(pages)
}

#[derive(Default)]
struct Audit {
    requests: Vec<Value>,
    signatures: Vec<Value>,
    token_batches: Vec<Vec<usize>>,
    cdn_pages: Vec<usize>,
    image_keys: usize,
    cdn_in_flight: usize,
    maximum_cdn_in_flight: usize,
}

struct AuditedCrypto {
    native: NativeCrypto,
    native_signing: bool,
    audit: Arc<Mutex<Audit>>,
}

impl ProtocolCrypto for AuditedCrypto {
    fn sign(&self, context: &RequestContext) -> bilicomics::source::Result<Signature> {
        assert_eq!(context.sign_query, "device=pc&platform=web&nov=27&eot=812");
        assert!(matches!(context.endpoint.as_str(), "ComicDetail" | "GetImageIndex" | "ImageToken"));
        assert!(serde_json::from_str::<Value>(&context.body).unwrap().is_object());
        let started = Instant::now();
        let signature = if self.native_signing {
            self.native.sign(context)?
        } else {
            Signature { ultra_sign: format!("fixture-{}", digest(context.body.as_bytes())),
                data_sn: "synthetic-download-batching".into() }
        };
        self.audit.lock().unwrap().signatures.push(json!({"endpoint":context.endpoint,
            "body_sha256":digest(context.body.as_bytes()),
            "signature_sha256":digest(signature.ultra_sign.as_bytes()),
            "data_sn_sha256":digest(signature.data_sn.as_bytes()),
            "elapsed_ms":started.elapsed().as_secs_f64()*1000.0,
            "signer":if self.native_signing { "native_crypto_pinned_wasm" } else { "injected_fixture" }}));
        Ok(signature)
    }

    fn decode_response(&self, _: &RequestContext, _: Value) -> bilicomics::source::Result<Value> {
        Err(SourceError::protocol("unexpected_batch_fixture_response_cipher"))
    }

    fn new_image_key(&self) -> bilicomics::source::Result<ImageKey> {
        let key = self.native.new_image_key()?;
        self.audit.lock().unwrap().image_keys += 1;
        Ok(key)
    }

    fn decrypt_image(&self, _: &ImageKey, _: &str, _: &[u8]) -> bilicomics::source::Result<Vec<u8>> {
        Err(SourceError::protocol("unexpected_batch_fixture_image_cipher"))
    }
}

struct FixtureTransport {
    pages: Arc<Vec<FixturePage>>,
    allowed_pages: Vec<usize>,
    token_latency: Duration,
    started: Arc<OnceLock<Instant>>,
    audit: Arc<Mutex<Audit>>,
}

struct CdnFlight<'a>(&'a Mutex<Audit>);

impl<'a> CdnFlight<'a> {
    fn begin(audit: &'a Mutex<Audit>) -> Self {
        let mut state = audit.lock().unwrap();
        state.cdn_in_flight += 1;
        state.maximum_cdn_in_flight = state.maximum_cdn_in_flight.max(state.cdn_in_flight);
        assert!(state.cdn_in_flight <= CONCURRENCY, "CDN acquisition exceeded the shared page limit");
        Self(audit)
    }
}

impl Drop for CdnFlight<'_> {
    fn drop(&mut self) { self.0.lock().unwrap().cdn_in_flight -= 1; }
}

fn header<'a>(request: &'a HttpRequest, name: &str) -> Option<&'a str> {
    request.headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
}

fn page_from_path(path: &str, prefix: &str, suffix: &str) -> usize {
    let page = path.strip_prefix(prefix).unwrap().strip_suffix(suffix).unwrap().parse::<usize>().unwrap();
    assert!(page < PAGE_COUNT);
    page
}

fn envelope(data: Value) -> HttpResponse {
    HttpResponse { status: 200, headers: Vec::new(),
        body: serde_json::to_vec(&json!({"code":0,"data":data})).unwrap() }
}

impl FixtureTransport {
    fn record(&self, request: &HttpRequest, endpoint: &str, pages: &[usize]) {
        let signature = url::Url::parse(&request.url).unwrap().query_pairs()
            .find(|(key, _)| key == "ultra_sign").map(|(_, value)| value.into_owned());
        if endpoint != "CDN" { assert!(signature.as_ref().is_some_and(|value| !value.is_empty())); }
        let mut audit = self.audit.lock().unwrap();
        let request_id = audit.requests.len();
        audit.requests.push(json!({"request_id":request_id,"endpoint":endpoint,
            "method":request.method,"page_indices":pages,"body_sha256":digest(&request.body),
            "signature_sha256":signature.map(|value| digest(value.as_bytes())),
            "elapsed_to_request_ms":self.started.get().unwrap().elapsed().as_secs_f64()*1000.0}));
        if endpoint == "ImageToken" { audit.token_batches.push(pages.to_vec()); }
        if endpoint == "CDN" { audit.cdn_pages.extend_from_slice(pages); }
    }
}

impl Transport for FixtureTransport {
    fn send(&self, request: HttpRequest) -> ResponseFuture<'_> {
        Box::pin(async move {
            let url = url::Url::parse(&request.url).unwrap();
            let path = url.path();
            match (url.host_str().unwrap(), path) {
                ("manga.bilibili.com", "/twirp/comic.v1.Comic/ComicDetail") => {
                    assert_eq!(request.method, "POST");
                    assert!(header(&request, "cookie").is_some());
                    let body: Value = serde_json::from_slice(&request.body).unwrap();
                    assert_eq!(body["comic_id"], 910001);
                    self.record(&request, "ComicDetail", &[]);
                    Ok(envelope(json!({"id":910001,"title":"Synthetic full-access comic",
                        "ep_list":[{"id":910002,"title":"Synthetic full-access episode",
                            "ord":1,"is_locked":false,"pay_mode":0}]})))
                }
                ("manga.bilibili.com", "/twirp/comic.v1.Comic/GetImageIndex") => {
                    assert_eq!(request.method, "POST");
                    assert!(header(&request, "cookie").is_some());
                    let body: Value = serde_json::from_slice(&request.body).unwrap();
                    assert_eq!(body["ep_id"], 910002);
                    self.record(&request, "GetImageIndex", &[]);
                    Ok(envelope(json!({"images":(0..PAGE_COUNT).map(|page| json!({
                        "path":format!("/synthetic-batching/page-{page}"),"x":IMAGE_WIDTH,"y":IMAGE_HEIGHT
                    })).collect::<Vec<_>>()})))
                }
                ("manga.bilibili.com", "/twirp/comic.v1.Comic/ImageToken") => {
                    assert_eq!(request.method, "POST");
                    assert!(header(&request, "cookie").is_some());
                    let body: Value = serde_json::from_slice(&request.body).unwrap();
                    let paths: Vec<String> = serde_json::from_str(body["urls"].as_str().unwrap()).unwrap();
                    assert!((1..=20).contains(&paths.len()));
                    let public = STANDARD.decode(body["m1"].as_str().unwrap()).unwrap();
                    assert_eq!(public.len(), 65);
                    assert_eq!(public[0], 4);
                    let pages = paths.iter().map(|path| page_from_path(path, "/synthetic-batching/page-", "")).collect::<Vec<_>>();
                    assert!(pages.windows(2).all(|pair| pair[0] < pair[1]));
                    assert!(pages.iter().all(|page| self.allowed_pages.contains(page)),
                        "Only missing pages may be tokenized");
                    self.record(&request, "ImageToken", &pages);
                    tokio::time::sleep(self.token_latency).await;
                    Ok(envelope(json!(pages.iter().map(|page| json!({
                        "complete_url":format!("https://i0.hdslb.com/synthetic-batching-page-{page}.png?token=synthetic"),
                        "hit_encrpyt":false})).collect::<Vec<_>>())))
                }
                ("i0.hdslb.com", _) if path.starts_with("/synthetic-batching-page-") => {
                    assert_eq!(request.method, "GET");
                    assert!(request.body.is_empty());
                    assert!(header(&request, "cookie").is_none());
                    assert!(header(&request, "x-xsrf-token").is_none());
                    let page = page_from_path(path, "/synthetic-batching-page-", ".png");
                    assert!(self.allowed_pages.contains(&page), "A committed page must be reused");
                    self.record(&request, "CDN", &[page]);
                    let _flight = CdnFlight::begin(&self.audit);
                    tokio::time::sleep(Duration::from_millis(CDN_LATENCY_MS)).await;
                    Ok(HttpResponse { status:200, headers:vec![("content-type".into(), "image/png".into())],
                        body:self.pages[page].bytes.clone() })
                }
                _ => Err(SourceError::protocol("batch_fixture_endpoint_not_allowed")),
            }
        })
    }
}

/// Retains the old source contract so each page prepares its own production token.
struct LegacySource { client: BiliClient }

impl AcquisitionSource for LegacySource {
    fn image_index<'a>(&'a self, episode_id: &'a str) -> AcquisitionFuture<'a, ImageIndex> {
        <BiliClient as AcquisitionSource>::image_index(&self.client, episode_id)
    }

    fn image_index_for<'a>(&'a self, comic_id: &'a str, episode_id: &'a str) -> AcquisitionFuture<'a, ImageIndex> {
        <BiliClient as AcquisitionSource>::image_index_for(&self.client, comic_id, episode_id)
    }

    fn download_page<'a>(&'a self, index: &'a ImageIndex, page_index: usize) -> AcquisitionFuture<'a, AcquiredPage> {
        <BiliClient as AcquisitionSource>::download_page(&self.client, index, page_index)
    }
}

fn synthetic_session() -> AuthSession {
    let session = AuthSession::from_cookie_text(concat!(
        "SESSDATA=synthetic-batching-session; bili_jct=synthetic-batching-csrf; ",
        "DedeUserID=424244; buvid3=synthetic-batching-device")).unwrap()
        .with_identity(AccountIdentity { id:"424244".into(), name:"Synthetic batching account".into(),
            avatar_url:String::new() }, 1_800_000_000).unwrap();
    assert_eq!(session.account_key(), Some(ACCOUNT));
    session
}

fn download_request(page_indices: Option<Vec<usize>>) -> DownloadRequest {
    DownloadRequest { comic_id:COMIC.into(), episode_id:EPISODE.into(), title:"Synthetic batching episode".into(),
        comic_title:"Synthetic batching comic".into(), retain:true, page_indices, ..Default::default() }
}

fn write_json(path: &Path, value: &Value) {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path).unwrap();
    serde_json::to_writer_pretty(&mut file, value).unwrap();
    file.sync_all().unwrap();
}

#[derive(Default)]
struct Timing {
    elapsed_ms: f64,
    first_commit_ms: Option<f64>,
    first_committed_page: Option<usize>,
    page_zero_commit_ms: Option<f64>,
}

fn observe_events(manager: &mut DownloadManager, started: Instant, timing: &mut Timing,
    pages: &[FixturePage], missing: &[usize]) {
    for event in manager.drain_events(1024) {
        if let DownloadEventKind::Failed { code, .. } = &event.kind { panic!("Acquisition failed: {code}"); }
        let DownloadEventKind::PageReady { page_index, page_id, revision, path, .. } = event.kind else { continue; };
        if !missing.contains(&page_index) { continue; }
        if timing.first_commit_ms.is_none() || page_index == 0 && timing.page_zero_commit_ms.is_none() {
            // Reopen independently; a PageReady event alone is not durable commit evidence.
            let store = Store::open(manager.database_path()).unwrap();
            let receipt = store.page_receipt(ACCOUNT, EPISODE, &revision, &page_id).unwrap().unwrap();
            assert_eq!(receipt.page_index, page_index);
            assert_eq!(receipt.sha256, pages[page_index].sha256);
            assert_eq!(digest(&fs::read(path).unwrap()), pages[page_index].sha256);
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            if timing.first_commit_ms.is_none() {
                timing.first_commit_ms = Some(elapsed);
                timing.first_committed_page = Some(page_index);
            }
            if page_index == 0 { timing.page_zero_commit_ms = Some(elapsed); }
        }
    }
}

async fn wait_terminal(manager: &mut DownloadManager, expected: DownloadStatus, started: Instant,
    pages: &[FixturePage], missing: &[usize], timeout: Duration) -> Timing {
    let mut timing = Timing::default();
    tokio::time::timeout(timeout, async {
        loop {
            observe_events(manager, started, &mut timing, pages, missing);
            let records = manager.records().unwrap();
            assert_eq!(records.len(), 1);
            let record = &records[0];
            assert_ne!(record.status, DownloadStatus::Failed, "Acquisition failed: {:?}", record.error_code);
            if record.status == expected {
                observe_events(manager, started, &mut timing, pages, missing);
                timing.elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }).await.expect("Controlled acquisition exceeded its phase deadline");
    timing
}

fn verify_receipts(manager: &DownloadManager, expected_pages: &[usize], pages: &[FixturePage]) -> Value {
    let store = Store::open(manager.database_path()).unwrap();
    let record = store.downloads(ACCOUNT).unwrap().pop().unwrap();
    assert_eq!(record.completed_pages as usize, expected_pages.len());
    assert_eq!(record.total_pages as usize, PAGE_COUNT);
    let index = store.load_index(ACCOUNT, EPISODE, Some(&record.revision)).unwrap().unwrap();
    assert!(index.paths.is_empty(), "Transient source paths must never enter durable metadata");
    let mut receipts = store.page_receipts(ACCOUNT, EPISODE, &record.revision).unwrap();
    receipts.sort_by_key(|receipt| receipt.page_index);
    assert_eq!(receipts.iter().map(|receipt| receipt.page_index).collect::<Vec<_>>(), expected_pages.to_vec());
    let mut fingerprint = Vec::new();
    for receipt in &receipts {
        let page = receipt.page_index;
        assert_eq!(receipt.page_id, index.images[page].id);
        assert_eq!(receipt.page_id, digest(format!("/synthetic-batching/page-{page}").as_bytes()));
        assert_eq!((receipt.width, receipt.height), (IMAGE_WIDTH, IMAGE_HEIGHT));
        assert_eq!(receipt.sha256, pages[page].sha256);
        assert_eq!(receipt.bytes, pages[page].bytes.len() as u64);
        assert!(receipt.retained);
        let path = manager.local_page(EPISODE, &record.revision, &receipt.page_id).unwrap().unwrap();
        let bytes = fs::read(path).unwrap();
        assert_eq!(bytes, pages[page].bytes);
        let image = image::load_from_memory(&bytes).unwrap().to_luma8();
        assert_eq!(digest(image.as_raw()), pages[page].pixels_sha256);
        fingerprint.push(format!("{page}:{}:{}:{}:{}:{}", receipt.page_id, receipt.sha256,
            receipt.bytes, receipt.width, receipt.height));
    }
    assert_eq!(record.bytes, receipts.iter().map(|receipt| receipt.bytes).sum::<u64>());
    assert!(store.downloads("bili_424245").unwrap().is_empty());
    store.checkpoint().unwrap();
    json!({"receipt_count":receipts.len(), "receipt_content_sha256":digest(fingerprint.join("\n").as_bytes()),
        "persisted_bytes":record.bytes,"revision":record.revision,"all_byte_hashes_match":true,
        "all_decoded_pixels_match":true,"transient_paths_persisted":false,"account_isolation":true})
}

enum Operation { Enqueue(Option<Vec<usize>>), Resume, Cached }

async fn run_phase(root: &Path, data: &Path, mode: Mode, configuration: &Configuration,
    pages: Arc<Vec<FixturePage>>, missing: &[usize], committed: &[usize], operation: Operation) -> Value {
    fs::create_dir(root).unwrap();
    let audit = Arc::new(Mutex::new(Audit::default()));
    let started = Arc::new(OnceLock::new());
    let transport = Arc::new(FixtureTransport { pages:pages.clone(), allowed_pages:missing.to_vec(),
        token_latency:configuration.token_latency, started:started.clone(), audit:audit.clone() });
    let unused_assets = root.join("unused-fixture-assets");
    let crypto = Arc::new(AuditedCrypto {
        native:NativeCrypto::new(configuration.native_assets.as_deref().unwrap_or(&unused_assets)),
        native_signing:configuration.signer == "native", audit:audit.clone() });
    let client = BiliClient::with_adapters(Some(synthetic_session()), data, transport, crypto)
        .bound_to_account(ACCOUNT).unwrap();
    let source: Arc<dyn AcquisitionSource> = match mode {
        Mode::Legacy => Arc::new(LegacySource { client }), Mode::Batched => Arc::new(client),
    };
    let mut manager = DownloadManager::new(data, ACCOUNT.into(), 1, source, tokio::runtime::Handle::current()).unwrap();
    manager.set_concurrency(CONCURRENCY).unwrap();
    // Both paths start with the same initialized store; page writes remain inside the measured interval.
    let measured = Instant::now();
    started.set(measured).unwrap();
    let expected_status = match operation {
        Operation::Enqueue(selection) => {
            let expected = if selection.is_some() { DownloadStatus::Paused } else { DownloadStatus::Completed };
            manager.enqueue(download_request(selection)).unwrap();
            expected
        }
        Operation::Resume => {
            manager.recover().unwrap();
            let record = manager.records().unwrap().pop().unwrap();
            assert_eq!(record.status, DownloadStatus::Paused);
            manager.resume(&record.id).unwrap();
            DownloadStatus::Completed
        }
        Operation::Cached => {
            manager.recover().unwrap();
            manager.enqueue(download_request(None)).unwrap();
            DownloadStatus::Completed
        }
    };
    let timeout = Duration::from_secs(180) + configuration.token_latency * PAGE_COUNT as u32;
    let timing = wait_terminal(&mut manager, expected_status, measured, &pages, missing, timeout).await;
    let verified = verify_receipts(&manager, committed, &pages);
    let state = audit.lock().unwrap();
    let mut token_pages = state.token_batches.iter().flatten().copied().collect::<Vec<_>>();
    token_pages.sort_unstable();
    assert_eq!(token_pages.as_slice(), missing);
    let expected_sizes = match mode {
        Mode::Legacy => vec![1; missing.len()],
        Mode::Batched => missing.chunks(20).map(|chunk| chunk.len()).collect::<Vec<_>>(),
    };
    let batch_sizes = state.token_batches.iter().map(Vec::len).collect::<Vec<_>>();
    assert_eq!(batch_sizes, expected_sizes);
    let mut cdn_pages = state.cdn_pages.clone();
    cdn_pages.sort_unstable();
    assert_eq!(cdn_pages.as_slice(), missing);
    assert_eq!(state.cdn_in_flight, 0);
    assert!(state.maximum_cdn_in_flight <= CONCURRENCY);
    let index_requests = state.requests.iter().filter(|request| request["endpoint"] == "GetImageIndex").count();
    let detail_requests = state.requests.iter().filter(|request| request["endpoint"] == "ComicDetail").count();
    assert_eq!(index_requests, usize::from(!missing.is_empty()));
    assert_eq!(detail_requests, index_requests, "Every fresh index must follow a current permission check");
    assert_eq!(state.signatures.len(), detail_requests + index_requests + state.token_batches.len());
    assert_eq!(state.image_keys, state.token_batches.len());
    assert_eq!(state.requests.len(), detail_requests + index_requests + state.token_batches.len() + missing.len());
    for endpoint in ["ComicDetail", "GetImageIndex", "ImageToken"] {
        let mut sent = state.requests.iter().filter(|request| request["endpoint"] == endpoint)
            .map(|request| request["signature_sha256"].as_str().unwrap()).collect::<Vec<_>>();
        let mut signed = state.signatures.iter().filter(|signature| signature["endpoint"] == endpoint)
            .map(|signature| signature["signature_sha256"].as_str().unwrap()).collect::<Vec<_>>();
        sent.sort_unstable(); signed.sort_unstable();
        assert_eq!(sent, signed, "Every actual request must carry its audited signature");
    }
    if !missing.is_empty() {
        assert!(timing.first_commit_ms.is_some());
        if missing.contains(&0) { assert!(timing.page_zero_commit_ms.is_some()); }
    } else {
        assert!(state.requests.is_empty(), "A complete cache must work without any transport request");
    }
    write_json(&root.join("requests.json"), &json!(state.requests));
    write_json(&root.join("signatures.json"), &json!(state.signatures));
    let signing_ms = state.signatures.iter().map(|signature| signature["elapsed_ms"].as_f64().unwrap()).sum::<f64>();
    let result = json!({"passed":true,"mode":mode.label(),"elapsed_ms":timing.elapsed_ms,
        "first_durable_commit_observed_ms":timing.first_commit_ms,
        "first_committed_page_index":timing.first_committed_page,
        "page_zero_durable_commit_observed_ms":timing.page_zero_commit_ms,
        "first_commit_observer_poll_ms":2,"token_requests":state.token_batches.len(),
        "token_batch_sizes":batch_sizes,"token_page_indices":token_pages,"cdn_requests":state.cdn_pages.len(),
        "cdn_page_indices":cdn_pages,"detail_requests":detail_requests,"index_requests":index_requests,"total_requests":state.requests.len(),
        "signatures":state.signatures.len(),"image_keys":state.image_keys,"signing_elapsed_ms_sum":signing_ms,
        "configured_page_concurrency":CONCURRENCY,"maximum_observed_cdn_concurrency":state.maximum_cdn_in_flight,
        "durable_cache":verified,"request_audit":"requests.json","signature_audit":"signatures.json"});
    drop(state);
    write_json(&root.join("result.json"), &result);
    result
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    (values[middle - 1] + values[middle]) / 2.0
}

async fn benchmark(root: &Path, configuration: Configuration) {
    let pages = fixture_pages();
    let all_pages = (0..PAGE_COUNT).collect::<Vec<_>>();
    let mut cold_runs = Vec::new();
    let mut cached_runs = Vec::new();
    let mut execution_order = Vec::new();
    for repetition in 0..configuration.repetitions {
        let modes = if repetition % 2 == 0 { [Mode::Legacy, Mode::Batched] } else { [Mode::Batched, Mode::Legacy] };
        for mode in modes {
            let relative = format!("round-{repetition:02}-{}", mode.label());
            let run_root = root.join(&relative);
            fs::create_dir(&run_root).unwrap();
            let data = run_root.join("data");
            let cold = run_phase(&run_root.join("cold"), &data, mode, &configuration, pages.clone(),
                &all_pages, &all_pages, Operation::Enqueue(None)).await;
            let cached = run_phase(&run_root.join("cached-reopen"), &data, mode, &configuration, pages.clone(),
                &[], &all_pages, Operation::Cached).await;
            execution_order.push(json!({"repetition":repetition,"mode":mode.label(),"path":relative}));
            cold_runs.push(cold);
            cached_runs.push(cached);
        }
    }
    let selected = vec![0,7,21,44];
    let missing = all_pages.iter().filter(|page| !selected.contains(page)).copied().collect::<Vec<_>>();
    let mut partial_runs = Vec::new();
    for mode in [Mode::Legacy, Mode::Batched] {
        let relative = format!("partial-{}", mode.label());
        let run_root = root.join(&relative);
        fs::create_dir(&run_root).unwrap();
        let data = run_root.join("data");
        let prime = run_phase(&run_root.join("prime"), &data, mode, &configuration, pages.clone(),
            &selected, &selected, Operation::Enqueue(Some(selected.clone()))).await;
        let resumed = run_phase(&run_root.join("resume-after-reopen"), &data, mode, &configuration, pages.clone(),
            &missing, &all_pages, Operation::Resume).await;
        let cached = run_phase(&run_root.join("cached-reopen"), &data, mode, &configuration, pages.clone(),
            &[], &all_pages, Operation::Cached).await;
        partial_runs.push(json!({"path":relative,"mode":mode.label(),"prime":prime,"resume":resumed,"cached":cached}));
    }
    let expected_fingerprint = cold_runs[0]["durable_cache"]["receipt_content_sha256"].clone();
    for result in cold_runs.iter().chain(cached_runs.iter()) {
        assert_eq!(result["durable_cache"]["receipt_content_sha256"], expected_fingerprint);
    }
    for result in &partial_runs {
        assert_eq!(result["resume"]["durable_cache"]["receipt_content_sha256"], expected_fingerprint);
        assert_eq!(result["cached"]["durable_cache"]["receipt_content_sha256"], expected_fingerprint);
    }
    let mut legacy_ms = cold_runs.iter().filter(|run| run["mode"] == Mode::Legacy.label())
        .map(|run| run["elapsed_ms"].as_f64().unwrap()).collect::<Vec<_>>();
    let mut batched_ms = cold_runs.iter().filter(|run| run["mode"] == Mode::Batched.label())
        .map(|run| run["elapsed_ms"].as_f64().unwrap()).collect::<Vec<_>>();
    let legacy_median = median(&mut legacy_ms);
    let batched_median = median(&mut batched_ms);
    let result = json!({"passed":true,"evidence_kind":"synthetic_production_download_batching",
        "physical_device":false,"live_account":false,"live_service_verified":false,
        "actual_network_speed_measured":false,"firmware_emulator":false,"encrypted_content_decode_verified":false,
        "signer":if configuration.signer == "native" { "native_crypto_pinned_wasm" } else { "injected_fixture" },
        "native_signing_executed":configuration.signer == "native",
        "native_asset_sha256":if configuration.signer == "native" { Some(ASSET_SHA256) } else { None },
        "page_count":PAGE_COUNT,"configured_page_concurrency":CONCURRENCY,
        "injected_token_response_latency_ms":configuration.token_latency.as_millis(),
        "injected_cdn_response_latency_ms":CDN_LATENCY_MS,"image_payload":"identical_synthetic_plain_png",
        "repetitions_per_mode":configuration.repetitions,"execution_order":execution_order,
        "legacy_cold_token_requests":45,"batched_cold_token_requests":3,"batched_cold_token_batch_sizes":[20,20,5],
        "cdn_requests_per_cold_run":45,"cold_runs":cold_runs,"cached_runs":cached_runs,"partial_runs":partial_runs,
        "legacy_cold_elapsed_ms_median":legacy_median,"batched_cold_elapsed_ms_median":batched_median,
        "controlled_cold_elapsed_ratio":legacy_median/batched_median,
        "receipt_content_sha256":expected_fingerprint,"all_output_bytes_and_pixels_match":true,
        "all_complete_cache_reopens_have_zero_requests":true,
        "timing_scope":"production source acquisition, injected response delays, image inspection, and durable page receipts; fixture construction and final readback excluded",
        "timing_limit":"controlled remote fixture timings only; no actual service, Kindle, or real network throughput claim",
        "partial_scope":"selected-page commit, manager reopen, explicit resume of missing pages, complete offline cache reuse; process crash is covered separately by production_flow"});
    write_json(&root.join("result.json"), &result);
    println!("DOWNLOAD_BATCHING_RESULT {}", json!({"passed":true,"signer":configuration.signer,
        "legacy_token_requests":45,"batched_token_requests":3,"cdn_requests":45,
        "legacy_cold_elapsed_ms_median":legacy_median,"batched_cold_elapsed_ms_median":batched_median}));
}

#[test]
fn controlled_download_batching_reduces_tokens_without_changing_commits() {
    let configuration = Configuration::read();
    let temporary = tempfile::tempdir().unwrap();
    let root = std::env::var_os("BILICOMICS_BATCH_OUTPUT").map(PathBuf::from)
        .unwrap_or_else(|| temporary.path().join("download-batching"));
    fs::create_dir_all(root.parent().unwrap()).unwrap();
    fs::create_dir(&root).expect("Each benchmark requires a fresh evidence directory");
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap()
        .block_on(benchmark(&root, configuration));
}
