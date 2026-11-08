//! Opt-in anonymous service compatibility check with a closed endpoint list.
//!
//! This check reads public category metadata only. It never authenticates,
//! downloads comic pages, or submits account, history, or transaction changes.

#![cfg(target_os = "linux")]

use bilicomics::source::{BiliClient, ErrorKind, Result, SourceError};
use bilicomics::source::crypto::NativeCrypto;
use bilicomics::source::transport::{HttpRequest, ReqwestTransport, ResponseFuture, Transport};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{io::Write, path::PathBuf, sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}}, time::Duration};
use url::Url;

const ASSET_FILE: &str = "efae82c96a7eef44bee5.wasm";
const ASSET_SHA256: &str = "39bc0676953752c461197df592e1f5894f1a7492a29400c946e560fc109a8e2e";

struct PublicCatalogTransport {
    inner: ReqwestTransport,
    requests: AtomicUsize,
    events: Mutex<Vec<Value>>,
}

impl Transport for PublicCatalogTransport {
    fn send(&self, request: HttpRequest) -> ResponseFuture<'_> {
        Box::pin(async move {
            let url = Url::parse(&request.url)
                .map_err(|_| SourceError::invalid("public_check_invalid_url"))?;
            let endpoint = match (request.method.as_str(), url.path()) {
                ("POST", "/twirp/comic.v1.Comic/AllLabel") => "category_metadata",
                ("GET", "/ductape/buvid") => "guest_device_initialization",
                ("POST", "/twirp/comic.v1.Comic/ClassPage") => "signed_category_page",
                _ => return Err(SourceError::invalid("public_check_endpoint_blocked")),
            };
            if url.scheme() != "https" || url.host_str() != Some("manga.bilibili.com")
                || !url.username().is_empty() || url.password().is_some()
                || url.port_or_known_default() != Some(443)
                || self.requests.fetch_add(1, Ordering::SeqCst) >= 3
            {
                return Err(SourceError::invalid("public_check_request_blocked"));
            }
            for (name, value) in &request.headers {
                if name.eq_ignore_ascii_case("authorization")
                    || name.eq_ignore_ascii_case("x-xsrf-token")
                {
                    return Err(SourceError::invalid("public_check_credentials_blocked"));
                }
                if name.eq_ignore_ascii_case("cookie") {
                    let guest = value.strip_prefix("buvid3=").unwrap_or_default();
                    if endpoint != "signed_category_page" || guest.is_empty() || guest.len() > 256
                        || !guest.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                    {
                        return Err(SourceError::invalid("public_check_credentials_blocked"));
                    }
                }
            }
            let signed = url.query_pairs().any(|(name, value)| name == "ultra_sign" && !value.is_empty());
            if endpoint == "signed_category_page" && (!signed || !request.headers.iter().any(|(name, value)|
                name.eq_ignore_ascii_case("x-bili-data-sn") && value == "1E74C20E5720FBF3BB351965D7A9DFC1"))
            {
                return Err(SourceError::invalid("public_check_signature_missing"));
            }
            let request_bytes = request.body.len();
            let response = match tokio::time::timeout(Duration::from_secs(30), self.inner.send(request)).await {
                Ok(result) => result,
                Err(_) => Err(SourceError::new(ErrorKind::Network, "public_check_timeout")),
            };
            let event = match &response {
                Ok(response) => {
                    let envelope: Option<Value> = serde_json::from_slice(&response.body).ok();
                    json!({"endpoint":endpoint, "signed":signed, "request_bytes":request_bytes,
                        "http_status":response.status, "response_bytes":response.body.len(),
                        "business_code":envelope.as_ref().and_then(|value| value.get("code")).and_then(Value::as_i64),
                        "encrypted_envelope":envelope.as_ref().is_some_and(|value| value.get("bytesData").is_some())})
                }
                Err(error) => json!({"endpoint":endpoint, "signed":signed,
                    "request_bytes":request_bytes, "error_code":error.code}),
            };
            self.events.lock().expect("Record safe public-check metadata").push(event);
            response
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Contacts only public category endpoints; explicitly provision the pinned asset and output path"]
async fn anonymous_public_categories_use_the_real_signer() {
    let asset_dir = PathBuf::from(std::env::var_os("BILICOMICS_SIGNING_ASSET_DIR")
        .expect("Set BILICOMICS_SIGNING_ASSET_DIR to the explicitly acquired public asset"));
    let output = PathBuf::from(std::env::var_os("BILICOMICS_PUBLIC_CATALOG_OUTPUT")
        .expect("Set BILICOMICS_PUBLIC_CATALOG_OUTPUT to a new report file"));
    let asset = std::fs::read(asset_dir.join(ASSET_FILE)).expect("Read the pinned public signing asset");
    assert_eq!(format!("{:x}", Sha256::digest(&asset)), ASSET_SHA256);
    let transport = Arc::new(PublicCatalogTransport {
        inner: ReqwestTransport::new().expect("Create the production HTTPS transport"),
        requests: AtomicUsize::new(0), events: Mutex::new(Vec::new()),
    });
    let data = tempfile::tempdir().expect("Create an empty account-free test directory");
    let client = BiliClient::with_adapters(None, data.path(), transport.clone(), Arc::new(NativeCrypto::new(&asset_dir)));
    let mut counts = json!({});
    let outcome: Result<()> = async {
        let metadata = client.categories().await?;
        counts["styles"] = json!(metadata.styles.len());
        counts["orders"] = json!(metadata.orders.len());
        let category = metadata.styles.first().ok_or_else(|| SourceError::protocol("public_check_categories_empty"))?;
        let page = client.category_page(&category.id, 0, 1).await?;
        if page.page != 1 || page.comics.is_empty() {
            return Err(SourceError::protocol("public_check_page_empty"));
        }
        counts["comics"] = json!(page.comics.len());
        counts["page"] = json!(page.page);
        counts["has_more"] = json!(page.has_more);
        Ok(())
    }.await;
    let events = transport.events.lock().expect("Read safe public-check metadata").clone();
    let session_absent = client.session().is_none();
    let report = json!({
        "schema_version":1,
        "passed":outcome.is_ok() && session_absent && events.len() == 3,
        "mode":"anonymous_public_metadata_with_endpoint_allowlist",
        "signing_asset_sha256":ASSET_SHA256,
        "real_signer":true, "real_https_transport":true,
        "session_absent":session_absent, "live_account_verified":false,
        "comic_pages_acquired":0, "account_mutations":0, "physical_device":false,
        "counts":counts, "requests":events,
        "error":outcome.as_ref().err().map(|error| json!({"code":error.code,
            "http_status":error.status, "business_code":error.business_code,
            "kind":format!("{:?}",error.kind)})),
    });
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&output)
        .expect("Create a new public-check evidence report without replacing prior results");
    serde_json::to_writer_pretty(&mut file, &report).expect("Write safe public-check metadata");
    file.write_all(b"\n").expect("Complete the report");
    file.sync_all().expect("Persist the report");
    assert!(report["passed"].as_bool().unwrap_or(false), "Public compatibility check failed: {:?}", outcome.err());
}
