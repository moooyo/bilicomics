//! Controlled subprocess evidence for the production source, cache, and reader services.
//!
//! These tests never use the live service. The transport is a closed in-memory
//! responder, credentials and images are synthetic, and child processes are
//! killed only after an independently reopened SQLite connection sees the commit.

#![cfg(target_os = "linux")]

use std::{
    fs::{self, File, OpenOptions},
    io::{Cursor, Write},
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}},
    thread,
    time::{Duration, Instant},
};

use bilicomics::{
    app::{App, AppOptions},
    domain::ImageIndex,
    downloads::{DownloadManager, DownloadRequest},
    platform::DeviceProfile,
    reader::{ReaderCore, ReaderSettings, Viewport},
    source::{
        BiliClient, QrLoginState, SessionFile, SourceError,
        crypto::{ImageKey, NativeCrypto, ProtocolCrypto, RequestContext, Signature},
        transport::{HttpRequest, HttpResponse, ResponseFuture, Transport},
    },
    store::{DownloadStatus, Store},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const ACCOUNT: &str = "bili_424242";
const OTHER_ACCOUNT: &str = "bili_525252";
const COMIC: &str = "900001";
const EPISODE: &str = "900002";
const IMAGE_WIDTH: u32 = 96;
const IMAGE_HEIGHT: u32 = 144;
const FLOW_TEST: &str = "source_download_process_crash_recovers_missing_pages_and_offline_anchor";
const TRANSACTION_TEST: &str = "interrupted_transaction_intents_stay_unknown_after_preparation_restart";

struct FixtureCrypto;

impl ProtocolCrypto for FixtureCrypto {
    fn sign(&self, context: &RequestContext) -> bilicomics::source::Result<Signature> {
        assert_eq!(context.sign_query, "device=pc&platform=web&nov=27&eot=812");
        assert!(serde_json::from_str::<Value>(&context.body).unwrap().is_object());
        Ok(Signature { ultra_sign: "synthetic-service-flow".into(), data_sn: "synthetic-sn".into() })
    }

    fn decode_response(&self, _: &RequestContext, _: Value) -> bilicomics::source::Result<Value> {
        Err(SourceError::protocol("unexpected_fixture_response_cipher"))
    }

    fn new_image_key(&self) -> bilicomics::source::Result<ImageKey> {
        NativeCrypto::new(Path::new("unused-fixture-assets")).new_image_key()
    }

    fn decrypt_image(&self, _: &ImageKey, _: &str, _: &[u8]) -> bilicomics::source::Result<Vec<u8>> {
        Err(SourceError::protocol("unexpected_fixture_image_cipher"))
    }
}

struct FixtureTransport {
    root: PathBuf,
    phase: String,
    audit: Mutex<File>,
    token_requests: AtomicUsize,
    kill_checkpoint_written: AtomicBool,
}

impl FixtureTransport {
    fn new(root: &Path, phase: &str) -> Self {
        let audit = OpenOptions::new().create_new(true).write(true)
            .open(root.join(format!("http-{phase}.jsonl"))).unwrap();
        Self { root: root.into(), phase: phase.into(), audit: Mutex::new(audit),
            token_requests: AtomicUsize::new(0), kill_checkpoint_written: AtomicBool::new(false) }
    }

    fn record(&self, endpoint: &str, page: Option<usize>) {
        let mut audit = self.audit.lock().unwrap();
        writeln!(audit, "{}", json!({"endpoint": endpoint, "page_index": page})).unwrap();
        audit.sync_all().unwrap();
    }

    fn record_token_batch(&self, pages: &[usize]) {
        let request_id = self.token_requests.fetch_add(1, Ordering::SeqCst);
        let mut audit = self.audit.lock().unwrap();
        for (position, page) in pages.iter().enumerate() {
            writeln!(audit, "{}", json!({"endpoint":"ImageToken", "page_index":page,
                "request_id":request_id, "batch_position":position, "batch_pages":pages})).unwrap();
        }
        audit.sync_all().unwrap();
    }
}

impl Transport for FixtureTransport {
    fn send(&self, request: HttpRequest) -> ResponseFuture<'_> {
        Box::pin(async move {
            let url = url::Url::parse(&request.url).unwrap();
            let path = url.path();
            let endpoint = path.rsplit('/').next().unwrap_or_default();
            let body = || serde_json::from_slice::<Value>(&request.body).unwrap();
            let reply = match (url.host_str().unwrap(), path) {
                ("passport.bilibili.com", "/x/passport-login/web/qrcode/generate") => {
                    self.record("qr-generate", None);
                    envelope(json!({"qrcode_key":"synthetic-flow-key",
                        "url":"https://account.bilibili.com/h5/account-h5/auth/scan-web?fixture=1"}))
                }
                ("passport.bilibili.com", "/x/passport-login/web/qrcode/poll") => {
                    self.record("qr-poll", None);
                    let mut response = envelope(json!({"code":0,"refresh_token":"synthetic-flow-refresh"}));
                    response.headers = [
                        "SESSDATA=synthetic-flow-session; Domain=.bilibili.com; Path=/",
                        "bili_jct=synthetic-flow-csrf; Domain=.bilibili.com; Path=/",
                        "DedeUserID=424242; Domain=.bilibili.com; Path=/",
                    ].map(|value| ("set-cookie".into(), value.into())).into();
                    response
                }
                ("manga.bilibili.com", "/ductape/buvid") => {
                    assert!(!has_header(&request, "cookie"));
                    self.record("manga-device", None);
                    let mut response = envelope(json!({}));
                    response.headers.push(("set-cookie".into(),
                        "buvid3=synthetic-flow-device; Domain=.bilibili.com; Path=/".into()));
                    response
                }
                ("api.bilibili.com", "/x/web-interface/nav") => {
                    assert!(has_header(&request, "cookie"));
                    self.record("account-validation", None);
                    envelope(json!({"isLogin":true,"mid":424242,"uname":"Synthetic service-flow account"}))
                }
                ("manga.bilibili.com", "/twirp/comic.v1.Comic/ComicDetail") => {
                    assert_eq!(body()["comic_id"], 900001);
                    self.record("ComicDetail", None);
                    envelope(json!({"id":900001,"title":"Synthetic recovery comic", "author_name":["Fixture"],
                        "ep_list":[{"id":900002,"title":"Synthetic episode", "short_title":"1", "ord":1,
                            "is_locked":false,"pay_mode":0}]}))
                }
                ("manga.bilibili.com", "/twirp/comic.v1.Comic/GetImageIndex") => {
                    assert_eq!(body()["ep_id"], 900002);
                    self.record("GetImageIndex", None);
                    envelope(json!({"images":(0..3).map(|page| json!({
                        "path":format!("/synthetic-flow/page-{page}"), "x":IMAGE_WIDTH,"y":IMAGE_HEIGHT
                    })).collect::<Vec<_>>()}))
                }
                ("manga.bilibili.com", "/twirp/comic.v1.Comic/ImageToken") => {
                    let body = body();
                    let paths: Vec<String> = serde_json::from_str(body["urls"].as_str().unwrap()).unwrap();
                    assert!((1..=20).contains(&paths.len()));
                    assert_eq!(body["m1"].as_str().unwrap().len(), 88);
                    let pages = paths.iter().map(|path| {
                        let page = path.strip_prefix("/synthetic-flow/page-").unwrap().parse::<usize>().unwrap();
                        assert!(page < 3);
                        page
                    }).collect::<Vec<_>>();
                    assert!(pages.windows(2).all(|pair| pair[0] < pair[1]));
                    if self.phase == "recover" { assert!(!pages.contains(&0), "A committed page must not be tokenized again"); }
                    if self.phase == "repair" { assert_eq!(pages, [0], "Only the corrupt page needs a token"); }
                    self.record_token_batch(&pages);
                    envelope(json!(pages.iter().map(|page| json!({
                        "complete_url":format!("https://i0.hdslb.com/synthetic-flow-page-{page}.png?token=synthetic"),
                        "hit_encrpyt":true})).collect::<Vec<_>>()))
                }
                ("i0.hdslb.com", _) if path.starts_with("/synthetic-flow-page-") => {
                    assert_eq!(request.method, "GET");
                    assert!(!has_header(&request, "cookie"));
                    assert!(!has_header(&request, "x-xsrf-token"));
                    let page = path.strip_prefix("/synthetic-flow-page-").unwrap()
                        .strip_suffix(".png").unwrap().parse::<usize>().unwrap();
                    assert!(page < 3);
                    if self.phase == "recover" { assert_ne!(page, 0, "A committed page must be reused"); }
                    if self.phase == "repair" { assert_eq!(page, 0, "Only the corrupt page needs repair"); }
                    self.record("CDN", Some(page));
                    if self.phase == "initial" && page != 0 {
                        let store = Store::open(self.root.join("data/state.sqlite3")).unwrap();
                        let task = store.downloads(ACCOUNT).unwrap().pop().unwrap();
                        assert_eq!(task.status, DownloadStatus::Running);
                        assert_eq!(task.completed_pages, 1);
                        let receipts = store.page_receipts(ACCOUNT, EPISODE, &task.revision).unwrap();
                        assert_eq!(receipts.len(), 1);
                        assert_eq!(receipts[0].page_index, 0);
                        if !self.kill_checkpoint_written.swap(true, Ordering::SeqCst) {
                            write_json(&self.root.join("ready-to-kill.json"), &json!({"pid":std::process::id(),
                                "committed_pages":1,"receipt_count":1,"committed_page_index":0,
                                "blocked_page_index":page,"task_status":task.status.as_str()}));
                        }
                        std::future::pending::<()>().await;
                    }
                    HttpResponse { status:200, headers:vec![("content-type".into(),"image/png".into())], body:fixture_png(page) }
                }
                _ => {
                    self.record("REJECTED", None);
                    return Err(SourceError::protocol("fixture_endpoint_not_allowed"));
                }
            };
            assert!(!matches!(endpoint,"BuyEpisode"|"CreateOrder"|"AddHistory"|"AddFavorite"|"DeleteFavorite"));
            Ok(reply)
        })
    }
}

fn has_header(request: &HttpRequest, name: &str) -> bool {
    request.headers.iter().any(|(key, _)| key.eq_ignore_ascii_case(name))
}

fn envelope(data: Value) -> HttpResponse {
    HttpResponse { status:200, headers:Vec::new(), body:serde_json::to_vec(&json!({"code":0,"data":data})).unwrap() }
}

fn fixture_png(page: usize) -> Vec<u8> {
    let image = image::GrayImage::from_fn(IMAGE_WIDTH, IMAGE_HEIGHT, |x, y| {
        let ink = ((x / 8 + y / 12 + page as u32) % 4 == 0) || x < 3 || y < 3;
        image::Luma([if ink { 32 } else { 248 }])
    });
    let mut output = Cursor::new(Vec::new());
    image::DynamicImage::ImageLuma8(image).write_to(&mut output, image::ImageFormat::Png).unwrap();
    output.into_inner()
}

fn write_json(path: &Path, value: &Value) {
    let temporary = path.with_extension("pending");
    let mut file = File::create(&temporary).unwrap();
    serde_json::to_writer_pretty(&mut file, value).unwrap();
    file.sync_all().unwrap();
    fs::rename(temporary, path).unwrap();
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn output_root(case: &str) -> (tempfile::TempDir, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let root = std::env::var_os("BILICOMICS_FLOW_OUTPUT").map(PathBuf::from)
        .unwrap_or_else(|| temporary.path().to_owned()).join(case);
    fs::create_dir_all(root.parent().unwrap()).unwrap();
    fs::create_dir(&root).expect("Each flow run requires a fresh evidence directory");
    (temporary, root)
}

struct Worker(Child);

impl Drop for Worker {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn spawn_worker(test_name: &str, phase: &str, root: &Path) -> Worker {
    let log = File::create(root.join(format!("{phase}.log"))).unwrap();
    Worker(Command::new(std::env::current_exe().unwrap()).args(["--exact",test_name,"--nocapture"])
        .env("BILICOMICS_FLOW_WORKER", phase).env("BILICOMICS_FLOW_ROOT", root)
        .stdout(Stdio::from(log.try_clone().unwrap())).stderr(Stdio::from(log)).spawn().unwrap())
}

fn wait_file(path: &Path, worker: &mut Worker) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !path.is_file() {
        assert!(worker.0.try_wait().unwrap().is_none(), "Worker exited before its durable checkpoint");
        assert!(Instant::now() < deadline, "Worker checkpoint timed out");
        thread::sleep(Duration::from_millis(10));
    }
}

fn run_worker(test_name: &str, phase: &str, root: &Path) {
    let mut worker = spawn_worker(test_name, phase, root);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = worker.0.try_wait().unwrap() {
            assert!(status.success(), "Worker failed; inspect its phase log");
            return;
        }
        assert!(Instant::now() < deadline, "Worker completion timed out");
        thread::sleep(Duration::from_millis(10));
    }
}

fn worker_context() -> Option<(String, PathBuf)> {
    std::env::var("BILICOMICS_FLOW_WORKER").ok().map(|phase| {
        (phase, PathBuf::from(std::env::var_os("BILICOMICS_FLOW_ROOT").unwrap()))
    })
}

async fn wait_completed(manager: &DownloadManager) {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let records = manager.records().unwrap();
            if records.first().is_some_and(|record| record.status == DownloadStatus::Completed) { break; }
            assert!(!records.iter().any(|record| record.status == DownloadStatus::Failed), "Acquisition failed");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("Acquisition did not finish");
}

async fn service_worker(phase: &str, root: &Path) {
    let data = root.join("data");
    fs::create_dir_all(&data).unwrap();
    let session_file = SessionFile::new(&data.join("account"));
    let transport = Arc::new(FixtureTransport::new(root, phase));
    let client = BiliClient::with_adapters(session_file.load().unwrap(), &data, transport,
        Arc::new(FixtureCrypto));
    if phase == "initial" {
        let qr = client.begin_qr().await.unwrap();
        let QrLoginState::Confirmed(session) = client.poll_qr(&qr.key).await.unwrap() else { panic!("Confirmed candidate expected"); };
        assert!(client.session().is_none());
        assert_eq!(session.account_key(), Some(ACCOUNT));
        session_file.save(&session).unwrap();
        assert_eq!(session_file.load().unwrap().unwrap().account_key(), Some(ACCOUNT));
        client.replace_session(Some(session)).unwrap();
        let detail = client.comic_detail(COMIC).await.unwrap();
        assert_eq!(detail.episodes.len(), 1);
        assert!(detail.episodes[0].readable_at(1_800_000_000));
        let mut store = Store::open(data.join("state.sqlite3")).unwrap();
        store.save_comics(ACCOUNT, std::slice::from_ref(&detail.comic)).unwrap();
        store.save_detail(ACCOUNT, &detail).unwrap();
        store.set_preference("device", "last_account", &ACCOUNT).unwrap();
        store.set_preference(ACCOUNT, "preferences", &json!({"onboarded":true,"reader_guidance_seen":true,"launch":"resume"})).unwrap();
    }
    let mut manager = DownloadManager::new(&data, ACCOUNT.into(), 1, Arc::new(client), tokio::runtime::Handle::current()).unwrap();
    manager.set_concurrency(2).unwrap();
    if phase == "initial" {
        let request = DownloadRequest { comic_id:COMIC.into(), episode_id:EPISODE.into(),
            title:"Synthetic episode".into(),comic_title:"Synthetic recovery comic".into(),retain:true,..Default::default() };
        manager.enqueue(DownloadRequest { page_indices:Some(vec![0]), ..request.clone() }).unwrap();
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let record = manager.records().unwrap().pop().unwrap();
                if record.status == DownloadStatus::Paused {
                    assert_eq!(record.completed_pages, 1);
                    break;
                }
                assert_ne!(record.status, DownloadStatus::Failed);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("The first durable source page was not committed");
        manager.enqueue(request).unwrap();
        std::future::pending::<()>().await;
    }
    if phase == "recover" {
        manager.recover().unwrap();
        wait_completed(&manager).await;
    } else if phase == "repair" {
        manager.recover().unwrap();
        let record = manager.records().unwrap().pop().unwrap();
        assert_eq!(record.status, DownloadStatus::Failed);
        assert_eq!(record.error_code.as_deref(), Some("local_page_missing"));
        assert_eq!(record.completed_pages, 2);
        manager.retry(&record.id).unwrap();
        wait_completed(&manager).await;
    } else {
        assert_eq!(phase, "offline");
    }
    let store = Store::open(data.join("state.sqlite3")).unwrap();
    let record = store.downloads(ACCOUNT).unwrap().pop().unwrap();
    assert_eq!(record.completed_pages, 3);
    let index: ImageIndex = store.load_index(ACCOUNT, EPISODE, Some(&record.revision)).unwrap().unwrap();
    assert!(index.paths.is_empty());
    assert_eq!(store.page_receipts(ACCOUNT, EPISODE, &record.revision).unwrap().len(), 3);
    assert!(store.downloads(OTHER_ACCOUNT).unwrap().is_empty());
    assert!(store.load_index(OTHER_ACCOUNT, EPISODE, None).unwrap().is_none());
    let other = DownloadManager::new(&data, OTHER_ACCOUNT.into(), 1,
        Arc::new(BiliClient::offline(&data).unwrap()), tokio::runtime::Handle::current()).unwrap();
    assert!(other.local_page(EPISODE, &record.revision, &index.images[1].id).unwrap().is_none());
    let mut reader = ReaderCore::new(index.clone(), ReaderSettings::default()).unwrap();
    if phase == "recover" {
        reader.set_page(1).unwrap();
        store.save_anchor(ACCOUNT, COMIC, &reader.anchor(1_800_000_001)).unwrap();
    }
    let anchor = store.load_anchor(ACCOUNT, COMIC).unwrap().unwrap();
    assert_eq!(anchor.page_index, 1);
    reader.restore(&anchor).unwrap();
    let local = manager.local_page(EPISODE, &record.revision, &anchor.page_id).unwrap().unwrap();
    let rendered = reader.render(&local, Viewport::new(480, 720)).unwrap();
    assert_eq!(rendered.page_index, 1);
    let raster_digest = format!("{:x}", Sha256::digest(&rendered.rgba));
    image::RgbaImage::from_raw(rendered.width, rendered.height, rendered.rgba).unwrap()
        .save(root.join(format!("{phase}-reader.png"))).unwrap();
    store.checkpoint().unwrap();
    let audit = fs::read_to_string(root.join(format!("http-{phase}.jsonl"))).unwrap();
    assert!(!audit.contains("REJECTED"));
    if phase == "offline" { assert!(audit.is_empty()); }
    let audit_entries = audit.lines().map(|line| serde_json::from_str::<Value>(line).unwrap()).collect::<Vec<_>>();
    let mut token_pages = audit_entries.iter().filter(|entry| entry["endpoint"] == "ImageToken")
        .map(|entry| entry["page_index"].as_u64().unwrap() as usize).collect::<Vec<_>>();
    token_pages.sort_unstable();
    let mut cdn_pages = audit_entries.iter().filter(|entry| entry["endpoint"] == "CDN")
        .map(|entry| entry["page_index"].as_u64().unwrap() as usize).collect::<Vec<_>>();
    cdn_pages.sort_unstable();
    let expected_pages: Vec<usize> = match phase { "recover" => vec![1,2], "repair" => vec![0], "offline" => vec![], _ => unreachable!() };
    assert_eq!(token_pages, expected_pages);
    assert_eq!(cdn_pages, expected_pages);
    let token_requests = audit_entries.iter().filter(|entry| entry["endpoint"] == "ImageToken" && entry["batch_position"] == 0).count();
    assert_eq!(token_requests, usize::from(!expected_pages.is_empty()));
    let http_requests = audit_entries.iter().filter(|entry| entry["endpoint"] != "ImageToken").count() + token_requests;
    write_json(&root.join(format!("{phase}-result.json")), &json!({"passed":true,"pid":std::process::id(),
        "evidence_kind":"synthetic_source_services_subprocess","production_app_click_flow":false,
        "real_account":false,"live_service_verified":false,"signer":"injected_fixture",
        "image_payload":"synthetic_plain_png_with_encryption_flag","encrypted_content_decode_verified":false,
        "comic_id":COMIC,"episode_id":EPISODE,"page_index":reader.page_index(),"total_pages":3,
        "local_receipts":3,"account_isolation":true,"transient_paths_persisted":false,
        "raster_sha256":raster_digest,"http_requests":http_requests,"http_audit_entries":audit_entries.len(),
        "token_requests":token_requests,"token_page_indices":token_pages,"cdn_page_indices":cdn_pages}));
}

#[test]
fn source_download_process_crash_recovers_missing_pages_and_offline_anchor() {
    if let Some((phase, root)) = worker_context() {
        tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap()
            .block_on(service_worker(&phase, &root));
        return;
    }
    let (_temporary, root) = output_root("source-services");
    let mut first = spawn_worker(FLOW_TEST, "initial", &root);
    wait_file(&root.join("ready-to-kill.json"), &mut first);
    let checkpoint = read_json(&root.join("ready-to-kill.json"));
    assert_eq!(checkpoint["committed_page_index"], 0);
    let store = Store::open(root.join("data/state.sqlite3")).unwrap();
    let committed = store.downloads(ACCOUNT).unwrap().pop().unwrap();
    assert_eq!(committed.completed_pages, 1);
    let committed_receipts = store.page_receipts(ACCOUNT, EPISODE, &committed.revision).unwrap();
    assert_eq!(committed_receipts.len(), 1);
    assert_eq!(committed_receipts[0].page_index, 0);
    drop(store);
    first.0.kill().unwrap();
    let killed = first.0.wait().unwrap();
    assert_eq!(killed.signal(), Some(9));
    run_worker(FLOW_TEST, "recover", &root);
    run_worker(FLOW_TEST, "offline", &root);
    let recovered = read_json(&root.join("recover-result.json"));
    let offline = read_json(&root.join("offline-result.json"));
    assert_ne!(checkpoint["pid"], recovered["pid"]);
    assert_ne!(recovered["pid"], offline["pid"]);
    assert_eq!(recovered["raster_sha256"], offline["raster_sha256"]);
    let store = Store::open(root.join("data/state.sqlite3")).unwrap();
    let record = store.downloads(ACCOUNT).unwrap().pop().unwrap();
    let receipts = store.page_receipts(ACCOUNT, EPISODE, &record.revision).unwrap();
    let damaged = receipts.iter().find(|receipt| receipt.page_index == 0).unwrap();
    let account_digest = format!("{:x}", Sha256::digest(ACCOUNT.as_bytes()));
    fs::write(root.join("data/accounts").join(account_digest).join("pages").join(&damaged.file_name), b"corrupt synthetic object").unwrap();
    drop(store);
    run_worker(FLOW_TEST, "repair", &root);
    assert_eq!(recovered["raster_sha256"], read_json(&root.join("repair-result.json"))["raster_sha256"]);
    // Leave only local synthetic metadata and images for the actual offline App check.
    SessionFile::new(&root.join("data/account")).clear().unwrap();
    write_json(&root.join("result.json"), &json!({"passed":true,"physical_device":false,
        "evidence_kind":"synthetic_source_services_subprocess","real_account":false,
        "crash_signal":9,"first_process_pid":checkpoint["pid"],"recovery_pid":recovered["pid"],"offline_pid":offline["pid"],
        "committed_page_reused":true,"corrupt_receipt_revoked_and_repaired":true,
        "offline_raster_matches":true,"source_paths_persisted":false,"credentials_removed":true,
        "app_fixture_data":"data","expected_comic_id":COMIC,"expected_episode_id":EPISODE,"expected_page_index":1}));
}

fn transaction_worker(phase: &str, root: &Path) {
    let data = root.join("data");
    fs::create_dir_all(&data).unwrap();
    let store = Store::open(data.join("state.sqlite3")).unwrap();
    if phase == "intent-durable" {
        store.set_preference("device", "last_account", &ACCOUNT).unwrap();
        let purchase = json!({"id":"synthetic-purchase","account":ACCOUNT,"state":"submitting",
            "created_at":1800000000,"updated_at":1800000000,"attempts":1,"acknowledgement":null,
            "error_code":null,"entitlement_observed":false,"persistence_pending":false,
            "quote":{"selection":"single","payment":"coin","account":ACCOUNT,"comic_id":COMIC,
                "episode_id":EPISODE,"amount":1.0,"original_amount":1.0,"balance":2.0,
                "request":{"scope":{"kind":"single","episode_id":EPISODE,"comic_id":COMIC},
                    "method":"currency","pay_amount":1.0,"coupon_id":null,"coupon_ids":[],
                    "free_gold_card_id":null,"free_gold_amount":null},"expected_ids":[EPISODE],
                "before_access":[[EPISODE,"locked"]],"raw_offer_index":null,"observed_at":1800000000,
                "fingerprint":"synthetic-quote-fingerprint"}});
        store.set_preference(ACCOUNT, "purchase_journal", &vec![purchase]).unwrap();
        store.set_preference(ACCOUNT, "recharge_intent", &json!({"id":"synthetic-recharge","account":ACCOUNT,
            "amount_cents":100,"fingerprint":"synthetic-recharge-fingerprint","state":"creating",
            "created_at":1800000000,"updated_at":1800000000,"order":null,"reconciliation_id":null,
            "error_code":null,"persistence_pending":false})).unwrap();
        store.checkpoint().unwrap();
        write_json(&root.join("intent-ready.json"), &json!({"pid":std::process::id(),"attempts":1}));
        loop { thread::sleep(Duration::from_secs(1)); }
    }
    drop(store);
    let options = AppOptions { data_dir:data.clone(), profile:DeviceProfile::Paperwhite5, demo:false,
        offline:true,initial_screen:String::new(),landscape:false };
    drop(App::prepare(options).unwrap());
    let store = Store::open(data.join("state.sqlite3")).unwrap();
    let purchase: Vec<Value> = store.preference(ACCOUNT,"purchase_journal").unwrap().unwrap();
    let recharge: Value = store.preference(ACCOUNT,"recharge_intent").unwrap().unwrap();
    assert_eq!(purchase[0]["state"], "unknown");
    assert_eq!(purchase[0]["attempts"], 1);
    assert_eq!(purchase[0]["error_code"], "interrupted_after_dispatch_intent");
    assert!(purchase[0]["acknowledgement"].is_null());
    assert_eq!(recharge["state"], "unknown");
    assert_eq!(recharge["error_code"], "interrupted_order_creation");
    assert!(recharge["order"].is_null());
    write_json(&root.join(format!("{phase}-result.json")), &json!({"passed":true,"pid":std::process::id(),
        "evidence_kind":"production_startup_preparation_with_synthetic_durable_intents",
        "purchase_state":"unknown","purchase_attempts":1,"recharge_state":"unknown",
        "real_purchase_sent":false,"ui_reconciliation_verified":false}));
}

#[test]
fn interrupted_transaction_intents_stay_unknown_after_preparation_restart() {
    if let Some((phase, root)) = worker_context() { transaction_worker(&phase, &root); return; }
    let (_temporary, root) = output_root("transaction-startup");
    let mut initial = spawn_worker(TRANSACTION_TEST,"intent-durable",&root);
    wait_file(&root.join("intent-ready.json"),&mut initial);
    initial.0.kill().unwrap();
    assert_eq!(initial.0.wait().unwrap().signal(),Some(9));
    run_worker(TRANSACTION_TEST,"first-preparation",&root);
    run_worker(TRANSACTION_TEST,"second-preparation",&root);
    let first = read_json(&root.join("first-preparation-result.json"));
    let second = read_json(&root.join("second-preparation-result.json"));
    assert_ne!(first["pid"],second["pid"]);
    write_json(&root.join("result.json"),&json!({"passed":true,"physical_device":false,
        "evidence_kind":"production_startup_preparation_with_synthetic_durable_intents",
        "real_account":false,"crash_signal":9,"restart_count":2,
        "purchase_attempts_unchanged":true,"purchase_state":"unknown","recharge_state":"unknown",
        "ui_reconciliation_verified":false,"real_service_verified":false}));
}
