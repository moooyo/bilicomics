//! Synthetic account callers exercise the actual shared source worker boundary.

use super::*;
use super::crypto::{ImageKey, NativeCrypto, ProtocolCrypto, RequestContext, Signature};
use super::transport::{HttpRequest, HttpResponse, ResponseFuture, Transport};
use serde_json::{Value, json};
use std::{path::Path, sync::{Arc, Condvar, Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}}, time::Duration};
use tokio::sync::Semaphore;

struct SignGate {
    entered: Semaphore,
    released: Mutex<bool>,
    changed: Condvar,
    active: AtomicUsize,
    maximum: AtomicUsize,
    started: AtomicUsize,
}

impl SignGate {
    fn new() -> Arc<Self> {
        Arc::new(Self { entered: Semaphore::new(0), released: Mutex::new(false), changed: Condvar::new(),
            active: AtomicUsize::new(0), maximum: AtomicUsize::new(0), started: AtomicUsize::new(0) })
    }
    fn wait(&self) {
        let count = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.maximum.fetch_max(count, Ordering::SeqCst);
        self.started.fetch_add(1, Ordering::SeqCst);
        self.entered.add_permits(1);
        let mut released = self.released.lock().unwrap_or_else(|error| error.into_inner());
        while !*released { released = self.changed.wait(released).unwrap_or_else(|error| error.into_inner()); }
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
    fn release(&self) {
        *self.released.lock().unwrap_or_else(|error| error.into_inner()) = true;
        self.changed.notify_all();
    }
}

struct ReleaseOnDrop(Arc<SignGate>);
impl Drop for ReleaseOnDrop { fn drop(&mut self) { self.0.release(); } }

struct FixtureCrypto { gate: Option<Arc<SignGate>> }

impl ProtocolCrypto for FixtureCrypto {
    fn sign(&self, _: &RequestContext) -> Result<Signature> {
        if let Some(gate) = &self.gate { gate.wait(); }
        Ok(Signature { ultra_sign: "synthetic-budget-signature".into(), data_sn: "synthetic-budget".into() })
    }
    fn decode_response(&self, _: &RequestContext, _: Value) -> Result<Value> {
        Err(SourceError::protocol("unexpected_synthetic_response_cipher"))
    }
    fn new_image_key(&self) -> Result<ImageKey> { NativeCrypto::new(Path::new("unused-work-fixture-assets")).new_image_key() }
    fn decrypt_image(&self, _: &ImageKey, _: &str, _: &[u8]) -> Result<Vec<u8>> {
        Err(SourceError::protocol("unexpected_synthetic_image_cipher"))
    }
}

struct FixtureTransport {
    posts: AtomicUsize,
    cdn_entered: Semaphore,
    cdn_release: Semaphore,
    cdn_dropped: AtomicBool,
}

impl FixtureTransport {
    fn new() -> Arc<Self> {
        Arc::new(Self { posts: AtomicUsize::new(0), cdn_entered: Semaphore::new(0),
            cdn_release: Semaphore::new(0), cdn_dropped: AtomicBool::new(false) })
    }
}

struct CdnFlight<'a>(&'a AtomicBool);
impl Drop for CdnFlight<'_> { fn drop(&mut self) { self.0.store(true, Ordering::Release); } }

impl Transport for FixtureTransport {
    fn send(&self, request: HttpRequest) -> ResponseFuture<'_> {
        Box::pin(async move {
            let url = url::Url::parse(&request.url).unwrap();
            if url.host_str() == Some("i0.hdslb.com") {
                let _flight = CdnFlight(&self.cdn_dropped);
                self.cdn_entered.add_permits(1);
                self.cdn_release.acquire().await.unwrap().forget();
                return Ok(HttpResponse { status: 200, headers: Vec::new(), body: Vec::new() });
            }
            self.posts.fetch_add(1, Ordering::SeqCst);
            let data = match url.path() {
                "/twirp/comic.v1.Comic/ComicDetail" => json!({"id":12,"ep_list":[]}),
                "/twirp/comic.v1.Comic/ImageToken" => json!([{
                    "complete_url":"https://i0.hdslb.com/synthetic-budget-image?token=synthetic", "hit_encrpyt":false,
                }]),
                _ => return Err(SourceError::protocol("unexpected_synthetic_budget_endpoint")),
            };
            Ok(HttpResponse { status: 200, headers: Vec::new(),
                body: serde_json::to_vec(&json!({"code":0,"data":data})).unwrap() })
        })
    }
}

fn client(account: &str, transport: Arc<FixtureTransport>, gate: Option<Arc<SignGate>>) -> BiliClient {
    let session = AuthSession::from_cookie_text(&format!(
        "SESSDATA=synthetic-budget-session; bili_jct=synthetic-budget-csrf; buvid3=synthetic-budget-device; DedeUserID={account}"))
        .unwrap().with_identity(AccountIdentity { id: account.into(), name: "Synthetic budget account".into(), avatar_url: String::new() }, 1).unwrap();
    BiliClient::with_adapters(Some(session), Path::new("unused-work-fixture-data"), transport,
        Arc::new(FixtureCrypto { gate })).bound_to_account(&format!("bili_{account}")).unwrap()
}

#[tokio::test]
async fn old_and_new_account_clients_share_running_sign_capacity_after_awaiter_cancellation() {
    let _serial = work::process_test_lock().lock().await;
    let gate = SignGate::new(); let _release = ReleaseOnDrop(gate.clone());
    let transport = FixtureTransport::new();
    let mut tasks = Vec::new();
    for account in ["61001", "61002", "61003"] {
        let client = client(account, transport.clone(), Some(gate.clone()));
        tasks.push(tokio::spawn(async move { client.comic_detail("12").await }));
    }
    tokio::time::timeout(Duration::from_secs(8), gate.entered.acquire_many(work::COMPUTE_SLOTS as u32))
        .await.unwrap().unwrap().forget();
    for task in &tasks { task.abort(); }
    for task in tasks { assert!(task.await.unwrap_err().is_cancelled()); }
    assert_eq!(gate.started.load(Ordering::SeqCst), work::COMPUTE_SLOTS);
    assert!(gate.maximum.load(Ordering::SeqCst) <= work::COMPUTE_SLOTS);
    assert_eq!(transport.posts.load(Ordering::SeqCst), 0);
    gate.release();
    tokio::time::timeout(Duration::from_secs(8), async {
        while gate.active.load(Ordering::SeqCst) != 0 { tokio::task::yield_now().await; }
    }).await.unwrap();
}

#[tokio::test]
async fn cancelling_a_cdn_wait_drops_the_network_future_without_waiting_for_its_timeout() {
    let transport = FixtureTransport::new();
    let client = client("61004", transport.clone(), None);
    let batch = client.prepare_image_batch(&["/synthetic-budget-image".into()]).await.unwrap();
    let downloading = tokio::spawn(async move { client.download_image(&batch, 0).await });
    tokio::time::timeout(Duration::from_secs(8), transport.cdn_entered.acquire()).await.unwrap().unwrap().forget();
    downloading.abort();
    assert!(downloading.await.unwrap_err().is_cancelled());
    assert!(transport.cdn_dropped.load(Ordering::Acquire));
}
