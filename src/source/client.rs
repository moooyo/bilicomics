use super::{AuthSession, ErrorKind, Result, SessionFile, SourceError};
use super::crypto::{NativeCrypto, ProtocolCrypto, RequestContext};
use super::transport::{HttpRequest, HttpResponse, ReqwestTransport, Transport, check_status};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU64, Ordering}};

pub(super) const MANGA_ORIGIN: &str = "https://manga.bilibili.com";
pub(super) const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux) AppleWebKit/537.36 Chrome/130.0.0.0 Safari/537.36";
pub(super) const MAX_JSON_BYTES: usize = 8 * 1024 * 1024;
const SIGNING_FILE: &str = "efae82c96a7eef44bee5.wasm";
const SIGNING_URL: &str = "https://s1.hdslb.com/bfs/manga-static/manga-pc/efae82c96a7eef44bee5.wasm";
pub(super) const SIGNING_SHA256: &str = "39bc0676953752c461197df592e1f5894f1a7492a29400c946e560fc109a8e2e";
static NEXT_ASSET: AtomicU64 = AtomicU64::new(1);
static NEXT_RENEWAL_MARKER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct BiliClient {
    pub(super) transport: Arc<dyn Transport>,
    pub(super) crypto: Arc<dyn ProtocolCrypto>,
    pub(super) session: Arc<Mutex<Option<AuthSession>>>,
    session_file: Option<Arc<SessionFile>>,
    persistence_blocked: Arc<AtomicBool>,
    pub(super) session_epoch: Arc<AtomicU64>,
    pub(super) account_epoch: Arc<AtomicU64>,
    pub(super) login_epoch: Arc<AtomicU64>,
    pub(super) login_key: Arc<Mutex<Option<String>>>,
    renewal_marker: Arc<AtomicU64>,
    expected_account: Option<Arc<str>>,
    asset_root: Arc<PathBuf>,
    asset_lock: Arc<tokio::sync::Mutex<()>>,
    manage_assets: bool,
}

impl std::fmt::Debug for BiliClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("BiliClient").field("configured", &self.session().is_some()).finish_non_exhaustive()
    }
}

#[derive(Default)]
pub(super) struct PostOptions {
    pub require_session: bool,
    pub expected_epoch: Option<u64>,
    pub anonymous: bool,
    pub query: Vec<(String, String)>,
    pub flag: Option<&'static str>,
    pub guest_device: Option<String>,
}

impl BiliClient {
    /// Permanently disables transport and protocol work for isolated demonstrations.
    /// This client never constructs an HTTP client or acquires signing assets.
    pub fn offline(data_dir: &Path) -> Result<Self> {
        Ok(Self::with_adapters(None, data_dir,
            Arc::new(super::offline::OfflineTransport), Arc::new(super::offline::OfflineCrypto)))
    }

    pub fn new(session: Option<AuthSession>, data_dir: &Path) -> Result<Self> {
        let asset_root = data_dir.join("protocol-assets");
        Ok(Self {
            transport: Arc::new(ReqwestTransport::new()?),
            crypto: Arc::new(NativeCrypto::new(&asset_root)),
            session: Arc::new(Mutex::new(session)), session_file: None,
            persistence_blocked: Arc::new(AtomicBool::new(false)), session_epoch: Arc::new(AtomicU64::new(0)),
            account_epoch: Arc::new(AtomicU64::new(0)),
            login_epoch: Arc::new(AtomicU64::new(0)),
            login_key: Arc::new(Mutex::new(None)),
            renewal_marker: Arc::new(AtomicU64::new(0)),
            expected_account: None,
            asset_root: Arc::new(asset_root),
            asset_lock: Arc::new(tokio::sync::Mutex::new(())), manage_assets: true,
        })
    }

    /// Fixture and alternative-provider integration never silently substitutes successful data.
    pub fn with_adapters(session: Option<AuthSession>, data_dir: &Path,
        transport: Arc<dyn Transport>, crypto: Arc<dyn ProtocolCrypto>) -> Self {
        Self { transport, crypto, session: Arc::new(Mutex::new(session)), session_file: None,
            persistence_blocked: Arc::new(AtomicBool::new(false)),
            session_epoch: Arc::new(AtomicU64::new(0)), login_epoch: Arc::new(AtomicU64::new(0)),
            account_epoch: Arc::new(AtomicU64::new(0)),
            login_key: Arc::new(Mutex::new(None)),
            renewal_marker: Arc::new(AtomicU64::new(0)),
            expected_account: None,
            asset_root: Arc::new(data_dir.join("protocol-assets")),
            asset_lock: Arc::new(tokio::sync::Mutex::new(())), manage_assets: false }
    }

    /// Opt in only for the active application client, never an import candidate.
    /// Every shared-session mutation saves under the same lock before publication.
    pub fn with_session_file(mut self, session_file: SessionFile) -> Self {
        self.session_file = Some(Arc::new(session_file));
        self
    }

    /// Validate a newly issued QR candidate independently of an uncertain active store.
    /// This client has no active credentials, persistence, or shared login authority.
    pub(super) fn isolated_auth_client(&self) -> Self {
        Self::with_adapters(None, self.asset_root.as_ref(), self.transport.clone(), self.crypto.clone())
    }

    fn persist_session(&self, value: Option<&AuthSession>, explicit_recovery: bool) -> Result<()> {
        if self.persistence_blocked.load(Ordering::SeqCst) && !explicit_recovery {
            return Err(SourceError::storage("session_storage_outcome_unresolved"));
        }
        let Some(file) = &self.session_file else { return Ok(()); };
        let result = if let Some(session) = value.filter(|session| session.has_session()) { file.save(session) }
            else { file.clear() };
        if result.as_ref().is_err_and(|error| matches!(error.code.as_str(),
            "session_publish_sync_unknown" | "session_clear_sync_unknown")) {
            // The namespace changed on disk, but its crash durability is unknown.
            // Retire old in-flight contexts and prevent old RAM from overwriting
            // the published file. Only a new explicit login or logout can recover.
            self.persistence_blocked.store(true, Ordering::SeqCst);
            self.session_epoch.fetch_add(1, Ordering::SeqCst);
            self.account_epoch.fetch_add(1, Ordering::SeqCst);
        } else if result.is_ok() && explicit_recovery {
            self.persistence_blocked.store(false, Ordering::SeqCst);
        }
        result
    }

    pub fn session(&self) -> Option<AuthSession> {
        let session = self.session_snapshot().1;
        self.require_bound_account(session.as_ref()).ok()?;
        session
    }

    /// Keeps shared credentials while permanently restricting this clone to one verified account.
    pub fn bound_to_account(&self, expected_account: &str) -> Result<Self> {
        let (_, session) = self.session_snapshot();
        self.require_bound_account(session.as_ref())?;
        if self.expected_account.as_deref().is_some_and(|existing| existing != expected_account)
            || session.as_ref().and_then(AuthSession::account_key) != Some(expected_account) {
            return Err(bound_account_mismatch());
        }
        let mut client = self.clone();
        client.expected_account = Some(Arc::from(expected_account));
        Ok(client)
    }

    pub(super) fn require_bound_account(&self, session: Option<&AuthSession>) -> Result<()> {
        if self.expected_account.as_deref().is_some_and(|expected|
            session.and_then(AuthSession::account_key) != Some(expected)) {
            return Err(bound_account_mismatch());
        }
        Ok(())
    }

    /// Capture this version before scheduling work that may rotate account credentials.
    pub fn credential_epoch(&self) -> u64 { self.session_generation() }

    /// Returns an atomic account/version pair without exposing credential material.
    pub fn credential_context(&self) -> Option<(String, u64)> {
        let (epoch, session) = self.session_snapshot();
        self.require_bound_account(session.as_ref()).ok()?;
        let session = session?;
        Some((session.account_key()?.to_owned(), epoch))
    }

    pub(super) fn session_snapshot(&self) -> (u64, Option<AuthSession>) {
        let session = self.session.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        (self.session_epoch.load(Ordering::SeqCst),
            if self.persistence_blocked.load(Ordering::SeqCst) { None } else { session.clone() })
    }

    pub fn replace_session(&self, value: Option<AuthSession>) -> Result<()> {
        let mut session = self.session.lock().map_err(|_| SourceError::protocol("session_lock_failed"))?;
        self.require_bound_account(session.as_ref())?;
        self.require_bound_account(value.as_ref())?;
        self.persist_session(value.as_ref(), true)?;
        *session = value;
        self.renewal_marker.store(0, Ordering::SeqCst);
        self.session_epoch.fetch_add(1, Ordering::SeqCst);
        self.account_epoch.fetch_add(1, Ordering::SeqCst);
        self.login_epoch.fetch_add(1, Ordering::SeqCst);
        drop(session);
        *self.login_key.lock().map_err(|_| SourceError::protocol("login_lock_failed"))? = None;
        Ok(())
    }

    pub(super) fn session_generation(&self) -> u64 { self.session_epoch.load(Ordering::SeqCst) }

    pub(super) fn replace_session_if_epoch(&self, expected: u64, value: Option<AuthSession>) -> Result<bool> {
        let mut session = self.session.lock().map_err(|_| SourceError::protocol("session_lock_failed"))?;
        if self.session_epoch.load(Ordering::SeqCst) != expected { return Ok(false); }
        self.require_bound_account(session.as_ref())?;
        self.require_bound_account(value.as_ref())?;
        let changed_account = account_namespace(session.as_ref()) != account_namespace(value.as_ref());
        let changed_credentials = match (session.as_ref(), value.as_ref()) {
            (Some(current), Some(candidate)) => !current.same_credentials(candidate),
            (None, None) => false,
            _ => true,
        };
        self.persist_session(value.as_ref(), false)?;
        *session = value;
        if changed_credentials { self.renewal_marker.store(0, Ordering::SeqCst); }
        self.session_epoch.fetch_add(1, Ordering::SeqCst);
        if changed_account { self.account_epoch.fetch_add(1, Ordering::SeqCst); }
        Ok(true)
    }

    /// An obsolete authentication failure must not remove a newer saved session.
    pub fn clear_session_if_epoch(&self, expected: u64) -> Result<bool> {
        let mut session = self.session.lock().map_err(|_| SourceError::protocol("session_lock_failed"))?;
        if self.session_epoch.load(Ordering::SeqCst) != expected { return Ok(false); }
        self.require_bound_account(session.as_ref())?;
        self.persist_session(None, false)?;
        *session = None;
        self.renewal_marker.store(0, Ordering::SeqCst);
        self.session_epoch.fetch_add(1, Ordering::SeqCst);
        self.account_epoch.fetch_add(1, Ordering::SeqCst);
        self.login_epoch.fetch_add(1, Ordering::SeqCst);
        drop(session);
        *self.login_key.lock().map_err(|_| SourceError::protocol("login_lock_failed"))? = None;
        Ok(true)
    }

    pub(super) fn mark_renewal_if_epoch(&self, expected: u64, candidate: AuthSession) -> Result<Option<(u64, u64)>> {
        let mut session = self.session.lock().map_err(|_| SourceError::protocol("session_lock_failed"))?;
        if self.session_epoch.load(Ordering::SeqCst) != expected { return Ok(None); }
        if !session.as_ref().is_some_and(|current| current.same_credentials(&candidate)
            && current.account_key() == candidate.account_key()) { return Ok(None); }
        let marker = loop {
            let current = NEXT_RENEWAL_MARKER.load(Ordering::SeqCst);
            let next = current.checked_add(1).ok_or_else(|| SourceError::protocol("renewal_marker_exhausted"))?;
            if NEXT_RENEWAL_MARKER.compare_exchange(current, next, Ordering::SeqCst, Ordering::SeqCst).is_ok() { break current; }
        };
        self.persist_session(Some(&candidate), false)?;
        *session = Some(candidate);
        let epoch = self.session_epoch.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
        self.renewal_marker.store(marker, Ordering::SeqCst);
        Ok(Some((epoch, marker)))
    }

    /// Clear only this attempt's marker while preserving unrelated cookie updates.
    pub(super) fn clear_renewal_attempt_if_owned(&self, marker: u64, original: &AuthSession, confirmation: bool) -> Result<bool> {
        let mut session = self.session.lock().map_err(|_| SourceError::protocol("session_lock_failed"))?;
        if self.renewal_marker.load(Ordering::SeqCst) != marker { return Ok(false); }
        let Some(current) = session.as_ref() else { return Ok(false); };
        if !current.same_credentials(original) || current.account_key() != original.account_key() { return Ok(false); }
        let mut candidate = current.clone();
        if confirmation { candidate.confirmation_blocked = original.confirmation_blocked; }
        else { candidate.refresh_blocked = original.refresh_blocked; }
        self.persist_session(Some(&candidate), false)?;
        *session = Some(candidate);
        self.renewal_marker.store(0, Ordering::SeqCst);
        self.session_epoch.fetch_add(1, Ordering::SeqCst);
        Ok(true)
    }

    fn apply_http_response_cookies(&self, original: &AuthSession, response: &HttpResponse,
        host: &str, path: &str, account_epoch: Option<u64>, expected_epoch: Option<u64>) -> Result<()> {
        let mut updated = original.clone();
        updated.apply_set_cookie_for_path(&response.headers, host, path, now_seconds())?;
        if &updated == original { return Ok(()); }
        let mut session = self.session.lock().map_err(|_| SourceError::protocol("session_lock_failed"))?;
        let Some(current) = session.as_ref() else { return Ok(()); };
        self.require_bound_account(Some(current))?;
        // A response sent with retired credentials cannot restore them after a
        // refresh, logout, or account replacement. Concurrent independent cookie
        // changes merge instead of losing the second response to an epoch CAS.
        if account_epoch.is_some_and(|expected| self.account_epoch.load(Ordering::SeqCst) != expected)
            || expected_epoch.is_some_and(|expected| self.session_epoch.load(Ordering::SeqCst) != expected)
            || !current.same_credentials(original) { return Ok(()); }
        let candidate = current.merge_cookie_updates(original, &updated)?;
        if &candidate == current { return Ok(()); }
        let next = candidate.has_session().then_some(candidate);
        let changed_account = account_namespace(Some(current)) != account_namespace(next.as_ref());
        let changed_credentials = next.as_ref().is_none_or(|candidate| !current.same_credentials(candidate));
        self.persist_session(next.as_ref(), false)?;
        *session = next;
        if changed_credentials { self.renewal_marker.store(0, Ordering::SeqCst); }
        self.session_epoch.fetch_add(1, Ordering::SeqCst);
        if changed_account { self.account_epoch.fetch_add(1, Ordering::SeqCst); }
        Ok(())
    }

    pub(super) fn apply_auth_error_cookies(&self, original: &AuthSession, response: &HttpResponse,
        host: &str, path: &str, expected_epoch: u64) -> Result<()> {
        self.apply_http_response_cookies(original, response, host, path, None, Some(expected_epoch))
    }

    fn response_error(&self, original: Option<&AuthSession>, account_epoch: u64, error: SourceError) -> SourceError {
        if error.kind != ErrorKind::Authentication { return error; }
        let Some(original) = original else { return error; };
        let clear = (|| -> Result<()> {
            let mut session = self.session.lock().map_err(|_| SourceError::protocol("session_lock_failed"))?;
            if self.account_epoch.load(Ordering::SeqCst) != account_epoch
                || !session.as_ref().is_some_and(|current| current.same_credentials(original)) {
                return Ok(());
            }
            self.require_bound_account(session.as_ref())?;
            self.persist_session(None, false)?;
            *session = None;
            self.renewal_marker.store(0, Ordering::SeqCst);
            self.session_epoch.fetch_add(1, Ordering::SeqCst);
            self.account_epoch.fetch_add(1, Ordering::SeqCst);
            self.login_epoch.fetch_add(1, Ordering::SeqCst);
            drop(session);
            *self.login_key.lock().map_err(|_| SourceError::protocol("login_lock_failed"))? = None;
            Ok(())
        })();
        match clear { Ok(()) => error, Err(storage) => storage }
    }

    pub(super) async fn send(&self, request: HttpRequest) -> Result<HttpResponse> {
        if self.persistence_blocked.load(Ordering::SeqCst)
            && request.headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("cookie")) {
            return Err(SourceError { transmitted: Some(false),
                ..SourceError::storage("session_storage_outcome_unresolved") });
        }
        self.require_bound_account(self.session_snapshot().1.as_ref())?;
        let response = self.transport.send(request).await?;
        self.require_bound_account(self.session_snapshot().1.as_ref()).map_err(|mut error| {
            error.transmitted = Some(true);
            error.definitive = false;
            error
        })?;
        Ok(response)
    }

    pub(super) async fn post(&self, service: &str, endpoint: &str, body: Value, options: PostOptions) -> Result<Value> {
        let (epoch, original_session) = self.session_snapshot();
        self.require_bound_account(original_session.as_ref())?;
        let account_epoch = self.account_epoch.load(Ordering::SeqCst);
        if options.expected_epoch.is_some_and(|expected| expected != epoch) {
            return Err(SourceError { transmitted: Some(false), definitive: true,
                ..SourceError::new(ErrorKind::Cancelled, "account_changed") });
        }
        let session = if options.guest_device.is_some() || options.anonymous { None } else { original_session.clone() };
        if options.require_session && !session.as_ref().is_some_and(|session| session.has_session_for("manga.bilibili.com")) {
            return Err(SourceError { transmitted: Some(false), ..SourceError::authentication("session_required") });
        }
        let path = format!("/twirp/{service}/{endpoint}");
        let encoded = serde_json::to_string(&body).map_err(|_| SourceError::invalid("request_json_invalid"))?;
        let mut url = url::Url::parse(&format!("{MANGA_ORIGIN}{path}"))
            .map_err(|_| SourceError::invalid("request_endpoint_invalid"))?;
        { let mut query = url.query_pairs_mut();
            query.extend_pairs([("device", "pc"), ("platform", "web"), ("nov", "27"), ("a", "810")]);
            for (name, value) in &options.query { query.append_pair(name, value); }
        }
        if let Some(flag) = options.flag {
            let query = format!("{}&{flag}", url.query().unwrap_or_default()); url.set_query(Some(&query));
        }
        let mut headers = vec![ ("user-agent".into(), USER_AGENT.into()),
            ("accept".into(), "application/json, text/plain, */*".into()),
            ("content-type".into(), "application/json;charset=UTF-8".into()),
            ("referer".into(), format!("{MANGA_ORIGIN}/")), ("origin".into(), MANGA_ORIGIN.into()) ];
        if let Some(device) = &options.guest_device { headers.push(("cookie".into(), format!("buvid3={device}"))); }
        else if let Some(session) = &session {
            let cookie = session.cookie_header("manga.bilibili.com")?;
            if !cookie.is_empty() { headers.push(("cookie".into(), cookie)); }
            if let Some(value) = session.cookie_value_for("manga.bilibili.com", "XSRF-TOKEN") {
                headers.push(("x-xsrf-token".into(), percent_decode(value)?));
            }
        }
        let buvid = options.guest_device.clone().or_else(|| session.as_ref().and_then(|session|
            session.cookie_value_for("manga.bilibili.com", "buvid3")
                .or_else(|| session.cookie_value_for("manga.bilibili.com", "buvid4")).map(str::to_owned))).unwrap_or_default();
        let mut context = RequestContext { endpoint: endpoint.into(), path, url: String::new(), body: encoded.clone(),
            sign_query: "device=pc&platform=web&nov=27&eot=812".into(), buvid, platform: "web".into(),
            timestamp_ms: now_millis() };
        if matches!(endpoint, "ComicDetail" | "ClassPage" | "GetImageIndex" | "ImageToken") {
            self.ensure_signing_asset().await?;
            let crypto = self.crypto.clone(); let mut signing_context = context.clone();
            let account_state = self.account_epoch.clone();
            let signature = super::work::run_blocking(super::work::HEAVY_WORK_BYTES, move |cancel| {
                let obsolete = || cancel.is_cancelled() || account_state.load(Ordering::SeqCst) != account_epoch;
                if obsolete() { return Err(SourceError::new(ErrorKind::Cancelled, "account_changed")); }
                signing_context.timestamp_ms = now_millis();
                crypto.sign_cancellable(&signing_context, &obsolete)
            }).await.map_err(|error| super::work::source_error(error, "signing_worker_failed"))??;
            if signature.ultra_sign.is_empty() || signature.data_sn.is_empty() {
                return Err(SourceError::protocol("signing_result_invalid"));
            }
            url.query_pairs_mut().append_pair("ultra_sign", &signature.ultra_sign);
            headers.push(("x-bili-data-sn".into(), signature.data_sn));
        }
        context.url = url.to_string();
        if self.session_generation() != epoch && options.guest_device.is_none() && !options.anonymous {
            return Err(SourceError { transmitted: Some(false), definitive: true,
                ..SourceError::new(ErrorKind::Cancelled, "account_changed") });
        }
        let received = self.send(HttpRequest { method: "POST".into(), url: context.url.clone(), headers,
            body: encoded.into_bytes(), max_bytes: MAX_JSON_BYTES }).await;
        let response = match received {
            Ok(response) => response,
            Err(error) => return Err(transaction_error(endpoint, error)),
        };
        if self.account_epoch.load(Ordering::SeqCst) != account_epoch && !options.anonymous && options.guest_device.is_none()
            && !matches!(endpoint, "BuyEpisode" | "CreateOrder" | "AddHistory" | "AddFavorite" | "DeleteFavorite") {
            return Err(SourceError { transmitted: Some(true), definitive: true,
                ..SourceError::new(ErrorKind::Cancelled, "account_changed") });
        }
        // Set-Cookie belongs to the HTTP response, including rejected business
        // envelopes. Preserve its result separately so a completed mutation is
        // never turned into a retryable submission by a cookie-storage failure.
        let cookie_result = match session.as_ref() {
            Some(original) => self.apply_http_response_cookies(original, &response,
                "manga.bilibili.com", &context.path, Some(account_epoch), None),
            None => Ok(()),
        };
        if let Err(error) = check_status(&response) {
            return Err(transaction_error(endpoint, self.response_error(session.as_ref(), account_epoch, error)));
        }
        let decoded = if endpoint == "GetPayOrders" { super::payments::decode_recharge_envelope(&response.body) }
            else { serde_json::from_slice(&response.body).map_err(|_| SourceError::protocol("response_json_invalid")) };
        let mut envelope: Value = match decoded {
            Ok(value) => value, Err(error) => return Err(transaction_error(endpoint, error)),
        };
        let code = number_code(envelope.get("code")).ok_or_else(||
            transaction_error(endpoint, SourceError::protocol("response_business_code_missing")))?;
        if code != 0 {
            return Err(self.response_error(session.as_ref(), account_epoch, SourceError::business(endpoint, code)));
        }
        if envelope.get("bytesData").is_some_and(|value| !value.is_null() && value.as_str() != Some("")) {
            let crypto = self.crypto.clone();
            let decoded = super::work::run_blocking(super::work::HEAVY_WORK_BYTES, move |cancel| {
                crypto.decode_response_cancellable(&context, envelope, &|| cancel.is_cancelled())
            }).await.map_err(|error| super::work::source_error(error, "decoding_worker_failed"))
                .and_then(|result| result);
            envelope = match decoded {
                Ok(value) => value,
                Err(mut error) => {
                    error.transmitted = Some(true);
                    return Err(transaction_error(endpoint, error));
                }
            };
        }
        let code = number_code(envelope.get("code")).ok_or_else(||
            transaction_error(endpoint, SourceError::protocol("response_business_code_missing")))?;
        if code != 0 {
            return Err(self.response_error(session.as_ref(), account_epoch, SourceError::business(endpoint, code)));
        }
        let accepted = matches!(endpoint, "BuyEpisode" | "AddHistory" | "AddFavorite" | "DeleteFavorite");
        let data = match envelope.get("data") {
            Some(data) if data.is_object() || data.is_array() => data.clone(),
            _ if accepted => serde_json::json!({"accepted": true, "code": 0}),
            _ => return Err(transaction_error(endpoint, SourceError::protocol("response_data_missing"))),
        };
        if !matches!(endpoint, "BuyEpisode" | "AddFavorite" | "DeleteFavorite" | "CreateOrder" | "AddHistory") {
            cookie_result?;
        }
        Ok(data)
    }

    async fn ensure_signing_asset(&self) -> Result<()> {
        if !self.manage_assets { return Ok(()); }
        let _guard = self.asset_lock.lock().await;
        let path = self.asset_root.join(SIGNING_FILE);
        let crypto = self.crypto.clone();
        let ready = super::work::run_blocking(64 * 1024 * 1024, move |cancel| {
            crypto.signing_asset_ready_cancellable(&|| cancel.is_cancelled())
        }).await.map_err(|error| super::work::source_error(error, "signing_asset_worker_failed"))??;
        if ready { return Ok(()); }
        let response = self.send(HttpRequest { method: "GET".into(), url: SIGNING_URL.into(),
            headers: vec![("user-agent".into(), USER_AGENT.into())], body: Vec::new(), max_bytes: 16 * 1024 * 1024 }).await?;
        check_status(&response)?;
        let temporary = self.asset_root.join(format!(".signing-{}-{}.pending", std::process::id(), NEXT_ASSET.fetch_add(1, Ordering::Relaxed)));
        let bytes = response.body; let destination = path;
        let crypto = self.crypto.clone();
        super::work::run_blocking(64 * 1024 * 1024, move |cancel| -> Result<()> {
            use std::io::Write;
            struct Temporary(PathBuf);
            impl Drop for Temporary { fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); } }
            if cancel.is_cancelled() { return Err(SourceError::new(ErrorKind::Cancelled, "source_work_cancelled")); }
            if format!("{:x}", Sha256::digest(&bytes)) != SIGNING_SHA256 {
                return Err(SourceError::capability("signing_asset_integrity_failed"));
            }
            if cancel.is_cancelled() { return Err(SourceError::new(ErrorKind::Cancelled, "source_work_cancelled")); }
            let directory = destination.parent().ok_or_else(|| SourceError::storage("signing_asset_directory_failed"))?;
            std::fs::create_dir_all(directory).map_err(|_| SourceError::storage("signing_asset_directory_failed"))?;
            let _temporary = Temporary(temporary.clone());
            let mut options = std::fs::OpenOptions::new(); options.write(true).create_new(true);
            #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
            let result = (|| {
                let mut file = options.open(&temporary).map_err(|_| SourceError::storage("signing_asset_write_failed"))?;
                file.write_all(&bytes).and_then(|_| file.sync_all()).map_err(|_| SourceError::storage("signing_asset_write_failed"))?;
                drop(file);
                if cancel.is_cancelled() { return Err(SourceError::new(ErrorKind::Cancelled, "source_work_cancelled")); }
                std::fs::rename(&temporary, &destination).map_err(|_| SourceError::storage("signing_asset_publish_failed"))?;
                if !crypto.signing_asset_ready_cancellable(&|| cancel.is_cancelled())? {
                    return Err(SourceError::capability("signing_asset_missing"));
                }
                Ok(())
            })();
            if result.is_err() { let _ = std::fs::remove_file(&temporary); }
            result
        }).await.map_err(|error| super::work::source_error(error, "signing_asset_worker_failed"))??;
        Ok(())
    }
}

pub(super) fn now_seconds() -> i64 { chrono::Utc::now().timestamp() }
pub(super) fn now_millis() -> i64 { chrono::Utc::now().timestamp_millis() }

pub(super) fn numeric_id(value: &str) -> Result<u64> {
    if value.is_empty() || value.len() > 15 || value.starts_with('0') || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(SourceError::invalid("invalid_identifier"));
    }
    value.parse().map_err(|_| SourceError::invalid("invalid_identifier"))
}

pub(super) fn number_code(value: Option<&Value>) -> Option<i64> {
    let value = value?; value.as_i64().or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

fn account_namespace(value: Option<&AuthSession>) -> Option<&str> {
    value.and_then(AuthSession::account_key)
}

fn bound_account_mismatch() -> SourceError {
    SourceError { transmitted: Some(false), definitive: true,
        ..SourceError::new(ErrorKind::AccountMismatch, "bound_account_mismatch") }
}

fn percent_decode(value: &str) -> Result<String> {
    let mut result = Vec::new(); let bytes = value.as_bytes(); let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let pair = bytes.get(index + 1..index + 3).ok_or_else(|| SourceError::invalid("invalid_xsrf_cookie"))?;
            let text = std::str::from_utf8(pair).map_err(|_| SourceError::invalid("invalid_xsrf_cookie"))?;
            result.push(u8::from_str_radix(text, 16).map_err(|_| SourceError::invalid("invalid_xsrf_cookie"))?); index += 3;
        } else { result.push(bytes[index]); index += 1; }
    }
    if result.iter().any(|byte| matches!(byte, 0 | b'\r' | b'\n')) { return Err(SourceError::invalid("invalid_xsrf_cookie")); }
    String::from_utf8(result).map_err(|_| SourceError::invalid("invalid_xsrf_cookie"))
}

fn transaction_error(endpoint: &str, error: SourceError) -> SourceError {
    if matches!(endpoint, "BuyEpisode" | "CreateOrder") && error.transmitted != Some(false) {
        SourceError { status: error.status, transmitted: error.transmitted.or(Some(true)), definitive: false,
            ..SourceError::new(ErrorKind::PurchaseUnknown, if endpoint == "CreateOrder" { "recharge_outcome_unknown" } else { "purchase_outcome_unknown" }) }
    } else { error }
}
