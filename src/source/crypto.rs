//! Pinned native signing and bounded Bilibili response/image transformations.

use std::{collections::BTreeMap, fmt, path::{Path, PathBuf}};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::Value;

use super::{ErrorKind, Result, SourceError};

const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const SIGNING_ASSET: &str = "efae82c96a7eef44bee5.wasm";
const DATA_SN: &str = "1E74C20E5720FBF3BB351965D7A9DFC1";

/// The exact serialized request used by the signing and response contracts.
#[derive(Clone)]
pub struct RequestContext {
    pub endpoint: String,
    pub path: String,
    pub url: String,
    pub body: String,
    pub sign_query: String,
    pub buvid: String,
    pub platform: String,
    pub timestamp_ms: i64,
}

pub struct Signature {
    pub ultra_sign: String,
    pub data_sn: String,
}

/// A transient P-256 key pair. Private material is never serialized or logged.
pub struct ImageKey {
    private: Sensitive<32>,
    public: [u8; 65],
}

impl ImageKey {
    pub fn m1(&self) -> String {
        STANDARD.encode(self.public)
    }
}

impl fmt::Debug for ImageKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ImageKey { private: [REDACTED] }")
    }
}

pub trait ProtocolCrypto: Send + Sync {
    fn sign(&self, context: &RequestContext) -> Result<Signature>;
    fn decode_response(&self, context: &RequestContext, envelope: Value) -> Result<Value>;
    fn new_image_key(&self) -> Result<ImageKey>;
    fn decrypt_image(&self, key: &ImageKey, url: &str, bytes: &[u8]) -> Result<Vec<u8>>;

    fn signing_asset_ready(&self) -> Result<bool> { Ok(false) }
    fn signing_asset_ready_cancellable(&self, is_cancelled: &dyn Fn() -> bool) -> Result<bool> {
        check_cancelled(is_cancelled)?;
        self.signing_asset_ready()
    }
    fn sign_cancellable(&self, context: &RequestContext, is_cancelled: &dyn Fn() -> bool) -> Result<Signature> {
        check_cancelled(is_cancelled)?;
        self.sign(context)
    }
    fn decode_response_cancellable(&self, context: &RequestContext, envelope: Value,
        is_cancelled: &dyn Fn() -> bool) -> Result<Value>
    {
        check_cancelled(is_cancelled)?;
        self.decode_response(context, envelope)
    }
    fn decrypt_image_cancellable(&self, key: &ImageKey, url: &str, bytes: &[u8],
        is_cancelled: &dyn Fn() -> bool) -> Result<Vec<u8>>
    {
        check_cancelled(is_cancelled)?;
        self.decrypt_image(key, url, bytes)
    }
}

/// Construction records the asset directory without loading native code or assets.
pub struct NativeCrypto {
    asset_root: PathBuf,
    signing_asset: super::assets::SigningAssetCache,
}

impl NativeCrypto {
    pub fn new(asset_root: &Path) -> Self {
        Self { asset_root: asset_root.to_owned(), signing_asset: super::assets::SigningAssetCache::new(super::client::SIGNING_SHA256) }
    }
}

impl ProtocolCrypto for NativeCrypto {
    fn sign(&self, context: &RequestContext) -> Result<Signature> {
        self.sign_cancellable(context, &|| false)
    }

    fn signing_asset_ready(&self) -> Result<bool> {
        Ok(self.signing_asset.load_if_present(&self.asset_root.join(SIGNING_ASSET))?.is_some())
    }

    fn signing_asset_ready_cancellable(&self, is_cancelled: &dyn Fn() -> bool) -> Result<bool> {
        Ok(self.signing_asset.load_if_present_cancellable(&self.asset_root.join(SIGNING_ASSET), is_cancelled)?.is_some())
    }

    fn sign_cancellable(&self, context: &RequestContext, is_cancelled: &dyn Fn() -> bool) -> Result<Signature> {
        check_cancelled(is_cancelled)?;
        if !(1_000_000_000_000..=9_999_999_999_999).contains(&context.timestamp_ms)
            || context.sign_query.is_empty()
            || context.sign_query.len() > 8192
            || context.sign_query.as_bytes().contains(&0)
            || context.body.len() > MAX_RESPONSE_BYTES
        {
            return Err(SourceError::invalid("invalid_signing_context"));
        }
        let body: Value = serde_json::from_str(&context.body)
            .map_err(|_| SourceError::invalid("invalid_signing_body"))?;
        if !body.is_object() {
            return Err(SourceError::invalid("invalid_signing_body"));
        }
        drop(body);
        check_cancelled(is_cancelled)?;
        let request = serde_json::json!({
            "function": "y1_z2w2a3",
            "args": [&context.sign_query, &context.body, context.timestamp_ms],
        });
        // Managed acquisition and direct native calls share the same verified
        // immutable snapshot; C never reopens a path after pin validation.
        let result = native::invoke(&self.signing_asset, &self.asset_root.join(SIGNING_ASSET), &request, is_cancelled)?;
        let sign = result.get("sign").and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 4096)
            .ok_or_else(|| SourceError::protocol("invalid_signature_result"))?;
        Ok(Signature { ultra_sign: sign.to_owned(), data_sn: DATA_SN.to_owned() })
    }

    fn decode_response(&self, context: &RequestContext, envelope: Value) -> Result<Value> {
        self.decode_response_cancellable(context, envelope, &|| false)
    }

    fn decode_response_cancellable(&self, context: &RequestContext, mut envelope: Value,
        is_cancelled: &dyn Fn() -> bool) -> Result<Value>
    {
        check_cancelled(is_cancelled)?;
        let object = envelope.as_object()
            .ok_or_else(|| SourceError::protocol("invalid_response_envelope"))?;
        if object.get("code").and_then(Value::as_i64) != Some(0) {
            return Ok(envelope);
        }
        let Some(encoded) = object.get("bytesData").filter(|value| !value.is_null()) else {
            if !object.get("data").is_some_and(json_container) {
                return Err(SourceError::protocol("missing_response_data"));
            }
            return Ok(envelope);
        };
        let encoded = encoded.as_str()
            .ok_or_else(|| SourceError::protocol("invalid_response_ciphertext"))?;
        if encoded.len() > MAX_RESPONSE_BYTES * 4 / 3 + 4 {
            return Err(SourceError::protocol("response_size_limit"));
        }
        check_cancelled(is_cancelled)?;
        let ciphertext = decode_base64(encoded.as_bytes(), "invalid_response_base64")?;
        if ciphertext.is_empty() || ciphertext.len() > MAX_RESPONSE_BYTES || ciphertext.len() % 16 != 0 {
            return Err(SourceError::protocol("invalid_response_ciphertext"));
        }
        let key = response_key(context)?;
        check_cancelled(is_cancelled)?;
        let mut decoded = SensitiveBytes(native::aes192_decrypt(key.bytes(), &ciphertext)?);
        let iv: [u8; 16] = key.bytes()[..16].try_into()
            .map_err(|_| SourceError::protocol("invalid_response_key"))?;
        unchain_cbc(&mut decoded.0, &ciphertext, &iv)?;
        let length = unpadded_length(&decoded.0)?;
        check_cancelled(is_cancelled)?;
        let data: Value = serde_json::from_slice(&decoded.0[..length])
            .map_err(|_| SourceError::protocol("invalid_decrypted_response"))?;
        if !json_container(&data) {
            return Err(SourceError::protocol("invalid_decrypted_response"));
        }
        let object = envelope.as_object_mut()
            .ok_or_else(|| SourceError::protocol("invalid_response_envelope"))?;
        object.remove("bytesData");
        object.insert("data".to_owned(), data);
        Ok(envelope)
    }

    fn new_image_key(&self) -> Result<ImageKey> {
        native::new_key()
    }

    fn decrypt_image(&self, key: &ImageKey, url: &str, bytes: &[u8]) -> Result<Vec<u8>> {
        self.decrypt_image_cancellable(key, url, bytes, &|| false)
    }

    fn decrypt_image_cancellable(&self, key: &ImageKey, url: &str, bytes: &[u8],
        is_cancelled: &dyn Fn() -> bool) -> Result<Vec<u8>>
    {
        check_cancelled(is_cancelled)?;
        if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
            return Err(SourceError::protocol("image_container_size_limit"));
        }
        let version = bytes[0];
        if !matches!(version, 3 | 5 | 6 | 7 | 8) {
            return Err(SourceError::capability("unsupported_image_version"));
        }
        let parameters = image_parameters(url, version)?;
        check_cancelled(is_cancelled)?;
        let container = image_container(bytes, parameters.timestamp, version)?;
        let secret = native::derive(key.private.bytes(), &container.public)?;
        check_cancelled(is_cancelled)?;
        match version {
            5 => decrypt_gcm(secret.bytes(), &parameters, &container.payload, is_cancelled),
            6 => decrypt_cbc(secret.bytes(), &parameters.iv, &container.payload, is_cancelled),
            _ => decrypt_ctr(secret.bytes(), &parameters.iv, &container.payload, version == 8, is_cancelled),
        }
    }
}

fn check_cancelled(is_cancelled: &dyn Fn() -> bool) -> Result<()> {
    if is_cancelled() { Err(SourceError::new(ErrorKind::Cancelled, "source_work_cancelled")) }
    else { Ok(()) }
}

fn json_container(value: &Value) -> bool {
    value.is_object() || value.is_array()
}

fn response_key(context: &RequestContext) -> Result<Sensitive<24>> {
    if context.platform != "web" {
        return Err(SourceError::capability("unsupported_response_platform"));
    }
    if !context.buvid.bytes().all(|byte| (0x20..=0x7e).contains(&byte)) {
        return Err(SourceError::protocol("invalid_response_buvid"));
    }
    let candidate = if !context.path.is_empty() { &context.path }
        else if !context.url.is_empty() { &context.url } else { &context.endpoint };
    let path = if candidate.starts_with("http://") || candidate.starts_with("https://") {
        url::Url::parse(candidate).map_err(|_| SourceError::protocol("invalid_response_path"))?
            .path().to_owned()
    } else {
        candidate.split(['?', '#']).next().unwrap_or_default().to_owned()
    };
    let path = if let Some(path) = path.strip_prefix("/twirp/") { format!("/{path}") }
        else if !path.starts_with('/') && !path.contains('/') { format!("/comic.v1.Comic/{path}") }
        else if !path.starts_with('/') { format!("/{path}") } else { path };
    let field = match path.as_str() {
        "/comic.v1.Comic/ComicDetail" => "comic_id",
        "/comic.v1.Comic/GetImageIndex" => "ep_id",
        "/comic.v1.Comic/ClassPage" => "style_id",
        "/comic.v1.Comic/ImageToken" => "urls",
        _ => return Err(SourceError::capability("unsupported_encrypted_response")),
    };
    let body: Value = serde_json::from_str(&context.body)
        .map_err(|_| SourceError::protocol("invalid_response_context_body"))?;
    if !body.is_object() {
        return Err(SourceError::protocol("invalid_response_context_body"));
    }
    let selected = match body.get(field) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(value)) if value.bytes().all(|byte| (0x20..=0x7e).contains(&byte)) => value.clone(),
        Some(Value::Number(value)) => {
            let value = value.as_f64().filter(|value| value.is_finite() && value.fract() == 0.0
                && value.abs() < 9_007_199_254_740_992.0)
                .ok_or_else(|| SourceError::protocol("invalid_response_key_field"))?;
            (value as i64).to_string()
        }
        _ => return Err(SourceError::protocol("invalid_response_key_field")),
    };
    let mut value: Vec<u8> = context.buvid.bytes().take(36).step_by(2).collect();
    value.extend_from_slice(b"web");
    value.extend_from_slice(&selected.as_bytes()[selected.len().saturating_sub(3)..]);
    if value.len() > 24 {
        return Err(SourceError::protocol("invalid_response_key"));
    }
    let mut key = Sensitive::zeroed();
    key.0.fill(b'=');
    key.0[..value.len()].copy_from_slice(&value);
    wipe(&mut value);
    Ok(key)
}

struct ImageParameters {
    iv: [u8; 16],
    salt: Option<[u8; 16]>,
    timestamp: Option<u32>,
}

fn image_parameters(input: &str, version: u8) -> Result<ImageParameters> {
    if input.len() > 16384 || input.bytes().any(|byte| matches!(byte, 0 | b'\r' | b'\n')) {
        return Err(SourceError::protocol("invalid_image_url"));
    }
    let parsed = url::Url::parse(input).map_err(|_| SourceError::protocol("invalid_image_url"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(SourceError::protocol("invalid_image_url"));
    }
    let query = input.split('#').next().unwrap_or_default().split_once('?')
        .map(|(_, query)| query).ok_or_else(|| SourceError::protocol("missing_image_parameters"))?;
    let mut parameters = BTreeMap::<Vec<u8>, Vec<u8>>::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        if pair.contains(';') {
            return Err(SourceError::protocol("invalid_image_parameters"));
        }
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let name = percent_decode(name.as_bytes(), true)?;
        let value = percent_decode(value.as_bytes(), true)?;
        parameters.entry(name).or_insert(value);
    }
    let timestamp = if version == 8 {
        let timestamp = parameters.get(b"ts".as_slice())
            .ok_or_else(|| SourceError::protocol("invalid_image_timestamp"))?;
        if timestamp.is_empty() || timestamp.len() > 16 || !timestamp.iter().all(u8::is_ascii_hexdigit) {
            return Err(SourceError::protocol("invalid_image_timestamp"));
        }
        let text = std::str::from_utf8(timestamp)
            .map_err(|_| SourceError::protocol("invalid_image_timestamp"))?;
        let timestamp = u64::from_str_radix(text, 16)
            .map_err(|_| SourceError::protocol("invalid_image_timestamp"))?;
        if timestamp > i64::MAX as u64 {
            return Err(SourceError::protocol("invalid_image_timestamp"));
        }
        Some(timestamp as u32)
    } else { None };
    let encoded = parameters.get(b"cpx".as_slice())
        .ok_or_else(|| SourceError::protocol("missing_image_parameters"))?;
    let encoded = if version == 8 { encoded.clone() } else { percent_decode(encoded, false)? };
    if encoded.is_empty() || encoded.len() > 4096 || encoded.len() % 4 != 0 {
        return Err(SourceError::protocol("invalid_image_parameters"));
    }
    let decoded = decode_base64(&encoded, "invalid_image_parameters")?;
    let iv_start = match version { 3 | 7 => 25, 5 => 32, 6 => 33, 8 => 31,
        _ => return Err(SourceError::capability("unsupported_image_version")) };
    if decoded.len() < iv_start + 16 || (version == 5 && decoded.len() < 64) {
        return Err(SourceError::protocol("truncated_image_parameters"));
    }
    let iv = decoded[iv_start..iv_start + 16].try_into()
        .map_err(|_| SourceError::protocol("invalid_image_parameters"))?;
    let salt = if version == 5 {
        Some(decoded[48..64].try_into().map_err(|_| SourceError::protocol("invalid_image_parameters"))?)
    } else { None };
    Ok(ImageParameters { iv, salt, timestamp })
}

fn percent_decode(input: &[u8], plus_as_space: bool) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(input.len());
    let mut position = 0;
    while position < input.len() {
        match input[position] {
            b'%' => {
                let high = input.get(position + 1).and_then(|byte| hex_digit(*byte));
                let low = input.get(position + 2).and_then(|byte| hex_digit(*byte));
                let (Some(high), Some(low)) = (high, low) else {
                    return Err(SourceError::protocol("invalid_image_parameters"));
                };
                output.push(high * 16 + low);
                position += 3;
            }
            b'+' if plus_as_space => { output.push(b' '); position += 1; }
            value => { output.push(value); position += 1; }
        }
    }
    Ok(output)
}

fn hex_digit(value: u8) -> Option<u8> {
    match value { b'0'..=b'9' => Some(value - b'0'), b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10), _ => None }
}

fn decode_base64(input: &[u8], code: &'static str) -> Result<Vec<u8>> {
    let decoded = STANDARD.decode(input).map_err(|_| SourceError::protocol(code))?;
    if STANDARD.encode(&decoded).as_bytes() != input {
        return Err(SourceError::protocol(code));
    }
    Ok(decoded)
}

struct ImageContainer {
    payload: Vec<u8>,
    public: [u8; 65],
}

fn image_container(bytes: &[u8], timestamp: Option<u32>, version: u8) -> Result<ImageContainer> {
    if version != 8 {
        if bytes.len() < 71 {
            return Err(SourceError::protocol("truncated_image_container"));
        }
        let length = u32::from_be_bytes(bytes[1..5].try_into()
            .map_err(|_| SourceError::protocol("invalid_image_length"))?) as usize;
        if length == 0 || length.checked_add(70) != Some(bytes.len()) {
            return Err(SourceError::protocol("invalid_image_length"));
        }
        let public: [u8; 65] = bytes[length + 5..].try_into()
            .map_err(|_| SourceError::protocol("invalid_image_public_key"))?;
        if public[0] != 4 {
            return Err(SourceError::protocol("invalid_image_public_key"));
        }
        return Ok(ImageContainer { payload: bytes[5..length + 5].to_vec(), public });
    }
    let length = bytes.len().checked_sub(70).filter(|length| *length >= 69)
        .ok_or_else(|| SourceError::protocol("truncated_image_container"))?;
    let timestamp = timestamp.ok_or_else(|| SourceError::protocol("invalid_image_timestamp"))?;
    let mut state = timestamp.wrapping_sub(length as u32).wrapping_sub(69);
    let mut positions = std::collections::BTreeSet::new();
    // The limit bounds malformed-input work without changing valid sample ordering.
    for _ in 0..1_000_000 {
        if positions.len() == 69 { break; }
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        // Match the source wrapper's IEEE-754 sampling order for large payloads.
        positions.insert(((f64::from(state) / 4_294_967_296.0) * length as f64).floor() as usize);
    }
    if positions.len() != 69 {
        return Err(SourceError::protocol("image_metadata_sampling_limit"));
    }
    let mut payload = Vec::with_capacity(length);
    let mut public = [0u8; 65];
    let mut previous = 1;
    for (index, position) in positions.into_iter().enumerate() {
        let offset = position + 1;
        payload.extend_from_slice(&bytes[previous..offset]);
        if index >= 4 { public[index - 4] = bytes[offset]; }
        previous = offset + 1;
    }
    payload.extend_from_slice(&bytes[previous..]);
    if public[0] != 4 || payload.len() != length {
        return Err(SourceError::protocol("invalid_image_public_key"));
    }
    Ok(ImageContainer { payload, public })
}

fn decrypt_ctr(key: &[u8; 32], iv: &[u8; 16], payload: &[u8], full_counter: bool,
    is_cancelled: &dyn Fn() -> bool) -> Result<Vec<u8>>
{
    check_cancelled(is_cancelled)?;
    let length = payload.len().min(25 * 1024);
    let mut blocks = Vec::with_capacity(length.div_ceil(16) * 16);
    let mut counter = *iv;
    let counter_start = if full_counter { 0 } else { 8 };
    for _ in 0..length.div_ceil(16) {
        blocks.extend_from_slice(&counter);
        for index in (counter_start..16).rev() {
            counter[index] = counter[index].wrapping_add(1);
            if counter[index] != 0 { break; }
        }
    }
    let stream = SensitiveBytes(native::aes256_encrypt(key, &blocks)?);
    check_cancelled(is_cancelled)?;
    let mut output = Vec::with_capacity(payload.len());
    output.extend(payload[..length].iter().zip(&stream.0).map(|(byte, stream)| byte ^ stream));
    output.extend_from_slice(&payload[length..]);
    Ok(output)
}

fn decrypt_cbc(key: &[u8; 32], iv: &[u8; 16], payload: &[u8],
    is_cancelled: &dyn Fn() -> bool) -> Result<Vec<u8>>
{
    check_cancelled(is_cancelled)?;
    let length = payload.len().min(21 * 1024 + 16);
    if length == 0 || length % 16 != 0 {
        return Err(SourceError::protocol("invalid_image_cbc_length"));
    }
    let mut decoded = native::aes256_decrypt(key, &payload[..length])?;
    unchain_cbc(&mut decoded, &payload[..length], iv)?;
    let unpadded = unpadded_length(&decoded)?;
    decoded.truncate(unpadded);
    if let Err(error) = check_cancelled(is_cancelled) { wipe(&mut decoded); return Err(error); }
    decoded.extend_from_slice(&payload[length..]);
    Ok(decoded)
}

fn unchain_cbc(decoded: &mut [u8], ciphertext: &[u8], iv: &[u8; 16]) -> Result<()> {
    if decoded.len() != ciphertext.len() || decoded.len() % 16 != 0 {
        return Err(SourceError::protocol("invalid_cbc_output"));
    }
    let mut previous = *iv;
    for (plain, cipher) in decoded.chunks_exact_mut(16).zip(ciphertext.chunks_exact(16)) {
        for (byte, previous) in plain.iter_mut().zip(previous) { *byte ^= previous; }
        previous.copy_from_slice(cipher);
    }
    Ok(())
}

fn unpadded_length(bytes: &[u8]) -> Result<usize> {
    let padding = bytes.last().copied().unwrap_or(0) as usize;
    if padding == 0 || padding > 16 || padding > bytes.len()
        || !bytes[bytes.len() - padding..].iter().all(|byte| *byte as usize == padding)
    {
        return Err(SourceError::protocol("invalid_pkcs7_padding"));
    }
    Ok(bytes.len() - padding)
}

fn decrypt_gcm(secret: &[u8; 32], parameters: &ImageParameters, payload: &[u8],
    is_cancelled: &dyn Fn() -> bool) -> Result<Vec<u8>>
{
    check_cancelled(is_cancelled)?;
    let length = payload.len().min(30 * 1024 + 16);
    if length < 16 {
        return Err(SourceError::protocol("invalid_image_gcm_length"));
    }
    let salt = parameters.salt.as_ref().ok_or_else(|| SourceError::protocol("missing_image_salt"))?;
    let key = native::pbkdf2(secret, salt, 100_000)?;
    check_cancelled(is_cancelled)?;
    let mut decoded = native::gcm(key.bytes(), &parameters.iv, salt,
        &payload[..length - 16], &payload[length - 16..length])?;
    if let Err(error) = check_cancelled(is_cancelled) { wipe(&mut decoded); return Err(error); }
    decoded.extend_from_slice(&payload[length..]);
    Ok(decoded)
}

struct SensitiveBytes(Vec<u8>);

impl Drop for SensitiveBytes {
    fn drop(&mut self) { wipe(&mut self.0); }
}

struct Sensitive<const N: usize>(Box<[u8; N]>);

impl<const N: usize> Sensitive<N> {
    fn zeroed() -> Self { Self(Box::new([0; N])) }
    fn bytes(&self) -> &[u8; N] { &self.0 }
}

impl<const N: usize> Drop for Sensitive<N> {
    fn drop(&mut self) { wipe(self.0.as_mut_slice()); }
}

fn wipe(bytes: &mut [u8]) {
    for byte in bytes {
        // Volatile stores prevent the compiler from removing explicit secret cleanup.
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

#[cfg(target_os = "linux")]
mod native {
    use super::*;
    use std::{ffi::{c_char, c_int, c_uint, c_ulong, CStr, CString}, sync::Mutex};

    static WASM_LOCK: Mutex<()> = Mutex::new(());

    unsafe extern "C" {
        fn biliwasm_run_bytes(bytes: *const u8, length: usize, request: *const c_char, output: *mut *mut c_char) -> c_int;
        fn biliwasm_free(output: *mut c_char);
        fn bili_p256_new(private: *mut u8, public: *mut u8) -> c_int;
        fn bili_p256_derive(private: *const u8, public: *const u8, secret: *mut u8) -> c_int;
        fn bili_aes192_ecb_decrypt(key: *const u8, input: *const u8, length: c_ulong, output: *mut u8) -> c_int;
        fn bili_aes256_ecb_encrypt(key: *const u8, input: *const u8, length: c_ulong, output: *mut u8) -> c_int;
        fn bili_aes256_ecb_decrypt(key: *const u8, input: *const u8, length: c_ulong, output: *mut u8) -> c_int;
        fn bili_pbkdf2_sha512(password: *const u8, passlen: usize, salt: *const u8, saltlen: usize,
            iterations: c_uint, output: *mut u8, outlen: usize) -> c_int;
        fn bili_aes_gcm_decrypt(key: *const u8, keylen: usize, iv: *const u8, ivlen: usize,
            aad: *const u8, aadlen: usize, cipher: *const u8, cipherlen: usize,
            tag: *const u8, taglen: usize, output: *mut u8) -> c_int;
    }

    struct Output(*mut c_char);
    impl Drop for Output {
        fn drop(&mut self) { if !self.0.is_null() { unsafe { biliwasm_free(self.0) }; } }
    }

    pub(super) fn invoke(cache: &super::super::assets::SigningAssetCache, path: &Path,
        request: &Value, is_cancelled: &dyn Fn() -> bool) -> Result<Value>
    {
        let asset = cache.load_if_present_cancellable(path, is_cancelled)?
            .ok_or_else(|| SourceError::capability("signing_asset_missing"))?;
        check_cancelled(is_cancelled)?;
        let _guard = WASM_LOCK.lock().map_err(|_| SourceError::capability("native_signing_unavailable"))?;
        check_cancelled(is_cancelled)?;
        let request = CString::new(request.to_string())
            .map_err(|_| SourceError::invalid("invalid_signing_request"))?;
        check_cancelled(is_cancelled)?;
        let mut output = Output(std::ptr::null_mut());
        // The Arc remains on this stack through C cleanup, including passive
        // data segments that still borrow the immutable module bytes at runtime.
        let status = unsafe { biliwasm_run_bytes(asset.bytes().as_ptr(), asset.bytes().len(), request.as_ptr(), &mut output.0) };
        check_cancelled(is_cancelled)?;
        if output.0.is_null() {
            return Err(SourceError::capability("native_signing_unavailable"));
        }
        let result: Value = serde_json::from_slice(unsafe { CStr::from_ptr(output.0) }.to_bytes())
            .map_err(|_| SourceError::protocol("invalid_native_signing_result"))?;
        if status != 0 || result.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(SourceError::capability("native_signing_failed"));
        }
        let result = result.get("result").filter(|result| result.is_object())
            .ok_or_else(|| SourceError::protocol("invalid_native_signing_result"))?;
        if result.get("error").and_then(Value::as_str).is_some_and(|error| !error.is_empty()) {
            return Err(SourceError::protocol("native_signing_rejected"));
        }
        Ok(result.clone())
    }

    pub(super) fn new_key() -> Result<ImageKey> {
        let mut private = Sensitive::zeroed();
        let mut public = [0; 65];
        if unsafe { bili_p256_new(private.0.as_mut_ptr(), public.as_mut_ptr()) } != 1 || public[0] != 4 {
            return Err(SourceError::capability("image_key_generation_failed"));
        }
        Ok(ImageKey { private, public })
    }

    pub(super) fn derive(private: &[u8; 32], public: &[u8; 65]) -> Result<Sensitive<32>> {
        let mut secret = Sensitive::zeroed();
        if unsafe { bili_p256_derive(private.as_ptr(), public.as_ptr(), secret.0.as_mut_ptr()) } != 1 {
            return Err(SourceError::protocol("image_key_agreement_failed"));
        }
        Ok(secret)
    }

    fn aes(key: &[u8], input: &[u8], operation: unsafe extern "C" fn(*const u8, *const u8, c_ulong, *mut u8) -> c_int)
        -> Result<Vec<u8>> {
        if input.len() % 16 != 0 || input.len() > MAX_IMAGE_BYTES {
            return Err(SourceError::protocol("invalid_aes_block_length"));
        }
        let length = c_ulong::try_from(input.len()).map_err(|_| SourceError::protocol("invalid_aes_block_length"))?;
        let mut output = vec![0; input.len()];
        if unsafe { operation(key.as_ptr(), input.as_ptr(), length, output.as_mut_ptr()) } != 1 {
            wipe(&mut output);
            return Err(SourceError::protocol("aes_transform_failed"));
        }
        Ok(output)
    }

    pub(super) fn aes192_decrypt(key: &[u8; 24], input: &[u8]) -> Result<Vec<u8>> {
        aes(key, input, bili_aes192_ecb_decrypt)
    }
    pub(super) fn aes256_encrypt(key: &[u8; 32], input: &[u8]) -> Result<Vec<u8>> {
        aes(key, input, bili_aes256_ecb_encrypt)
    }
    pub(super) fn aes256_decrypt(key: &[u8; 32], input: &[u8]) -> Result<Vec<u8>> {
        aes(key, input, bili_aes256_ecb_decrypt)
    }

    pub(super) fn pbkdf2(password: &[u8; 32], salt: &[u8; 16], iterations: u32) -> Result<Sensitive<32>> {
        let mut key = Sensitive::zeroed();
        if unsafe { bili_pbkdf2_sha512(password.as_ptr(), password.len(), salt.as_ptr(), salt.len(),
            iterations, key.0.as_mut_ptr(), 32) } != 1 {
            return Err(SourceError::protocol("image_key_derivation_failed"));
        }
        Ok(key)
    }

    pub(super) fn gcm(key: &[u8; 32], iv: &[u8; 16], aad: &[u8; 16], cipher: &[u8], tag: &[u8]) -> Result<Vec<u8>> {
        if tag.len() != 16 || cipher.len() > MAX_IMAGE_BYTES {
            return Err(SourceError::protocol("invalid_image_authentication_input"));
        }
        let mut output = vec![0; cipher.len()];
        if unsafe { bili_aes_gcm_decrypt(key.as_ptr(), key.len(), iv.as_ptr(), iv.len(), aad.as_ptr(), aad.len(),
            cipher.as_ptr(), cipher.len(), tag.as_ptr(), tag.len(), output.as_mut_ptr()) } != 1 {
            wipe(&mut output);
            return Err(SourceError::protocol("image_authentication_failed"));
        }
        Ok(output)
    }

    #[cfg(test)]
    mod asset_input_tests {
        use super::*;

        unsafe extern "C" {
            fn biliwasm_run(path: *const c_char, request: *const c_char, output: *mut *mut c_char) -> c_int;
        }

        #[test]
        fn bytes_input_rejects_invalid_lengths_before_reading_and_clears_the_output() {
            let _guard = WASM_LOCK.lock().unwrap();
            let request = CString::new("{}").unwrap();
            let bytes = [0u8; 8];
            for (pointer, length) in [(std::ptr::null(), 8usize), (bytes.as_ptr(), 0), (bytes.as_ptr(), usize::MAX)] {
                let mut output = std::ptr::NonNull::<c_char>::dangling().as_ptr();
                let status = unsafe { biliwasm_run_bytes(pointer, length, request.as_ptr(), &mut output) };
                assert_ne!(status, 0);
                assert!(output.is_null());
            }
        }

        #[test]
        fn borrowed_module_and_path_inputs_have_matching_isolated_failure_cleanup() {
            let _guard = WASM_LOCK.lock().unwrap();
            // A valid empty module has no Go exports. It exercises parse/load
            // cleanup without any public asset, account data, or network.
            let bytes = b"\0asm\x01\0\0\0";
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("empty.wasm");
            std::fs::write(&path, bytes).unwrap();
            let path = CString::new(path.to_str().unwrap()).unwrap();
            let request = CString::new(r#"{"function":"y1_z2w2a3","args":[]}"#).unwrap();
            let mut expected = Output(std::ptr::null_mut());
            let status = unsafe { biliwasm_run(path.as_ptr(), request.as_ptr(), &mut expected.0) };
            assert_ne!(status, 0);
            assert!(!expected.0.is_null());
            let expected: Value = serde_json::from_slice(unsafe { CStr::from_ptr(expected.0) }.to_bytes()).unwrap();
            for _ in 0..4 {
                let mut output = Output(std::ptr::null_mut());
                let status = unsafe { biliwasm_run_bytes(bytes.as_ptr(), bytes.len(), request.as_ptr(), &mut output.0) };
                assert_ne!(status, 0);
                assert!(!output.0.is_null(), "A previous failure must not retain active_job");
                let decoded: Value = serde_json::from_slice(unsafe { CStr::from_ptr(output.0) }.to_bytes()).unwrap();
                assert_eq!(decoded, expected);
                assert_eq!(decoded["ok"], false);
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod native {
    use super::*;
    fn unavailable<T>() -> Result<T> { Err(SourceError::capability("native_crypto_requires_linux")) }
    pub(super) fn invoke(_: &super::super::assets::SigningAssetCache, _: &Path, _: &Value,
        _: &dyn Fn() -> bool) -> Result<Value> { unavailable() }
    pub(super) fn new_key() -> Result<ImageKey> { unavailable() }
    pub(super) fn derive(_: &[u8; 32], _: &[u8; 65]) -> Result<Sensitive<32>> { unavailable() }
    pub(super) fn aes192_decrypt(_: &[u8; 24], _: &[u8]) -> Result<Vec<u8>> { unavailable() }
    pub(super) fn aes256_encrypt(_: &[u8; 32], _: &[u8]) -> Result<Vec<u8>> { unavailable() }
    pub(super) fn aes256_decrypt(_: &[u8; 32], _: &[u8]) -> Result<Vec<u8>> { unavailable() }
    pub(super) fn pbkdf2(_: &[u8; 32], _: &[u8; 16], _: u32) -> Result<Sensitive<32>> { unavailable() }
    pub(super) fn gcm(_: &[u8; 32], _: &[u8; 16], _: &[u8; 16], _: &[u8], _: &[u8]) -> Result<Vec<u8>> { unavailable() }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    #[test]
    fn signing_rejects_embedded_nul_before_native_module_loading() {
        let crypto = NativeCrypto::new(Path::new("unused-signing-assets"));
        let context = RequestContext { endpoint:"ComicDetail".into(), path:String::new(), url:String::new(),
            body:"{}".into(), sign_query:"device=pc\0&platform=web".into(), buvid:String::new(),
            platform:"web".into(), timestamp_ms:1_793_664_000_123 };
        let error = crypto.sign(&context).err().expect("A C-string boundary must reject an actual NUL");
        assert_eq!((error.kind, error.code.as_str()), (super::super::ErrorKind::InvalidRequest,"invalid_signing_context"));
    }

    fn fixture_context(value: &Value) -> RequestContext {
        RequestContext {
            endpoint: String::new(), path: String::new(),
            url: value["url"].as_str().expect("Fixture URL").to_owned(),
            body: value["body"].as_str().expect("Fixture body").to_owned(),
            sign_query: String::new(),
            buvid: value["buvid"].as_str().expect("Fixture device identifier").to_owned(),
            platform: value["platform"].as_str().expect("Fixture platform").to_owned(),
            timestamp_ms: 0,
        }
    }

    fn fixture_key(fixture: &Value) -> ImageKey {
        let json = STANDARD.decode(fixture["privateKey"].as_str().expect("Synthetic private JWK"))
            .expect("Fixture Base64");
        let jwk: Value = serde_json::from_slice(&json).expect("Fixture JWK");
        let scalar = URL_SAFE_NO_PAD.decode(jwk["d"].as_str().expect("Synthetic scalar"))
            .expect("Fixture scalar encoding");
        assert_eq!(scalar.len(), 32);
        let mut private = Sensitive::zeroed();
        private.0.copy_from_slice(&scalar);
        let mut public = [0; 65];
        public[0] = 4;
        for (start, name) in [(1, "x"), (33, "y")] {
            let coordinate = URL_SAFE_NO_PAD.decode(jwk[name].as_str().expect("Synthetic public point"))
                .expect("Fixture coordinate encoding");
            assert_eq!(coordinate.len(), 32);
            public[start..start + 32].copy_from_slice(&coordinate);
        }
        ImageKey { private, public }
    }

    #[test]
    fn response_decoder_matches_independent_official_wasm_golden() {
        let fixtures: Value = serde_json::from_str(include_str!(
            "../../assets/protocol/fixtures/crypto-response-fixtures.json")).expect("Response fixtures");
        let crypto = NativeCrypto::new(Path::new("unused"));
        for fixture in fixtures.as_array().expect("Response cases") {
            let context = fixture_context(&fixture["context"]);
            assert_eq!(response_key(&context).expect("Response key").bytes().as_slice(),
                fixture["key"].as_str().expect("Expected key").as_bytes());
            let envelope = serde_json::json!({"code": 0, "message": "", "marker": 7,
                "bytesData": fixture["bytesData"]});
            let decoded = crypto.decode_response(&context, envelope).expect("Golden response decode");
            let expected: Value = serde_json::from_str(
                fixture["official"]["data"].as_str().expect("Official output")).expect("Official JSON");
            assert_eq!(decoded["data"], expected);
            assert_eq!(decoded["marker"], 7);
            assert!(decoded.get("bytesData").is_none());
        }
    }

    #[test]
    fn response_decoder_rejects_malformed_padding_utf8_and_json() {
        let fixtures: Value = serde_json::from_str(include_str!(
            "../../assets/protocol/fixtures/crypto-response-invalid-fixtures.json")).expect("Invalid fixtures");
        let context = fixture_context(&fixtures["context"]);
        let crypto = NativeCrypto::new(Path::new("unused"));
        for fixture in fixtures["fixtures"].as_array().expect("Invalid cases") {
            assert!(crypto.decode_response(&context, serde_json::json!({"code": 0,
                "bytesData": fixture["data"]})).is_err());
        }
        assert!(crypto.decode_response(&context, serde_json::json!({"code": 0,
            "bytesData": "Zg=="})).is_err());
        let business = serde_json::json!({"code": 99, "bytesData": "invalid"});
        assert_eq!(crypto.decode_response(&context, business.clone()).expect("Business envelope"), business);
    }

    #[test]
    fn encrypted_images_match_official_golden_for_all_versions_and_counter_widths() {
        let legacy: Value = serde_json::from_str(include_str!(
            "../../assets/protocol/fixtures/image-legacy-fixtures.json")).expect("Legacy fixtures");
        let counters: Value = serde_json::from_str(include_str!(
            "../../assets/protocol/fixtures/image-legacy-counter-fixtures.json")).expect("Counter fixtures");
        let v8: Value = serde_json::from_str(include_str!(
            "../../assets/protocol/fixtures/image-v8-fixtures.json")).expect("Version eight fixtures");
        let crypto = NativeCrypto::new(Path::new("unused"));
        let cases = legacy.as_array().expect("Legacy cases").iter()
            .chain(counters.as_array().expect("Counter cases"))
            .chain(v8["cases"].as_array().expect("Version eight cases"));
        let mut count = 0;
        for fixture in cases {
            let key = fixture_key(fixture);
            let body = STANDARD.decode(fixture["bodyBase64"].as_str().expect("Fixture container"))
                .expect("Container Base64");
            let expected = STANDARD.decode(fixture["expectedBase64"].as_str().expect("Official plaintext"))
                .expect("Plaintext Base64");
            let decoded = crypto.decrypt_image(&key, fixture["url"].as_str().expect("Fixture URL"), &body)
                .expect("Golden image conversion");
            assert_eq!(decoded, expected, "Image version {}", fixture["version"]);
            assert_eq!(STANDARD.decode(key.m1()).expect("Public m1").len(), 65);
            assert!(!format!("{key:?}").contains(fixture["privateKey"].as_str().expect("Fixture key")));
            count += 1;
        }
        assert_eq!(count, 19);
    }

    #[test]
    fn image_decryption_rejects_modified_gcm_and_malformed_parameters() {
        let fixtures: Value = serde_json::from_str(include_str!(
            "../../assets/protocol/fixtures/image-legacy-fixtures.json")).expect("Legacy fixtures");
        let fixture = fixtures.as_array().expect("Legacy cases").iter()
            .find(|fixture| fixture["version"] == 5).expect("Authenticated image fixture");
        let key = fixture_key(fixture);
        let mut body = STANDARD.decode(fixture["bodyBase64"].as_str().expect("Container"))
            .expect("Container Base64");
        let crypto = NativeCrypto::new(Path::new("unused"));
        body[5] ^= 1;
        assert!(crypto.decrypt_image(&key, fixture["url"].as_str().expect("Fixture URL"), &body).is_err());
        for url in ["https://example.invalid/a?cpx=%QZ", "https://example.invalid/a?cpx=%A",
            "https://example.invalid/a?cpx=x&other=%", "https://example.invalid/a?cpx=x;other=y"] {
            assert!(image_parameters(url, 5).is_err());
        }
        assert!(image_parameters("https://example.invalid/a?ts=8000000000000000&cpx=AAAA", 8).is_err());
        assert!(crypto.decrypt_image(&key, "https://example.invalid/a", &[9; 100]).is_err());
    }

    #[test]
    fn native_p256_produces_fresh_uncompressed_public_points() {
        let crypto = NativeCrypto::new(Path::new("unused"));
        let first = crypto.new_image_key().expect("First key");
        let second = crypto.new_image_key().expect("Second key");
        assert_ne!(first.m1(), second.m1());
        let public = STANDARD.decode(first.m1()).expect("m1 Base64");
        assert_eq!(public.len(), 65);
        assert_eq!(public[0], 4);
    }
}
