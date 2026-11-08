//! Opt-in integration checks for the pinned official signing module.
//!
//! Run through scripts/verification/native_protocol.py on the designated remote host.
//! Every request uses synthetic data and a transport that cannot contact a service.

#![cfg(target_os = "linux")]

use bilicomics::source::{AuthSession, BiliClient, ErrorKind, Result};
use bilicomics::source::crypto::{ImageKey, NativeCrypto, ProtocolCrypto, RequestContext, Signature};
use bilicomics::source::transport::{HttpRequest, HttpResponse, ResponseFuture, Transport};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::{Arc, Mutex}};

const ASSET_FILE: &str = "efae82c96a7eef44bee5.wasm";
const ASSET_SHA256: &str = "39bc0676953752c461197df592e1f5894f1a7492a29400c946e560fc109a8e2e";
const SIGN_QUERY: &str = "device=pc&platform=web&nov=27&eot=812";
const DATA_SN: &str = "1E74C20E5720FBF3BB351965D7A9DFC1";

fn asset_root() -> PathBuf {
    let root = PathBuf::from(std::env::var_os("BILICOMICS_SIGNING_ASSET_DIR")
        .expect("Set BILICOMICS_SIGNING_ASSET_DIR to the explicitly acquired pinned asset"));
    let bytes = std::fs::read(root.join(ASSET_FILE)).expect("Read the pinned signing asset");
    assert_eq!(format!("{:x}", Sha256::digest(&bytes)), ASSET_SHA256,
        "The official signing module must match the reviewed digest");
    root
}

fn context() -> RequestContext {
    RequestContext {
        endpoint: "ComicDetail".into(),
        path: "/twirp/comic.v1.Comic/ComicDetail".into(),
        url: String::new(),
        body: "{\"comic_id\":12,\"m2\":\"error:synthetic-native-verification\"}".into(),
        sign_query: SIGN_QUERY.into(),
        buvid: "synthetic-device".into(),
        platform: "web".into(),
        timestamp_ms: 1_793_664_000_123,
    }
}

fn digest(text: &str) -> String { format!("{:x}", Sha256::digest(text.as_bytes())) }

fn record(case: &str, detail: Value) {
    println!("NATIVE_PROTOCOL_CASE {}", json!({"case":case, "passed":true, "detail":detail,
        "live_account":false, "service_requests":0, "physical_device":false}));
}

#[test]
#[ignore = "Requires the pinned public signing asset; use scripts/verification/native_protocol.py"]
fn official_wasm_callback_handles_exact_synthetic_inputs() {
    let crypto = NativeCrypto::new(&asset_root());
    let original = context();
    let first = crypto.sign(&original).expect("Execute the real official WASM callback");
    assert!(!first.ultra_sign.is_empty());
    assert_eq!(first.data_sn, DATA_SN);
    let second = crypto.sign(&original).expect("Execute a fresh runtime with the same input");
    let mut body_changed = original.clone();
    body_changed.body = "{\"comic_id\":13,\"m2\":\"error:synthetic-native-verification\"}".into();
    let changed_body = crypto.sign(&body_changed).expect("Sign a changed body");
    let mut query_changed = original.clone();
    query_changed.sign_query = "device=pc&platform=web&nov=27&eot=813".into();
    let changed_query = crypto.sign(&query_changed).expect("Sign a changed query");
    let mut time_changed = original.clone();
    time_changed.timestamp_ms += 1;
    let changed_time = crypto.sign(&time_changed).expect("Sign a changed timestamp");
    let mut whitespace_changed = original.clone();
    whitespace_changed.body = "{ \"comic_id\" : 12, \"m2\" : \"error:synthetic-native-verification\" }".into();
    let changed_whitespace = crypto.sign(&whitespace_changed).expect("Sign the original whitespace bytes");
    let mut unicode_changed = original.clone();
    unicode_changed.body = "{\"comic_id\":12,\"fixture\":\"caf\u{00e9} \u{03a9}\"}".into();
    let changed_unicode = crypto.sign(&unicode_changed).expect("Sign the original UTF-8 bytes");
    record("official_callback_inputs", json!({
        "signature_bytes":first.ultra_sign.len(), "signature_sha256":digest(&first.ultra_sign),
        "same_input_sha256":digest(&second.ultra_sign),
        "changed_body_sha256":digest(&changed_body.ultra_sign),
        "changed_query_sha256":digest(&changed_query.ultra_sign),
        "changed_timestamp_sha256":digest(&changed_time.ultra_sign),
        "body_sha256":digest(&original.body), "query_sha256":digest(&original.sign_query),
        "timestamp_ms":original.timestamp_ms,
        "reference_cases":[
            {"name":"baseline","query":original.sign_query,"body":original.body,
                "timestamp_ms":original.timestamp_ms,"signature_sha256":digest(&first.ultra_sign)},
            {"name":"same_input","query":original.sign_query,"body":original.body,
                "timestamp_ms":original.timestamp_ms,"signature_sha256":digest(&second.ultra_sign)},
            {"name":"changed_body","query":body_changed.sign_query,"body":body_changed.body,
                "timestamp_ms":body_changed.timestamp_ms,"signature_sha256":digest(&changed_body.ultra_sign)},
            {"name":"changed_query","query":query_changed.sign_query,"body":query_changed.body,
                "timestamp_ms":query_changed.timestamp_ms,"signature_sha256":digest(&changed_query.ultra_sign)},
            {"name":"changed_timestamp","query":time_changed.sign_query,"body":time_changed.body,
                "timestamp_ms":time_changed.timestamp_ms,"signature_sha256":digest(&changed_time.ultra_sign)},
            {"name":"changed_whitespace","query":whitespace_changed.sign_query,"body":whitespace_changed.body,
                "timestamp_ms":whitespace_changed.timestamp_ms,"signature_sha256":digest(&changed_whitespace.ultra_sign)},
            {"name":"unicode_body","query":unicode_changed.sign_query,"body":unicode_changed.body,
                "timestamp_ms":unicode_changed.timestamp_ms,"signature_sha256":digest(&changed_unicode.ultra_sign)}
        ]
    }));
}

#[test]
#[ignore = "Requires the pinned public signing asset; use scripts/verification/native_protocol.py"]
fn concurrent_callers_keep_native_runtime_state_isolated() {
    let crypto = Arc::new(NativeCrypto::new(&asset_root()));
    let expected = crypto.sign(&context()).expect("Establish the synthetic baseline").ultra_sign;
    let workers: Vec<_> = (0..4).map(|_| {
        let crypto = crypto.clone();
        std::thread::spawn(move || crypto.sign(&context()).expect("Sign through the serialized native boundary").ultra_sign)
    }).collect();
    for worker in workers {
        assert_eq!(worker.join().expect("Join a native signing worker"), expected);
    }
    record("concurrent_runtime_isolation", json!({"concurrent_callers":4,"same_input_matches":true}));
}

#[test]
#[ignore = "Requires the pinned public signing asset; use scripts/verification/native_protocol.py"]
fn missing_corrupt_and_truncated_modules_fail_without_poisoning_the_signer() {
    let original = context();
    let temporary = tempfile::tempdir().expect("Create a temporary asset directory");
    let crypto = NativeCrypto::new(temporary.path());
    let missing = crypto.sign(&original).err().expect("A missing module must fail");
    assert_eq!(missing.kind, ErrorKind::Capability);
    assert_eq!(missing.code, "signing_asset_missing");
    assert_eq!(missing.transmitted, Some(false));
    std::fs::write(temporary.path().join(ASSET_FILE), b"not a WebAssembly module")
        .expect("Write the deliberately corrupt asset");
    let corrupt = crypto.sign(&original).err().expect("A corrupt module must fail");
    assert_eq!(corrupt.kind, ErrorKind::Capability);
    assert_eq!(corrupt.code, "signing_asset_integrity_failed");
    assert_eq!(corrupt.transmitted, Some(false));
    std::fs::write(temporary.path().join(ASSET_FILE), b"\0asm\x01\0\0\0")
        .expect("Write the deliberately incomplete module");
    let truncated = crypto.sign(&original).err().expect("An incomplete module must fail");
    assert_eq!(truncated.kind, ErrorKind::Capability);
    assert_eq!(truncated.code, "signing_asset_integrity_failed");
    assert_eq!(truncated.transmitted, Some(false));
    std::fs::copy(asset_root().join(ASSET_FILE), temporary.path().join(ASSET_FILE))
        .expect("Restore the verified module");
    assert!(!crypto.sign(&original).expect("The next fresh runtime must recover").ultra_sign.is_empty());
    record("module_failures_and_recovery", json!({"missing":missing.code,"corrupt":corrupt.code,
        "incomplete":truncated.code,"recovered":true}));
}

#[test]
#[ignore = "Requires the pinned public signing asset; use scripts/verification/native_protocol.py"]
fn signing_validation_preserves_the_exact_input_contract() {
    let crypto = NativeCrypto::new(&asset_root());
    let original = context();
    for (name, value) in [("empty_query", String::new()), ("nul_query", "device=pc\0&platform=web".into()),
        ("oversized_query", "q".repeat(8193))] {
        let mut invalid = original.clone();
        invalid.sign_query = value;
        let error = crypto.sign(&invalid).err().expect(name);
        assert_eq!((error.kind, error.code.as_str()), (ErrorKind::InvalidRequest, "invalid_signing_context"));
    }
    for body in ["not-json", "[]", "null", "{\"bad\":\"actual\0nul\"}"] {
        let mut invalid = original.clone();
        invalid.body = body.into();
        assert_eq!(crypto.sign(&invalid).err().expect("Reject an invalid body").code, "invalid_signing_body");
    }
    for timestamp in [999_999_999_999, 10_000_000_000_000] {
        let mut invalid = original.clone();
        invalid.timestamp_ms = timestamp;
        assert_eq!(crypto.sign(&invalid).err().expect("Reject an invalid timestamp").code, "invalid_signing_context");
    }
    let mut escaped = original;
    escaped.body = "{\"comic_id\":12,\"fixture\":\"\\u0000\"}".into();
    assert!(!crypto.sign(&escaped).expect("JSON-escaped NUL remains literal serialized bytes").ultra_sign.is_empty());
    record("input_validation", json!({"invalid_contexts":5,"invalid_bodies":4,"escaped_json_nul":true}));
}

struct SignedCall { context: RequestContext, signature: String }
struct AuditedCrypto { native: NativeCrypto, signed: Arc<Mutex<Vec<SignedCall>>> }

impl ProtocolCrypto for AuditedCrypto {
    fn sign(&self, context: &RequestContext) -> Result<Signature> {
        let result = self.native.sign(context)?;
        self.signed.lock().expect("Signing audit lock").push(SignedCall {
            context:context.clone(), signature:result.ultra_sign.clone()
        });
        Ok(result)
    }
    fn decode_response(&self, context: &RequestContext, envelope: Value) -> Result<Value> {
        self.native.decode_response(context, envelope)
    }
    fn new_image_key(&self) -> Result<ImageKey> { self.native.new_image_key() }
    fn decrypt_image(&self, key: &ImageKey, url: &str, bytes: &[u8]) -> Result<Vec<u8>> {
        self.native.decrypt_image(key, url, bytes)
    }
}

struct WireAudit { signed: Arc<Mutex<Vec<SignedCall>>>, requests: Mutex<Vec<String>> }

impl Transport for WireAudit {
    fn send(&self, request: HttpRequest) -> ResponseFuture<'_> {
        Box::pin(async move {
            let url = url::Url::parse(&request.url).expect("Constructed request URL");
            let endpoint = url.path().rsplit('/').next().expect("Endpoint");
            assert_eq!(request.method, "POST");
            assert_eq!(url.host_str(), Some("manga.bilibili.com"));
            assert!(matches!(endpoint, "ComicDetail" | "GetImageIndex" | "ImageToken"),
                "A controlled transport rejects every unplanned operation");
            let signed = self.signed.lock().expect("Signing audit lock");
            let call = signed.last().expect("The native callback must finish before transmission");
            assert_eq!(call.context.endpoint, endpoint);
            assert_eq!(call.context.sign_query, SIGN_QUERY);
            assert_eq!(call.context.body.as_bytes(), request.body.as_slice());
            assert_eq!(url.query_pairs().find(|(name, _)|name == "a").map(|(_,value)|value.into_owned()), Some("810".into()));
            assert_eq!(url.query_pairs().find(|(name, _)|name == "ultra_sign").map(|(_,value)|value.into_owned()),
                Some(call.signature.clone()));
            assert_eq!(request.headers.iter().find(|(name,_)|name == "x-bili-data-sn").map(|(_,value)|value.as_str()),
                Some(DATA_SN));
            let body: Value = serde_json::from_slice(&request.body).expect("Wire JSON");
            let data = match endpoint {
                "ComicDetail" => { assert_eq!(body["comic_id"],12); json!({"id":12,"title":"Synthetic Native Comic","ep_list":[]}) },
                "GetImageIndex" => { assert_eq!(body["ep_id"],21); json!({"images":[{"path":"/synthetic/page","x":2,"y":3}]}) },
                "ImageToken" => { assert_eq!(body["urls"],"[\"/synthetic/page\"]");
                    assert_eq!(body["m1"].as_str().expect("P-256 public key").len(),88);
                    json!([{"complete_url":"https://i0.hdslb.com/synthetic.png?token=synthetic"}]) },
                _ => unreachable!(),
            };
            self.requests.lock().expect("Transport audit lock").push(endpoint.into());
            Ok(HttpResponse { status:200, headers:Vec::new(),
                body:serde_json::to_vec(&json!({"code":0,"data":data})).expect("Synthetic response") })
        })
    }
}

#[tokio::test]
#[ignore = "Requires the pinned public signing asset; use scripts/verification/native_protocol.py"]
async fn production_client_transmits_the_exact_bytes_signed_by_the_official_callback() {
    let signed = Arc::new(Mutex::new(Vec::new()));
    let transport = Arc::new(WireAudit { signed:signed.clone(), requests:Mutex::new(Vec::new()) });
    let crypto = Arc::new(AuditedCrypto { native:NativeCrypto::new(&asset_root()), signed:signed.clone() });
    let temporary = tempfile::tempdir().expect("Create isolated source data");
    let session = AuthSession::from_cookie_text("SESSDATA=synthetic-session; bili_jct=synthetic-csrf; buvid3=synthetic-device")
        .expect("Synthetic credentials");
    let client = BiliClient::with_adapters(Some(session), temporary.path(), transport.clone(), crypto);
    assert_eq!(client.comic_detail("12").await.expect("Protected detail").comic.id,"12");
    assert_eq!(client.image_index("21").await.expect("Protected index").images.len(),1);
    client.prepare_image_batch(&["/synthetic/page".into()]).await.expect("Protected tokens");
    assert_eq!(*transport.requests.lock().expect("Transport audit lock"), ["ComicDetail","GetImageIndex","ImageToken"]);
    assert_eq!(signed.lock().expect("Signing audit lock").len(),3);
    record("production_wire_contract", json!({"protected_requests":3,"exact_body_bytes":true,
        "signing_query":SIGN_QUERY,"wire_a":"810","native_callback":true,
        "controlled_transport":true,"asset_acquisition":"prevalidated_external_asset"}));
}
