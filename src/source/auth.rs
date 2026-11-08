//! Account exchanges keep unconfirmed credentials separate from the active session.

use super::{AccountIdentity, AuthSession, BiliClient, ErrorKind, Result, SourceError};
use super::transport::{HttpRequest, HttpResponse, check_status};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{io::Read, sync::{OnceLock, atomic::Ordering}, time::{SystemTime, UNIX_EPOCH}};
use url::Url;

const PASSPORT: &str = "passport.bilibili.com";
const API: &str = "api.bilibili.com";
const WWW: &str = "www.bilibili.com";
const MANGA: &str = "manga.bilibili.com";
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux) AppleWebKit/537.36 Chrome/130.0.0.0 Safari/537.36";
const MAX_EXACT_INTEGER: u64 = 9_007_199_254_740_991;
static RENEWAL_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

#[derive(Clone)]
pub struct QrLoginChallenge {
    pub url: String,
    pub key: String,
}

impl std::fmt::Debug for QrLoginChallenge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("QrLoginChallenge").field("ready", &true).finish()
    }
}

#[derive(Clone, Debug)]
pub enum QrLoginState {
    Waiting,
    Scanned,
    Expired,
    Confirmed(AuthSession),
}

#[derive(Clone, Debug)]
pub struct RenewalInfo {
    pub refresh: bool,
    pub timestamp: i64,
    pub session: AuthSession,
}

impl AuthSession {
    pub fn needs_refresh_confirmation(&self) -> bool { self.pending_refresh_token.is_some() }

    pub fn renewal_is_uncertain(&self) -> bool { self.refresh_blocked || self.confirmation_blocked }
}

#[derive(Clone)]
struct CredentialContext {
    account: Option<String>,
    epoch: u64,
}

#[derive(Debug)]
struct PreparedSession {
    candidate: AuthSession,
    expected_epoch: u64,
    mutation_completed: bool,
}

impl PreparedSession {
    fn activate(self, client: &BiliClient) -> Result<AuthSession> {
        if !client.replace_session_if_epoch(self.expected_epoch, Some(self.candidate.clone()))? {
            return Err(if self.mutation_completed { renewal_unknown() } else { credential_cancelled() });
        }
        Ok(self.candidate)
    }
}

impl BiliClient {
    pub fn cancel_login(&self) {
        let mut key = self.login_key.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.login_epoch.fetch_add(1, Ordering::SeqCst);
        *key = None;
    }

    pub async fn begin_qr(&self) -> Result<QrLoginChallenge> {
        self.cancel_login();
        let login_epoch = self.login_epoch.load(Ordering::SeqCst);
        let epoch = self.session_generation();
        let path = format!("/x/passport-login/web/qrcode/generate?{}", form(&[
            ("source", "main_web"), ("go_url", "https://manga.bilibili.com/"),
        ]));
        let (data, _) = auth_json(self, PASSPORT, &path, None, None, false, false).await?;
        require_epoch(self, epoch)?;
        require_login_epoch(self, login_epoch)?;
        let key = data.get("qrcode_key").and_then(Value::as_str)
            .filter(|value| valid_key(value)).ok_or_else(|| SourceError::protocol("invalid_qr_key"))?;
        let url = data.get("url").and_then(Value::as_str)
            .filter(|value| valid_qr_url(value)).ok_or_else(|| SourceError::protocol("invalid_qr_url"))?;
        let mut active_key = self.login_key.lock().map_err(|_| SourceError::protocol("login_lock_failed"))?;
        require_login_epoch(self, login_epoch)?;
        *active_key = Some(key.into());
        Ok(QrLoginChallenge { url: url.into(), key: key.into() })
    }

    /// Returns an isolated candidate. The application saves it before activating it.
    pub async fn poll_qr(&self, key: &str) -> Result<QrLoginState> {
        if !valid_key(key) { return Err(SourceError::invalid("invalid_qr_key")); }
        let login_epoch = {
            let active_key = self.login_key.lock().map_err(|_| SourceError::protocol("login_lock_failed"))?;
            if active_key.as_deref() != Some(key) { return Err(cancelled()); }
            self.login_epoch.load(Ordering::SeqCst)
        };
        let epoch = self.session_generation();
        let path = format!("/x/passport-login/web/qrcode/poll?{}", form(&[
            ("qrcode_key", key), ("source", "main_web"),
        ]));
        let (data, response) = auth_json(self, PASSPORT, &path, None, None, false, false).await?;
        require_epoch(self, epoch)?;
        require_login_epoch(self, login_epoch)?;
        match integer(data.get("code")) {
            Some(86101) => return Ok(QrLoginState::Waiting),
            Some(86090) => return Ok(QrLoginState::Scanned),
            Some(86038) => return Ok(QrLoginState::Expired),
            Some(0) => {},
            Some(code) => return Err(SourceError { business_code: Some(code), transmitted: Some(true),
                definitive: true, ..SourceError::authentication("qr_login_rejected") }),
            None => return Err(SourceError::protocol("invalid_qr_poll_status")),
        }
        let refresh_token = data.get("refresh_token").and_then(Value::as_str)
            .filter(|value| valid_opaque(value, 4096))
            .ok_or_else(|| SourceError::protocol("qr_renewal_credential_missing"))?;
        let mut candidate = AuthSession::default();
        candidate.imported_at = now();
        candidate.refresh_token = Some(refresh_token.to_owned());
        candidate.apply_set_cookie_for_path(&response.headers, PASSPORT, &path, now())?;
        if !required_credentials(&candidate, PASSPORT) {
            return Err(SourceError::protocol("qr_session_cookies_missing"));
        }
        candidate = ensure_candidate_site(self, candidate).await?;
        require_epoch(self, epoch)?;
        require_login_epoch(self, login_epoch)?;
        let validation_client = self.isolated_auth_client();
        candidate = validate_candidate(&validation_client, candidate, None).await?;
        require_epoch(self, epoch)?;
        require_login_epoch(self, login_epoch)?;
        if !candidate.has_device() { return Err(SourceError::protocol("manga_device_context_missing")); }
        Ok(QrLoginState::Confirmed(candidate))
    }

    pub async fn validate_session(&self) -> Result<AccountIdentity> {
        let (epoch, session) = self.session_snapshot();
        let session = session.ok_or_else(|| SourceError::authentication("session_missing"))?;
        let expected = session.account_key().map(str::to_owned);
        let candidate = validate_candidate(self, session, expected.as_deref()).await?;
        let identity = candidate.identity.clone().ok_or_else(|| SourceError::protocol("account_identity_missing"))?;
        if !self.replace_session_if_epoch(epoch, Some(candidate))? { return Err(cancelled()); }
        Ok(identity)
    }

    pub async fn ensure_site_context(&self) -> Result<AuthSession> {
        let (epoch, session) = self.session_snapshot();
        let session = session.ok_or_else(|| SourceError::authentication("session_missing"))?;
        if session.account_key().is_none() {
            return Err(SourceError::authentication("verified_session_required"));
        }
        let candidate = ensure_candidate_site(self, session).await?;
        if !self.replace_session_if_epoch(epoch, Some(candidate.clone()))? { return Err(cancelled()); }
        Ok(candidate)
    }

    pub async fn cookie_info(&self) -> Result<RenewalInfo> {
        let (epoch, session) = self.session_snapshot();
        let session = session.ok_or_else(|| SourceError::authentication("session_missing"))?;
        let mut context = CredentialContext { account: session.account_key().map(str::to_owned), epoch };
        cookie_info_bound(self, session, &mut context).await
    }

    /// Checks one captured session and invalidates only the credentials rejected by that check.
    pub async fn check_session_and_invalidate(&self) -> Result<RenewalInfo> {
        let (epoch, session) = self.session_snapshot();
        let session = session.ok_or_else(|| SourceError::authentication("session_missing"))?;
        self.require_bound_account(Some(&session))?;
        let mut context = CredentialContext { account: session.account_key().map(str::to_owned), epoch };
        let result = async {
            require_context(self, &context)?;
            if session.account_key().is_none() {
                return Err(SourceError::authentication("verified_session_required"));
            }
            let candidate = ensure_candidate_site(self, session).await?;
            require_context(self, &context)?;
            commit_context_session(self, &mut context, candidate.clone())?;
            let candidate = validate_candidate_bound(self, candidate, context.account.as_deref(), Some(&context)).await?;
            commit_context_session(self, &mut context, candidate.clone())?;
            cookie_info_bound(self, candidate, &mut context).await
        }.await;
        if let Err(error) = &result {
            if matches!(error.kind, ErrorKind::Authentication | ErrorKind::AccountMismatch)
                && !self.clear_session_if_epoch(context.epoch)? {
                let mut cancelled = credential_cancelled();
                cancelled.transmitted = error.transmitted;
                return Err(cancelled);
            }
        }
        result
    }

    /// An explicit renewal exchange. The caller owns its durable attempt journal.
    /// Save the returned candidate before activating it or calling confirm_refresh.
    /// The account/version snapshot is captured when this method is called, not when first polled.
    pub fn refresh_session(&self) -> impl std::future::Future<Output = Result<AuthSession>> + Send + 'static {
        let client = self.clone();
        let context = self.credential_context();
        async move {
            let (account, epoch) = context.ok_or_else(|| SourceError::authentication("verified_session_required"))?;
            client.refresh_session_for_session(&account, epoch).await
        }
    }

    /// Binds work to the credentials captured by the caller before it was queued.
    pub async fn refresh_session_for_session(&self, expected_account: &str, expected_epoch: u64) -> Result<AuthSession> {
        Ok(self.prepare_refresh_for_session(expected_account, expected_epoch).await?.candidate)
    }

    /// Durably activates the candidate only while its exact source version is still current.
    pub async fn refresh_and_activate_for_session(&self, expected_account: &str, expected_epoch: u64) -> Result<AuthSession> {
        self.prepare_refresh_for_session(expected_account, expected_epoch).await?.activate(self)
    }

    async fn prepare_refresh_for_session(&self, expected_account: &str, expected_epoch: u64) -> Result<PreparedSession> {
        let _guard = RENEWAL_LOCK.get_or_init(|| tokio::sync::Mutex::new(())).lock().await;
        let mut context = CredentialContext { account: Some(expected_account.into()), epoch: expected_epoch };
        let initial = context_session(self, &context)?;
        if initial.refresh_blocked { return Err(SourceError::capability("renewal_outcome_unresolved")); }
        require_credentials(&initial, PASSPORT)?;
        if initial.pending_refresh_token.is_some() { return Err(SourceError::invalid("renewal_confirmation_pending")); }
        let old_token = initial.refresh_token.as_deref().filter(|value| valid_opaque(value, 16384))
            .ok_or_else(|| SourceError::capability("renewal_credential_missing"))?.to_owned();
        let info = cookie_info_bound(self, initial.clone(), &mut context).await?;
        let mut session = info.session;
        require_context(self, &context)?;
        if session.refresh_token.as_deref() != Some(old_token.as_str()) || session.account_key != initial.account_key {
            return Err(credential_cancelled());
        }
        if !info.refresh {
            let mut candidate = validate_candidate_bound(self, session, initial.account_key(), Some(&context)).await?;
            require_context(self, &context)?;
            candidate.refresh_checked_at = Some(now());
            candidate.refresh_blocked = false;
            return Ok(PreparedSession { candidate, expected_epoch: context.epoch, mutation_completed: false });
        }
        let timestamp = info.timestamp;
        let ciphertext = super::work::run_blocking(super::work::METADATA_WORK_BYTES, move |cancel| {
            if cancel.is_cancelled() { return Err(cancelled()); }
            production_correspondence_path(timestamp)
        }).await.map_err(|error| super::work::source_error(error, "renewal_crypto_worker_failed"))??;
        let path = format!("/correspond/1/{ciphertext}");
        let response = auth_request_bound(self, WWW, &path, Some(&session), None, true, false, Some(&context)).await?;
        require_context(self, &context)?;
        let refresh_csrf = std::str::from_utf8(&response.body)
            .map_err(|_| SourceError::protocol("invalid_renewal_challenge"))
            .and_then(|body| extract_refresh_csrf(body)
                .ok_or_else(|| SourceError::protocol("invalid_renewal_challenge")))
            .map_err(|error| auth_error_response(self, Some(&session), Some(&context), WWW, &path, &response, error))?;
        let mut challenged = session.clone();
        challenged.apply_set_cookie_for_path(&response.headers, WWW, &path, now())?;
        require_credentials(&challenged, PASSPORT)?;
        if !challenged.same_credentials(&session) {
            challenged = validate_candidate_bound(self, challenged, session.account_key(), Some(&context)).await?;
        }
        if challenged != session {
            commit_context_session(self, &mut context, challenged.clone())?;
            session = challenged;
        }
        let csrf = session.cookie_value_for(PASSPORT, "bili_jct")
            .ok_or_else(|| SourceError::authentication("session_cookies_missing"))?;
        let body = form(&[("csrf", csrf), ("refresh_csrf", &refresh_csrf),
            ("source", "main_web"), ("refresh_token", &old_token)]);
        let attempt_marker = mark_renewal_attempt(self, &mut context, &session, false)?;
        let result = auth_json_bound(self, PASSPORT, "/x/passport-login/web/cookie/refresh",
            Some(&session), Some(body), false, true, Some(&context)).await;
        let (data, response) = match result {
            Ok(value) => value,
            Err(error) => {
                return Err(renewal_failure(self, &context, &session, attempt_marker, false, error));
            }
        };
        require_context(self, &context).map_err(|_| renewal_unknown())?;
        let candidate = (|| {
            let token = data.get("refresh_token").and_then(Value::as_str)
                .filter(|value| valid_opaque(value, 16384) && *value != old_token)
                .ok_or_else(renewal_unknown)?;
            let mut candidate = session.clone();
            candidate.apply_set_cookie_for_path(&response.headers, PASSPORT,
                "/x/passport-login/web/cookie/refresh", now()).map_err(|_| renewal_unknown())?;
            let fresh_cookies = new_session_cookies(&response.headers).map_err(|_| renewal_unknown())?;
            if !required_credentials(&candidate, PASSPORT) || !fresh_cookies { return Err(renewal_unknown()); }
            candidate.pending_refresh_token = Some(old_token);
            candidate.refresh_token = Some(token.into());
            candidate.refresh_checked_at = Some(now());
            candidate.last_refreshed_at = Some(now());
            candidate.refresh_blocked = false;
            Ok(candidate)
        })().map_err(|error| auth_error_response(self, Some(&session), Some(&context), PASSPORT,
            "/x/passport-login/web/cookie/refresh", &response, error))?;
        let candidate = validate_candidate_bound(self, candidate, session.account_key(), Some(&context)).await.map_err(|error| {
            if error.kind == ErrorKind::AccountMismatch { error } else { renewal_unknown() }
        })?;
        require_context(self, &context).map_err(|_| renewal_unknown())?;
        Ok(PreparedSession { candidate, expected_epoch: context.epoch, mutation_completed: true })
    }

    /// The caller must durably save and activate the replacement before this call.
    /// A successful result still needs to be saved to clear the pending old token.
    /// The snapshot is captured immediately, even when the returned future is queued.
    pub fn confirm_refresh(&self) -> impl std::future::Future<Output = Result<AuthSession>> + Send + 'static {
        let client = self.clone();
        let context = self.credential_context();
        async move {
            let (account, epoch) = context.ok_or_else(|| SourceError::authentication("verified_session_required"))?;
            client.confirm_refresh_for_session(&account, epoch).await
        }
    }

    pub async fn confirm_refresh_for_session(&self, expected_account: &str, expected_epoch: u64) -> Result<AuthSession> {
        Ok(self.prepare_confirmation_for_session(expected_account, expected_epoch).await?.candidate)
    }

    /// Durably activates confirmation results without allowing an older UI callback to replace cookies.
    pub async fn confirm_and_activate_for_session(&self, expected_account: &str, expected_epoch: u64) -> Result<AuthSession> {
        self.prepare_confirmation_for_session(expected_account, expected_epoch).await?.activate(self)
    }

    async fn prepare_confirmation_for_session(&self, expected_account: &str, expected_epoch: u64) -> Result<PreparedSession> {
        let _guard = RENEWAL_LOCK.get_or_init(|| tokio::sync::Mutex::new(())).lock().await;
        let mut context = CredentialContext { account: Some(expected_account.into()), epoch: expected_epoch };
        let session = context_session(self, &context)?;
        if session.confirmation_blocked { return Err(SourceError::capability("renewal_confirmation_unresolved")); }
        require_credentials(&session, PASSPORT)?;
        let token = session.pending_refresh_token.as_deref().filter(|value| valid_opaque(value, 16384))
            .ok_or_else(|| SourceError::invalid("renewal_confirmation_missing"))?;
        let csrf = session.cookie_value_for(PASSPORT, "bili_jct")
            .ok_or_else(|| SourceError::authentication("session_cookies_missing"))?;
        let body = form(&[("csrf", csrf), ("refresh_token", token)]);
        let attempt_marker = mark_renewal_attempt(self, &mut context, &session, true)?;
        let (_, response) = match auth_json_bound(self, PASSPORT, "/x/passport-login/web/confirm/refresh",
            Some(&session), Some(body), true, true, Some(&context)).await {
            Ok(value) => value,
            Err(error) => {
                return Err(renewal_failure(self, &context, &session, attempt_marker, true, error));
            }
        };
        require_context(self, &context).map_err(|_| renewal_unknown())?;
        let mut candidate = (|| {
            let mut candidate = session.clone();
            candidate.apply_set_cookie_for_path(&response.headers, PASSPORT,
                "/x/passport-login/web/confirm/refresh", now()).map_err(|_| renewal_unknown())?;
            if !required_credentials(&candidate, PASSPORT) { return Err(renewal_unknown()); }
            Ok(candidate)
        })().map_err(|error| auth_error_response(self, Some(&session), Some(&context), PASSPORT,
            "/x/passport-login/web/confirm/refresh", &response, error))?;
        if !candidate.same_credentials(&session) {
            candidate = validate_candidate_bound(self, candidate, session.account_key(), Some(&context)).await.map_err(|error| {
                if error.kind == ErrorKind::AccountMismatch { error } else { renewal_unknown() }
            })?;
        }
        require_context(self, &context).map_err(|_| renewal_unknown())?;
        candidate.pending_refresh_token = None;
        candidate.confirmation_blocked = false;
        Ok(PreparedSession { candidate, expected_epoch: context.epoch, mutation_completed: true })
    }
}

async fn ensure_candidate_site(client: &BiliClient, mut session: AuthSession) -> Result<AuthSession> {
    if session.has_device() { return Ok(session); }
    let response = client.send(HttpRequest {
        method: "GET".into(), url: "https://manga.bilibili.com/ductape/buvid".into(),
        headers: vec![("user-agent".into(), USER_AGENT.into()), ("accept".into(), "application/json".into()),
            ("referer".into(), "https://manga.bilibili.com/".into())], body: Vec::new(), max_bytes: 65536,
    }).await?;
    check_status(&response)?;
    if response.status != 200 { return Err(SourceError::protocol("manga_device_context_missing")); }
    let records = super::session::set_cookie_records(&response.headers)?;
    let mut selected = Vec::new();
    let mut observed = None;
    for record in records {
        let Some((name, value)) = record.split(';').next().and_then(|pair| pair.trim().split_once('=')) else { continue; };
        if name.trim() != "buvid3" { continue; }
        let value = value.trim();
        if value.is_empty() || value.len() > 256
            || !value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            || observed.as_deref().is_some_and(|previous| previous != value) {
            return Err(SourceError::protocol("manga_device_context_invalid"));
        }
        observed = Some(value.to_owned());
        selected.push(("set-cookie".into(), record));
    }
    let mut scratch = AuthSession::default();
    scratch.apply_set_cookie_for_path(&selected, MANGA, "/ductape/buvid", now())?;
    if !scratch.has_device() { return Err(SourceError::protocol("manga_device_context_missing")); }
    session.copy_cookie_from(&scratch, "buvid3")?;
    Ok(session)
}

async fn validate_candidate(client: &BiliClient, session: AuthSession, expected: Option<&str>) -> Result<AuthSession> {
    validate_candidate_bound(client, session, expected, None).await
}

async fn validate_candidate_bound(client: &BiliClient, mut session: AuthSession, expected: Option<&str>,
    context: Option<&CredentialContext>) -> Result<AuthSession> {
    require_credentials(&session, API)?;
    let (data, response) = auth_json_bound(client, API, "/x/web-interface/nav", Some(&session), None, false, false, context).await?;
    if let Some(context) = context { require_context(client, context)?; }
    let original = session.clone();
    let validated = (|| {
        if data.get("isLogin").and_then(Value::as_bool) != Some(true) {
            return Err(SourceError::authentication("session_rejected"));
        }
        session.apply_set_cookie_for_path(&response.headers, API, "/x/web-interface/nav", now())?;
        require_credentials(&session, API)?;
        let id = identity_id(data.get("mid")).ok_or_else(|| SourceError::protocol("invalid_account_identity"))?;
        let name = data.get("uname").or_else(|| data.get("name")).and_then(Value::as_str).unwrap_or_default();
        if name.len() > 4096 || name.chars().any(|value| value == '\0') {
            return Err(SourceError::protocol("invalid_account_identity"));
        }
        let avatar_url = data.get("face").and_then(Value::as_str).filter(|value| value.len() <= 8192)
            .unwrap_or_default().to_owned();
        let candidate = session.with_identity(AccountIdentity { id, name: name.into(), avatar_url }, now())?;
        if expected.is_some_and(|expected| candidate.account_key != expected) {
            return Err(SourceError::new(ErrorKind::AccountMismatch, "account_identity_changed"));
        }
        Ok(candidate)
    })();
    validated.map_err(|error| auth_error_response(client, Some(&original), context, API, "/x/web-interface/nav", &response, error))
}

async fn auth_request_bound(client: &BiliClient, host: &str, path: &str, session: Option<&AuthSession>,
    body: Option<String>, html: bool, mutation: bool, context: Option<&CredentialContext>) -> Result<HttpResponse> {
    if !matches!(host, PASSPORT | API | WWW) || !path.starts_with('/') || path.starts_with("//")
        || path.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(SourceError::invalid("invalid_authentication_endpoint"));
    }
    if let Some(session) = session {
        client.require_bound_account(Some(session))?;
        require_credentials(session, host)?;
    }
    let mut headers = vec![("user-agent".into(), USER_AGENT.into()),
        ("accept".into(), if html { "text/html" } else { "application/json, text/plain, */*" }.into()),
        ("referer".into(), "https://www.bilibili.com/".into()), ("origin".into(), "https://www.bilibili.com".into())];
    if let Some(session) = session {
        let cookie = session.cookie_header(host)?;
        if !cookie.is_empty() { headers.push(("cookie".into(), cookie)); }
    }
    if body.is_some() { headers.push(("content-type".into(), "application/x-www-form-urlencoded".into())); }
    let request = HttpRequest { method: if body.is_some() { "POST" } else { "GET" }.into(),
        url: format!("https://{host}{path}"), headers, body: body.unwrap_or_default().into_bytes(),
        max_bytes: if html { 1024 * 1024 } else { 256 * 1024 } };
    // The request retains its captured credentials; it never rebinds to a later active account.
    if let Some(context) = context { require_context(client, context)?; }
    let response = client.send(request).await.map_err(|mut error| {
        error.retryable = false;
        if mutation && error.transmitted != Some(false) { renewal_unknown() } else { error }
    })?;
    if let Err(mut error) = check_status(&response) {
        error.retryable = false;
        let error = if mutation && response.status >= 500 { renewal_unknown() } else { error };
        return Err(auth_error_response(client, session, context, host, path, &response, error));
    }
    Ok(response)
}

async fn auth_json(client: &BiliClient, host: &str, path: &str, session: Option<&AuthSession>,
    body: Option<String>, empty: bool, mutation: bool) -> Result<(Value, HttpResponse)> {
    auth_json_bound(client, host, path, session, body, empty, mutation, None).await
}

async fn auth_json_bound(client: &BiliClient, host: &str, path: &str, session: Option<&AuthSession>,
    body: Option<String>, empty: bool, mutation: bool, context: Option<&CredentialContext>) -> Result<(Value, HttpResponse)> {
    let response = auth_request_bound(client, host, path, session, body, false, mutation, context).await?;
    let decoded = (|| {
        let envelope: Value = serde_json::from_slice(&response.body)
            .map_err(|_| if mutation { renewal_unknown() } else { SourceError::protocol("invalid_authentication_response") })?;
        let code = integer(envelope.get("code"))
            .ok_or_else(|| if mutation { renewal_unknown() } else { SourceError::protocol("invalid_authentication_status") })?;
        if code != 0 {
            let mut error = SourceError::business("Authentication", code);
            error.retryable = false;
            if mutation { error.code = "renewal_rejected".into(); }
            return Err(error);
        }
        match envelope.get("data") {
            Some(value) if value.is_object() => Ok(value.clone()),
            _ if empty => Ok(Value::Object(Default::default())),
            _ => Err(if mutation { renewal_unknown() } else { SourceError::protocol("invalid_authentication_data") }),
        }
    })();
    let data = decoded.map_err(|error| auth_error_response(client, session, context, host, path, &response, error))?;
    Ok((data, response))
}

fn auth_error_response(client: &BiliClient, session: Option<&AuthSession>, context: Option<&CredentialContext>,
    host: &str, path: &str, response: &HttpResponse, error: SourceError) -> SourceError {
    let (Some(original), Some(context)) = (session, context) else { return error; };
    if matches!(error.kind, ErrorKind::Authentication | ErrorKind::AccountMismatch) {
        // A rejected identity cannot publish unverified credentials or advance
        // the request version before the caller conditionally invalidates it.
        return error;
    }
    if let Err(storage) = client.apply_auth_error_cookies(original, response, host, path, context.epoch) {
        if storage.kind == ErrorKind::Storage { return storage; }
    }
    error
}

async fn cookie_info_bound(client: &BiliClient, session: AuthSession,
    context: &mut CredentialContext) -> Result<RenewalInfo> {
    require_credentials(&session, PASSPORT)?;
    let path = format!("/x/passport-login/web/cookie/info?{}", form(&[
        ("csrf", session.cookie_value_for(PASSPORT, "bili_jct")
            .ok_or_else(|| SourceError::authentication("session_cookies_missing"))?),
    ]));
    let (data, response) = auth_json_bound(client, PASSPORT, &path, Some(&session), None, false, false, Some(context)).await?;
    require_context(client, context)?;
    let (refresh, timestamp) = (|| {
        let refresh = data.get("refresh").and_then(Value::as_bool)
            .ok_or_else(|| SourceError::protocol("invalid_renewal_state"))?;
        let timestamp = positive_timestamp(data.get("timestamp"))
            .ok_or_else(|| SourceError::protocol("invalid_renewal_timestamp"))?;
        Ok((refresh, timestamp))
    })().map_err(|error| auth_error_response(client, Some(&session), Some(context), PASSPORT, &path, &response, error))?;
    let mut candidate = session.clone();
    candidate.apply_set_cookie_for_path(&response.headers, PASSPORT, &path, now())?;
    if !candidate.same_credentials(&session) {
        candidate = validate_candidate_bound(client, candidate, session.account_key(), Some(context)).await?;
    }
    candidate.refresh_checked_at = Some(now());
    commit_context_session(client, context, candidate.clone())?;
    Ok(RenewalInfo { refresh, timestamp, session: candidate })
}

fn context_session(client: &BiliClient, context: &CredentialContext) -> Result<AuthSession> {
    let (epoch, session) = client.session_snapshot();
    let session = session.ok_or_else(credential_cancelled)?;
    client.require_bound_account(Some(&session))?;
    if epoch != context.epoch || session.account_key() != context.account.as_deref() {
        return Err(credential_cancelled());
    }
    Ok(session)
}

fn require_context(client: &BiliClient, context: &CredentialContext) -> Result<()> {
    context_session(client, context).map(|_| ())
}

fn commit_context_session(client: &BiliClient, context: &mut CredentialContext, candidate: AuthSession) -> Result<()> {
    let account = candidate.account_key().map(str::to_owned);
    if context.account.is_some() && account != context.account {
        return Err(SourceError::new(ErrorKind::AccountMismatch, "account_identity_changed"));
    }
    if !client.replace_session_if_epoch(context.epoch, Some(candidate))? { return Err(credential_cancelled()); }
    // Only this successful compare-and-replace is allowed to advance the operation's context.
    context.epoch = context.epoch.wrapping_add(1);
    context.account = account;
    require_context(client, context)
}

fn require_credentials(session: &AuthSession, host: &str) -> Result<()> {
    if required_credentials(session, host) { Ok(()) } else {
        Err(SourceError { transmitted: Some(false), definitive: true,
            ..SourceError::authentication("session_cookies_missing") })
    }
}

fn required_credentials(session: &AuthSession, host: &str) -> bool {
    session.has_session_for(host) && (host != PASSPORT || session.cookie_value_for(PASSPORT, "bili_jct").is_some())
}

fn new_session_cookies(headers: &[(String, String)]) -> Result<bool> {
    let mut candidate = AuthSession::default();
    candidate.apply_set_cookie_for_path(headers, PASSPORT, "/x/passport-login/web/cookie/refresh", now())?;
    Ok(required_credentials(&candidate, PASSPORT))
}

fn mark_renewal_attempt(client: &BiliClient, context: &mut CredentialContext, session: &AuthSession, confirmation: bool) -> Result<u64> {
    let mut candidate = session.clone();
    if confirmation { candidate.confirmation_blocked = true; } else { candidate.refresh_blocked = true; }
    let (epoch, marker) = client.mark_renewal_if_epoch(context.epoch, candidate)?.ok_or_else(credential_cancelled)?;
    context.epoch = epoch;
    Ok(marker)
}

fn renewal_failure(client: &BiliClient, context: &CredentialContext, session: &AuthSession,
    marker: u64, confirmation: bool, error: SourceError) -> SourceError {
    if error.kind == ErrorKind::Storage { return error; }
    if error.transmitted != Some(false) && require_context(client, context).is_err() {
        return renewal_unknown();
    }
    if error.definitive || error.transmitted == Some(false) {
        if let Err(storage) = client.clear_renewal_attempt_if_owned(marker, session, confirmation) { return storage; }
    }
    error
}

fn require_epoch(client: &BiliClient, epoch: u64) -> Result<()> {
    if client.session_generation() == epoch { Ok(()) } else { Err(cancelled()) }
}
fn require_login_epoch(client: &BiliClient, epoch: u64) -> Result<()> {
    if client.login_epoch.load(Ordering::SeqCst) == epoch { Ok(()) } else { Err(cancelled()) }
}

fn cancelled() -> SourceError { SourceError::new(ErrorKind::Cancelled, "account_operation_cancelled") }
fn credential_cancelled() -> SourceError {
    SourceError { transmitted: Some(false), definitive: true,
        ..SourceError::new(ErrorKind::Cancelled, "credential_context_changed") }
}
fn renewal_unknown() -> SourceError {
    SourceError { transmitted: Some(true), definitive: false, ..SourceError::new(ErrorKind::Business, "renewal_outcome_unknown") }
}

fn now() -> i64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|value| value.as_secs().min(i64::MAX as u64) as i64).unwrap_or(0) }
fn form(fields: &[(&str, &str)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.extend_pairs(fields.iter().copied());
    serializer.finish()
}
fn integer(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    value.as_i64().or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}
fn positive_timestamp(value: Option<&Value>) -> Option<i64> {
    integer(value).filter(|value| *value > 0 && *value <= MAX_EXACT_INTEGER as i64)
}
fn identity_id(value: Option<&Value>) -> Option<String> {
    let id = match value? { Value::String(value) => value.clone(), value => value.as_u64()?.to_string() };
    if id.is_empty() || id.len() > 15 || id.starts_with('0') || !id.bytes().all(|byte| byte.is_ascii_digit()) { None } else { Some(id) }
}
fn valid_key(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}
fn valid_opaque(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.chars().any(|value| value.is_whitespace() || value.is_control())
}
fn valid_qr_url(value: &str) -> bool {
    value.len() <= 8192 && valid_opaque(value, 8192) && Url::parse(value).is_ok_and(|url|
        url.scheme() == "https" && url.host_str() == Some("account.bilibili.com")
            && url.username().is_empty() && url.password().is_none() && url.port_or_known_default() == Some(443))
}

fn extract_refresh_csrf(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut offset = 0;
    while let Some(found) = lower[offset..].find("<div") {
        let start = offset + found + 4;
        if !lower.as_bytes().get(start).is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'>') {
            offset = start;
            continue;
        }
        let end = start + lower[start..].find('>')?;
        let attributes = &html[start..end];
        let close = end + 1 + lower[end + 1..].find("</div")?;
        if html_attribute(attributes, "id").as_deref() == Some("1-name") {
            let value = html[end + 1..close].trim();
            if !value.is_empty() && value.len() <= 512
                && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')) {
                return Some(value.into());
            }
        }
        offset = end + 1;
    }
    None
}

fn html_attribute(attributes: &str, wanted: &str) -> Option<String> {
    let bytes = attributes.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) { index += 1; }
        let start = index;
        while bytes.get(index).is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_')) { index += 1; }
        if index == start { index += 1; continue; }
        let name = &attributes[start..index];
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) { index += 1; }
        if bytes.get(index) != Some(&b'=') { continue; }
        index += 1;
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) { index += 1; }
        let quote = bytes.get(index).copied().filter(|byte| matches!(*byte, b'\'' | b'"'));
        if quote.is_some() { index += 1; }
        let start = index;
        while let Some(byte) = bytes.get(index) {
            if quote.map_or_else(|| byte.is_ascii_whitespace(), |quote| *byte == quote) { break; }
            index += 1;
        }
        let value = &attributes[start..index];
        if quote.is_some() {
            if index >= bytes.len() { return None; }
            index += 1;
        }
        if name.eq_ignore_ascii_case(wanted) { return Some(value.to_owned()); }
    }
    None
}

fn production_correspondence_path(timestamp: i64) -> Result<String> {
    let mut seed = [0u8; 32];
    #[cfg(unix)]
    {
        std::fs::File::open("/dev/urandom").and_then(|mut source| source.read_exact(&mut seed))
            .map_err(|_| SourceError::capability("secure_random_unavailable"))?;
    }
    #[cfg(not(unix))]
    { return Err(SourceError::capability("renewal_crypto_platform_unavailable")); }
    let result = correspondence_path(timestamp, &seed);
    for byte in &mut seed {
        // The seed is a live local byte and is cleared before this function returns.
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    result
}

/// Fixed public-key RSA-OAEP from the reviewed official correspondence component.
fn correspondence_path(timestamp: i64, seed: &[u8; 32]) -> Result<String> {
    if timestamp < 0 || timestamp > MAX_EXACT_INTEGER as i64 { return Err(SourceError::invalid("invalid_renewal_timestamp")); }
    let message = format!("refresh_{timestamp}");
    let mut db = Sha256::digest([]).to_vec();
    db.resize(128 - 32 - message.len() - 2, 0);
    db.push(1);
    db.extend_from_slice(message.as_bytes());
    let mask = mgf1(seed, 95);
    for (byte, mask) in db.iter_mut().zip(mask) { *byte ^= mask; }
    let mask = mgf1(&db, 32);
    let mut encoded = vec![0u8];
    encoded.extend(seed.iter().zip(mask).map(|(byte, mask)| byte ^ mask));
    encoded.extend_from_slice(&db);
    let modulus = modulus();
    let message = limbs(&encoded);
    let mut result = message;
    for _ in 0..16 { result = multiply_mod(&result, &result, &modulus); }
    result = multiply_mod(&result, &message, &modulus);
    let mut output = String::with_capacity(256);
    use std::fmt::Write;
    for limb in result.iter().rev() { write!(&mut output, "{limb:04x}").map_err(|_| SourceError::protocol("renewal_crypto_failed"))?; }
    Ok(output)
}

fn mgf1(seed: &[u8], length: usize) -> Vec<u8> {
    let mut output = Vec::with_capacity(length + 32);
    for counter in 0..length.div_ceil(32) as u32 {
        let mut hash = Sha256::new();
        hash.update(seed); hash.update(counter.to_be_bytes());
        output.extend_from_slice(&hash.finalize());
    }
    output.truncate(length);
    output
}

fn modulus() -> [u16; 64] {
    const HEX: &str = concat!(
        "cb81dd8e02470656da04dd38544446e2a3412051cfe9adc6a330a5ef902285096849",
        "60970b91c3360ca29c49e1690ff8fa068cb9dfc6179d1e9585cb9424e847db1ef59f",
        "33e37dd4dca8ccfb7631ee9b4a92640d00c8204300152a0ab7cd802889d3445aec6",
        "9918fe6022b534912e7b095be3424dad1ba81145e969b533181f1");
    let mut bytes = [0u8; 128];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&HEX[index * 2..index * 2 + 2], 16).expect("Fixed RSA modulus");
    }
    limbs(&bytes)
}

fn limbs(bytes: &[u8]) -> [u16; 64] {
    let mut result = [0u16; 64];
    for (index, limb) in result.iter_mut().enumerate() {
        let end = bytes.len() - index * 2;
        *limb = u16::from_be_bytes([bytes[end - 2], bytes[end - 1]]);
    }
    result
}

fn add_mod(left: &[u16; 64], right: &[u16; 64], modulus: &[u16; 64]) -> [u16; 64] {
    let mut result = [0u16; 64];
    let mut carry = 0u32;
    for index in 0..64 {
        let value = u32::from(left[index]) + u32::from(right[index]) + carry;
        result[index] = value as u16;
        carry = value >> 16;
    }
    if carry != 0 || result.iter().rev().cmp(modulus.iter().rev()).is_ge() {
        let mut borrow = 0i32;
        for index in 0..64 {
            let value = i32::from(result[index]) - i32::from(modulus[index]) - borrow;
            borrow = i32::from(value < 0);
            result[index] = value as u16;
        }
    }
    result
}

fn multiply_mod(left: &[u16; 64], right: &[u16; 64], modulus: &[u16; 64]) -> [u16; 64] {
    let mut result = [0u16; 64];
    let mut addend = *left;
    for digit in right {
        let mut digit = *digit;
        for _ in 0..16 {
            if digit & 1 != 0 { result = add_mod(&result, &addend, modulus); }
            digit >>= 1;
            addend = add_mod(&addend, &addend, modulus);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, path::Path, sync::{Arc, Mutex}};
    use super::super::{crypto::NativeCrypto, transport::{Transport, ResponseFuture}};

    struct FixtureTransport {
        replies: Mutex<VecDeque<(&'static str, HttpResponse)>>,
        requests: Mutex<Vec<HttpRequest>>,
    }

    impl Transport for FixtureTransport {
        fn send(&self, request: HttpRequest) -> ResponseFuture<'_> {
            Box::pin(async move {
                let (path, response) = self.replies.lock().unwrap().pop_front().expect("Fixture route");
                let url = Url::parse(&request.url).unwrap();
                if let Some(prefix) = path.strip_suffix('*') { assert!(url.path().starts_with(prefix)); }
                else { assert_eq!(url.path(), path); }
                self.requests.lock().unwrap().push(request);
                Ok(response)
            })
        }
    }

    fn reply(value: Value, cookies: &[&str]) -> HttpResponse {
        HttpResponse { status: 200, headers: cookies.iter().map(|value| ("set-cookie".into(), (*value).into())).collect(),
            body: serde_json::to_vec(&value).unwrap() }
    }

    fn fixture(routes: Vec<(&'static str, HttpResponse)>) -> (BiliClient, Arc<FixtureTransport>) {
        let transport = Arc::new(FixtureTransport { replies: Mutex::new(routes.into()), requests: Mutex::new(Vec::new()) });
        let client = BiliClient::with_adapters(None, Path::new("unused-fixture"), transport.clone(),
            Arc::new(NativeCrypto::new(Path::new("unused-fixture"))));
        (client, transport)
    }

    fn pending_renewal() -> AuthSession {
        let mut session = AuthSession::from_cookie_text("SESSDATA=fixture-session; bili_jct=fixture-csrf; DedeUserID=42")
            .unwrap().with_identity(AccountIdentity { id: "42".into(), name: "Fixture".into(), avatar_url: String::new() }, 1).unwrap();
        session.refresh_token = Some("fixture-new-refresh".into());
        session.pending_refresh_token = Some("fixture-old-refresh".into());
        session
    }

    fn alternate_renewal() -> AuthSession {
        let mut session = AuthSession::from_cookie_text("SESSDATA=alternate-session; bili_jct=alternate-csrf; DedeUserID=43")
            .unwrap().with_identity(AccountIdentity { id: "43".into(), name: "Alternate Fixture".into(), avatar_url: String::new() }, 1).unwrap();
        session.refresh_token = Some("alternate-new-refresh".into());
        session.pending_refresh_token = Some("alternate-old-refresh".into());
        session
    }

    fn with_device(mut session: AuthSession) -> AuthSession {
        session.apply_set_cookie(&[("set-cookie".into(),
            "buvid3=fixture-device; Domain=.bilibili.com; Path=/".into())], MANGA, now()).unwrap();
        session
    }

    fn request_header<'a>(request: &'a HttpRequest, name: &str) -> Option<&'a str> {
        request.headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
    }

    fn request_form(request: &HttpRequest) -> std::collections::BTreeMap<String, String> {
        url::form_urlencoded::parse(&request.body).into_owned().collect()
    }

    struct GatedTransport {
        base: Arc<FixtureTransport>,
        path: &'static str,
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    impl Transport for GatedTransport {
        fn send(&self, request: HttpRequest) -> ResponseFuture<'_> {
            Box::pin(async move {
                if Url::parse(&request.url).unwrap().path() == self.path {
                    self.entered.notify_one();
                    self.release.notified().await;
                }
                self.base.send(request).await
            })
        }
    }

    fn assert_not_dispatched(error: SourceError) {
        assert_eq!(error.kind, ErrorKind::Cancelled);
        assert_eq!(error.transmitted, Some(false));
        assert!(error.definitive);
        assert!(!error.retryable);
    }

    #[test]
    fn renewal_stage_getters_distinguish_pending_confirmation_from_uncertainty() {
        let mut session = pending_renewal();
        assert!(session.needs_refresh_confirmation());
        assert!(!session.renewal_is_uncertain());
        session.confirmation_blocked = true;
        assert!(session.needs_refresh_confirmation());
        assert!(session.renewal_is_uncertain());
        session.pending_refresh_token = None;
        session.confirmation_blocked = false;
        session.refresh_blocked = true;
        assert!(!session.needs_refresh_confirmation());
        assert!(session.renewal_is_uncertain());
    }

    #[tokio::test]
    async fn queued_renewal_operations_reject_account_and_credential_replacement_after_the_lock() {
        use std::{future::Future, pin::Pin, task::{Context, Waker}};
        for confirmation in [false, true] {
            for same_account in [false, true] {
                let mut initial = pending_renewal();
                if !confirmation { initial.pending_refresh_token = None; }
                let (_, transport) = fixture(Vec::new());
                let client = BiliClient::with_adapters(Some(initial.clone()), Path::new("unused-fixture"), transport.clone(),
                    Arc::new(NativeCrypto::new(Path::new("unused-fixture"))));
                let (account, epoch) = client.credential_context().unwrap();
                let lock = RENEWAL_LOCK.get_or_init(|| tokio::sync::Mutex::new(())).lock().await;
                let mut queued: Pin<Box<dyn Future<Output = Result<AuthSession>> + Send + '_>> = if confirmation {
                    Box::pin(client.confirm_refresh_for_session(&account, epoch))
                } else { Box::pin(client.refresh_session_for_session(&account, epoch)) };
                assert!(queued.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
                let replacement = if same_account {
                    let mut replacement = initial;
                    replacement.cookies.insert("SESSDATA".into(), "new-generation-session".into());
                    replacement.credential_generation += 1;
                    replacement
                } else { alternate_renewal() };
                client.replace_session(Some(replacement.clone())).unwrap();
                drop(lock);
                assert_not_dispatched(queued.await.unwrap_err());
                assert_eq!(client.session().unwrap(), replacement);
                assert!(transport.requests.lock().unwrap().is_empty());
            }
        }
    }

    #[tokio::test]
    async fn unbound_renewal_futures_capture_credentials_before_their_first_poll() {
        for confirmation in [false, true] {
            let mut initial = pending_renewal();
            if !confirmation { initial.pending_refresh_token = None; }
            let (_, transport) = fixture(Vec::new());
            let client = BiliClient::with_adapters(Some(initial), Path::new("unused-fixture"), transport.clone(),
                Arc::new(NativeCrypto::new(Path::new("unused-fixture"))));
            let future: std::pin::Pin<Box<dyn std::future::Future<Output = Result<AuthSession>> + Send>> = if confirmation {
                Box::pin(client.confirm_refresh())
            } else { Box::pin(client.refresh_session()) };
            client.replace_session(Some(alternate_renewal())).unwrap();
            assert_not_dispatched(future.await.unwrap_err());
            assert!(transport.requests.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn refresh_false_follows_only_its_own_cookie_info_commit_without_requesting_confirmation() {
        use serde_json::json;
        let mut initial = pending_renewal(); initial.pending_refresh_token = None;
        let (_, transport) = fixture(vec![
            ("/x/passport-login/web/cookie/info", reply(json!({"code":0,"data":{"refresh":false,"timestamp":1700000000000i64}}), &[])),
            ("/x/web-interface/nav", reply(json!({"code":0,"data":{"isLogin":true,"mid":42,"uname":"Fixture"}}), &[])),
        ]);
        let client = BiliClient::with_adapters(Some(initial), Path::new("unused-fixture"), transport.clone(),
            Arc::new(NativeCrypto::new(Path::new("unused-fixture"))));
        let (account, epoch) = client.credential_context().unwrap();
        let candidate = client.refresh_session_for_session(&account, epoch).await.unwrap();
        assert!(!candidate.needs_refresh_confirmation());
        assert!(!candidate.renewal_is_uncertain());
        assert_eq!(client.credential_epoch(), epoch + 1);
        assert!(transport.requests.lock().unwrap().iter().all(|request| request.method == "GET"));
        assert!(transport.replies.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_unsent_attempt_clears_only_its_marker_and_preserves_concurrent_cookie_updates() {
        use serde_json::json;
        let initial = pending_renewal();
        let (_, transport) = fixture(vec![
            ("/twirp/bookshelf.v1.Bookshelf/ListFavorite", reply(json!({"code":0,"data":{"list":[]}}), &[
                "XSRF-TOKEN=updated%2Bxsrf; Domain=manga.bilibili.com; Path=/",
            ])),
        ]);
        let client = BiliClient::with_adapters(Some(initial.clone()), Path::new("unused-fixture"), transport.clone(),
            Arc::new(NativeCrypto::new(Path::new("unused-fixture"))));
        let (account, epoch) = client.credential_context().unwrap();
        let mut context = CredentialContext { account: Some(account), epoch };
        let marker = mark_renewal_attempt(&client, &mut context, &initial, true).unwrap();
        client.list_favorites(1, 20).await.unwrap();
        assert!(client.credential_epoch() > context.epoch);
        let error = auth_json_bound(&client, PASSPORT, "/x/passport-login/web/confirm/refresh",
            Some(&initial), Some("fixture=not-sent".into()), true, true, Some(&context)).await.unwrap_err();
        assert_not_dispatched(error);
        assert!(client.clear_renewal_attempt_if_owned(marker, &initial, true).unwrap());
        let updated = client.session().unwrap();
        assert!(!updated.renewal_is_uncertain());
        assert!(updated.needs_refresh_confirmation());
        assert_eq!(updated.cookie_value("XSRF-TOKEN"), Some("updated%2Bxsrf"));
        assert_eq!(transport.requests.lock().unwrap().len(), 1);

        let (account, epoch) = client.credential_context().unwrap();
        let mut context = CredentialContext { account: Some(account), epoch };
        let marker = mark_renewal_attempt(&client, &mut context, &updated, true).unwrap();
        let mut replacement = alternate_renewal(); replacement.confirmation_blocked = true;
        client.replace_session(Some(replacement.clone())).unwrap();
        assert!(!client.clear_renewal_attempt_if_owned(marker, &updated, true).unwrap());
        assert_eq!(client.session().unwrap(), replacement);
    }

    #[tokio::test]
    async fn authentication_rejects_required_cookies_outside_the_destination_before_dispatch() {
        for name in ["SESSDATA", "bili_jct"] {
            let mut initial = pending_renewal();
            initial.cookie_domains.insert(name.into(), MANGA.into());
            let (_, transport) = fixture(Vec::new());
            let client = BiliClient::with_adapters(Some(initial.clone()), Path::new("unused-fixture"), transport.clone(),
                Arc::new(NativeCrypto::new(Path::new("unused-fixture"))));
            let mut errors = vec![client.cookie_info().await.unwrap_err(), client.confirm_refresh().await.unwrap_err()];
            if name == "SESSDATA" { errors.push(client.validate_session().await.unwrap_err()); }
            for error in errors {
                assert_eq!(error.kind, ErrorKind::Authentication);
                assert_eq!(error.transmitted, Some(false));
            }
            initial.pending_refresh_token = None;
            client.replace_session(Some(initial)).unwrap();
            let error = client.refresh_session().await.unwrap_err();
            assert_eq!(error.kind, ErrorKind::Authentication);
            assert_eq!(error.transmitted, Some(false));
            assert!(transport.requests.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn passport_requests_use_matching_cookie_and_csrf_without_manga_only_values() {
        use serde_json::json;
        let mut initial = pending_renewal();
        initial.apply_set_cookie(&[("set-cookie".into(),
            "XSRF-TOKEN=manga%2Bxsrf; Domain=manga.bilibili.com; Path=/".into()), ("set-cookie".into(),
            "buvid3=manga-device; Domain=manga.bilibili.com; Path=/".into())], MANGA, now()).unwrap();
        let (_, transport) = fixture(vec![
            ("/x/passport-login/web/cookie/info", reply(json!({"code":0,"data":{"refresh":false,"timestamp":1700000000000i64}}), &[])),
            ("/x/passport-login/web/confirm/refresh", reply(json!({"code":0}), &[])),
        ]);
        let client = BiliClient::with_adapters(Some(initial), Path::new("unused-fixture"), transport.clone(),
            Arc::new(NativeCrypto::new(Path::new("unused-fixture"))));
        client.cookie_info().await.unwrap();
        let candidate = client.confirm_refresh().await.unwrap();
        assert!(candidate.pending_refresh_token.is_none());
        let requests = transport.requests.lock().unwrap();
        for request in requests.iter() {
            assert_eq!(Url::parse(&request.url).unwrap().host_str(), Some(PASSPORT));
            let cookie = request_header(request, "cookie").unwrap();
            assert!(cookie.contains("SESSDATA=fixture-session"));
            assert!(cookie.contains("bili_jct=fixture-csrf"));
            assert!(!cookie.contains("manga"));
            assert!(request_header(request, "x-xsrf-token").is_none());
        }
        let query = Url::parse(&requests[0].url).unwrap().query_pairs().into_owned().collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(query.get("csrf").map(String::as_str), Some("fixture-csrf"));
        let body = request_form(&requests[1]);
        assert_eq!(body.get("csrf").map(String::as_str), Some("fixture-csrf"));
        assert_eq!(body.get("refresh_token").map(String::as_str), Some("fixture-old-refresh"));
        assert_eq!(request_header(&requests[1], "content-type"), Some("application/x-www-form-urlencoded"));
    }

    #[tokio::test]
    async fn identity_and_correspondence_get_requests_do_not_require_passport_only_csrf() {
        use serde_json::json;
        let mut initial = pending_renewal();
        initial.cookie_domains.insert("bili_jct".into(), PASSPORT.into());
        let (_, transport) = fixture(vec![
            ("/x/web-interface/nav", reply(json!({"code":0,"data":{"isLogin":true,"mid":42,"uname":"Fixture"}}), &[])),
            ("/correspond/1/fixture", HttpResponse { status:200, headers:Vec::new(),
                body:b"<div id='1-name'>fixture-challenge</div>".to_vec() }),
        ]);
        let client = BiliClient::with_adapters(Some(initial), Path::new("unused-fixture"), transport.clone(),
            Arc::new(NativeCrypto::new(Path::new("unused-fixture"))));
        client.validate_session().await.unwrap();
        let session = client.session().unwrap();
        auth_request_bound(&client, WWW, "/correspond/1/fixture", Some(&session), None, true, false, None).await.unwrap();
        for request in transport.requests.lock().unwrap().iter() {
            assert_eq!(request.method, "GET");
            let cookie = request_header(request, "cookie").unwrap();
            assert!(cookie.contains("SESSDATA=fixture-session"));
            assert!(!cookie.contains("bili_jct="));
        }
        assert!(transport.replies.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn authentication_error_cookie_rotations_are_available_to_the_next_request() {
        use serde_json::json;
        for http_error in [false, true] {
            let mut rejection = reply(json!({"code":-111}), &[
                "bili_jct=error-rotated-csrf; Domain=.bilibili.com; Path=/",
            ]);
            if http_error { rejection.status = 403; }
            let (_, transport) = fixture(vec![
                ("/x/passport-login/web/cookie/info", rejection),
                ("/x/passport-login/web/cookie/info", reply(json!({"code":0,"data":{"refresh":false,"timestamp":1700000000000i64}}), &[])),
            ]);
            let client = BiliClient::with_adapters(Some(pending_renewal()), Path::new("unused-fixture"), transport.clone(),
                Arc::new(NativeCrypto::new(Path::new("unused-fixture"))));
            let error = client.cookie_info().await.unwrap_err();
            assert_eq!(error.kind, if http_error { ErrorKind::Http } else { ErrorKind::RequestAuthentication });
            assert_eq!(client.session().unwrap().cookie_value("bili_jct"), Some("error-rotated-csrf"));
            client.cookie_info().await.unwrap();
            let requests = transport.requests.lock().unwrap();
            assert!(request_header(&requests[0], "cookie").unwrap().contains("bili_jct=fixture-csrf"));
            assert!(request_header(&requests[1], "cookie").unwrap().contains("bili_jct=error-rotated-csrf"));
            let query = Url::parse(&requests[1].url).unwrap().query_pairs().into_owned()
                .collect::<std::collections::BTreeMap<_, _>>();
            assert_eq!(query.get("csrf").map(String::as_str), Some("error-rotated-csrf"));
            assert!(transport.replies.lock().unwrap().is_empty());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn renewal_error_cookie_rotations_keep_the_attempt_blocked_without_resubmission() {
        use serde_json::json;
        for confirmation in [false, true] {
            let mut initial = pending_renewal();
            let mut routes = Vec::new();
            if !confirmation {
                initial.pending_refresh_token = None;
                routes.push(("/x/passport-login/web/cookie/info", reply(json!({"code":0,"data":{"refresh":true,"timestamp":1700000000000i64}}), &[])));
                routes.push(("/correspond/1/*", HttpResponse { status:200, headers:Vec::new(),
                    body:b"<div id='1-name'>fixture-challenge</div>".to_vec() }));
            }
            routes.push((if confirmation { "/x/passport-login/web/confirm/refresh" }
                else { "/x/passport-login/web/cookie/refresh" }, reply(json!({"code":-111}), &[
                "bili_jct=rejection-rotated-csrf; Domain=.bilibili.com; Path=/",
            ])));
            let (_, transport) = fixture(routes);
            let client = BiliClient::with_adapters(Some(initial), Path::new("unused-fixture"), transport.clone(),
                Arc::new(NativeCrypto::new(Path::new("unused-fixture"))));
            let error = if confirmation { client.confirm_refresh().await.unwrap_err() }
                else { client.refresh_session().await.unwrap_err() };
            assert_eq!(error.code, "renewal_outcome_unknown");
            assert_eq!(error.transmitted, Some(true));
            let current = client.session().unwrap();
            assert_eq!(current.cookie_value("bili_jct"), Some("rejection-rotated-csrf"));
            assert!(current.renewal_is_uncertain());
            let next_error = if confirmation { client.confirm_refresh().await.unwrap_err() }
                else { client.refresh_session().await.unwrap_err() };
            assert_eq!(next_error.code, if confirmation { "renewal_confirmation_unresolved" } else { "renewal_outcome_unresolved" });
            assert_eq!(transport.requests.lock().unwrap().len(), if confirmation { 1 } else { 3 });
            assert!(transport.replies.lock().unwrap().is_empty());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn renewal_applies_challenge_and_confirmation_cookies_before_subsequent_requests() {
        use serde_json::json;
        let mut initial = pending_renewal();
        initial.pending_refresh_token = None;
        let (_, transport) = fixture(vec![
            ("/x/passport-login/web/cookie/info", reply(json!({"code":0,"data":{"refresh":true,"timestamp":1700000000000i64}}), &[])),
            ("/correspond/1/*", HttpResponse { status:200, headers:vec![("set-cookie".into(),
                "bili_jct=challenge-csrf; Domain=.bilibili.com; Path=/".into())],
                body:b"<div id='1-name'>fixture-challenge</div>".to_vec() }),
            ("/x/web-interface/nav", reply(json!({"code":0,"data":{"isLogin":true,"mid":42,"uname":"Fixture"}}), &[])),
            ("/x/passport-login/web/cookie/refresh", reply(json!({"code":0,"data":{"refresh_token":"fixture-rotated-refresh"}}), &[
                "SESSDATA=rotated-session; Domain=.bilibili.com; Path=/", "bili_jct=rotated-csrf; Domain=.bilibili.com; Path=/",
            ])),
            ("/x/web-interface/nav", reply(json!({"code":0,"data":{"isLogin":true,"mid":42,"uname":"Fixture"}}), &[])),
            ("/x/passport-login/web/confirm/refresh", reply(json!({"code":0}), &[
                "bili_jct=confirmed-csrf; Domain=.bilibili.com; Path=/",
            ])),
            ("/x/web-interface/nav", reply(json!({"code":0,"data":{"isLogin":true,"mid":42,"uname":"Fixture"}}), &[])),
        ]);
        let client = BiliClient::with_adapters(Some(initial), Path::new("unused-fixture"), transport.clone(),
            Arc::new(NativeCrypto::new(Path::new("unused-fixture"))));
        let (account, epoch) = client.credential_context().unwrap();
        let refreshed = client.refresh_and_activate_for_session(&account, epoch).await.unwrap();
        assert_eq!(client.session().unwrap(), refreshed);
        assert_eq!(refreshed.refresh_token.as_deref(), Some("fixture-rotated-refresh"));
        assert_eq!(refreshed.pending_refresh_token.as_deref(), Some("fixture-new-refresh"));
        let (account, epoch) = client.credential_context().unwrap();
        let confirmed = client.confirm_and_activate_for_session(&account, epoch).await.unwrap();
        assert_eq!(client.session().unwrap(), confirmed);
        assert_eq!(confirmed.cookie_value("bili_jct"), Some("confirmed-csrf"));
        assert!(!confirmed.needs_refresh_confirmation());
        assert!(!confirmed.renewal_is_uncertain());
        let requests = transport.requests.lock().unwrap();
        assert!(request_header(&requests[1], "cookie").unwrap().contains("bili_jct=fixture-csrf"));
        assert!(request_header(&requests[2], "cookie").unwrap().contains("bili_jct=challenge-csrf"));
        let refresh = request_form(&requests[3]);
        assert_eq!(refresh.get("csrf").map(String::as_str), Some("challenge-csrf"));
        assert_eq!(refresh.get("refresh_csrf").map(String::as_str), Some("fixture-challenge"));
        assert_eq!(refresh.get("refresh_token").map(String::as_str), Some("fixture-new-refresh"));
        assert!(request_header(&requests[3], "cookie").unwrap().contains("bili_jct=challenge-csrf"));
        assert!(request_header(&requests[4], "cookie").unwrap().contains("SESSDATA=rotated-session"));
        let confirmation = request_form(&requests[5]);
        assert_eq!(confirmation.get("csrf").map(String::as_str), Some("rotated-csrf"));
        assert_eq!(confirmation.get("refresh_token").map(String::as_str), Some("fixture-new-refresh"));
        assert!(request_header(&requests[5], "cookie").unwrap().contains("SESSDATA=rotated-session"));
        assert!(request_header(&requests[6], "cookie").unwrap().contains("bili_jct=confirmed-csrf"));
        assert!(transport.replies.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_prepared_confirmation_cannot_overwrite_a_later_cookie_update() {
        use serde_json::json;
        let (_, transport) = fixture(vec![
            ("/x/passport-login/web/confirm/refresh", reply(json!({"code":0}), &[])),
            ("/twirp/bookshelf.v1.Bookshelf/ListFavorite", reply(json!({"code":0,"data":{"list":[]}}), &[
                "XSRF-TOKEN=later%2Bxsrf; Domain=manga.bilibili.com; Path=/",
            ])),
        ]);
        let client = BiliClient::with_adapters(Some(pending_renewal()), Path::new("unused-fixture"), transport.clone(),
            Arc::new(NativeCrypto::new(Path::new("unused-fixture"))));
        let (account, epoch) = client.credential_context().unwrap();
        let prepared = client.prepare_confirmation_for_session(&account, epoch).await.unwrap();
        client.list_favorites(1, 20).await.unwrap();
        assert!(client.credential_epoch() > prepared.expected_epoch);
        let error = prepared.activate(&client).unwrap_err();
        assert_eq!(error.code, "renewal_outcome_unknown");
        assert_eq!(error.transmitted, Some(true));
        let current = client.session().unwrap();
        assert_eq!(current.cookie_value("XSRF-TOKEN"), Some("later%2Bxsrf"));
        assert!(current.confirmation_blocked);
        assert!(current.needs_refresh_confirmation());
        assert_eq!(client.confirm_refresh().await.unwrap_err().code, "renewal_confirmation_unresolved");
        assert_eq!(transport.requests.lock().unwrap().len(), 2);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_prepared_refresh_cannot_overwrite_a_later_cookie_update() {
        use serde_json::json;
        let mut initial = pending_renewal();
        initial.pending_refresh_token = None;
        let (_, transport) = fixture(vec![
            ("/x/passport-login/web/cookie/info", reply(json!({"code":0,"data":{"refresh":true,"timestamp":1700000000000i64}}), &[])),
            ("/correspond/1/*", HttpResponse { status:200, headers:Vec::new(),
                body:b"<div id='1-name'>fixture-challenge</div>".to_vec() }),
            ("/x/passport-login/web/cookie/refresh", reply(json!({"code":0,"data":{"refresh_token":"fixture-rotated-refresh"}}), &[
                "SESSDATA=rotated-session; Domain=.bilibili.com; Path=/", "bili_jct=rotated-csrf; Domain=.bilibili.com; Path=/",
            ])),
            ("/x/web-interface/nav", reply(json!({"code":0,"data":{"isLogin":true,"mid":42,"uname":"Fixture"}}), &[])),
            ("/twirp/bookshelf.v1.Bookshelf/ListFavorite", reply(json!({"code":0,"data":{"list":[]}}), &[
                "XSRF-TOKEN=later%2Bxsrf; Domain=manga.bilibili.com; Path=/",
            ])),
        ]);
        let client = BiliClient::with_adapters(Some(initial), Path::new("unused-fixture"), transport.clone(),
            Arc::new(NativeCrypto::new(Path::new("unused-fixture"))));
        let (account, epoch) = client.credential_context().unwrap();
        let prepared = client.prepare_refresh_for_session(&account, epoch).await.unwrap();
        client.list_favorites(1, 20).await.unwrap();
        assert!(client.credential_epoch() > prepared.expected_epoch);
        assert_eq!(prepared.activate(&client).unwrap_err().code, "renewal_outcome_unknown");
        let current = client.session().unwrap();
        assert_eq!(current.cookie_value("XSRF-TOKEN"), Some("later%2Bxsrf"));
        assert_eq!(current.cookie_value("SESSDATA"), Some("fixture-session"));
        assert!(current.refresh_blocked);
        assert_eq!(client.refresh_session().await.unwrap_err().code, "renewal_outcome_unresolved");
        assert_eq!(transport.requests.lock().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn check_session_invalidates_a_rejection_after_its_own_intermediate_commits() {
        use serde_json::json;
        let (_, transport) = fixture(vec![
            ("/x/web-interface/nav", reply(json!({"code":0,"data":{"isLogin":true,"mid":42,"uname":"Fixture"}}), &[])),
            ("/x/passport-login/web/cookie/info", reply(json!({"code":-101}), &[])),
        ]);
        let client = BiliClient::with_adapters(Some(with_device(pending_renewal())), Path::new("unused-fixture"), transport.clone(),
            Arc::new(NativeCrypto::new(Path::new("unused-fixture"))));
        let error = client.check_session_and_invalidate().await.unwrap_err();
        assert_eq!(error.kind, ErrorKind::Authentication);
        assert!(client.session().is_none());
        assert_eq!(transport.requests.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn authentication_and_identity_rejections_cannot_publish_response_cookies_or_prevent_invalidation() {
        use serde_json::json;
        for (identity_mismatch, cookie) in [
            (false, "buvid3=updated-device; Domain=.bilibili.com; Path=/"),
            (true, "buvid3=updated-device; Domain=.bilibili.com; Path=/"),
            (false, "SESSDATA=unverified-session; Domain=.bilibili.com; Path=/"),
            (true, "SESSDATA=unverified-session; Domain=.bilibili.com; Path=/"),
        ] {
            let rejection = if identity_mismatch {
                json!({"code":0,"data":{"isLogin":true,"mid":43,"uname":"Another fixture"}})
            } else { json!({"code":-101}) };
            let (_, transport) = fixture(vec![("/x/web-interface/nav", reply(rejection, &[cookie]))]);
            let client = BiliClient::with_adapters(Some(with_device(pending_renewal())), Path::new("unused-fixture"), transport.clone(),
                Arc::new(NativeCrypto::new(Path::new("unused-fixture"))));
            let error = client.check_session_and_invalidate().await.unwrap_err();
            assert_eq!(error.kind, if identity_mismatch { ErrorKind::AccountMismatch } else { ErrorKind::Authentication });
            assert!(client.session().is_none());
            assert_eq!(transport.requests.lock().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn a_stale_authentication_rejection_cannot_invalidate_replacement_credentials() {
        use serde_json::json;
        for same_account in [true, false] {
            let (_, base) = fixture(vec![("/x/web-interface/nav", reply(json!({"code":-101}), &[]))]);
            let transport = Arc::new(GatedTransport { base, path:"/x/web-interface/nav",
                entered:tokio::sync::Notify::new(), release:tokio::sync::Notify::new() });
            let initial = with_device(pending_renewal());
            let client = BiliClient::with_adapters(Some(initial.clone()), Path::new("unused-fixture"), transport.clone(),
                Arc::new(NativeCrypto::new(Path::new("unused-fixture"))));
            let checking = client.clone();
            let task = tokio::spawn(async move { checking.check_session_and_invalidate().await });
            transport.entered.notified().await;
            let replacement = if same_account {
                let mut replacement = initial;
                replacement.cookies.insert("SESSDATA".into(), "replacement-session".into());
                replacement.credential_generation += 1;
                replacement
            } else { alternate_renewal() };
            client.replace_session(Some(replacement.clone())).unwrap();
            transport.release.notify_one();
            assert_eq!(task.await.unwrap().unwrap_err().kind, ErrorKind::Cancelled);
            assert_eq!(client.session().unwrap(), replacement);
        }
    }

    #[test]
    fn fixed_correspondence_vector_matches_reviewed_official_wasm() {
        let mut seed = [0u8; 32];
        for (index, byte) in seed.iter_mut().enumerate() { *byte = index as u8; }
        let ciphertext = correspondence_path(1_700_000_000_000, &seed).unwrap();
        assert_eq!(ciphertext, concat!(
            "ba7125952a10520b948f6f64ab5a24ddc09b9cfa0c5590e404ec24c5c42e6c098",
            "c30d27d6c023d5438f3955eb3cc7411a1994f03ec9175d3031958f42d20d0f123b",
            "bd5a4ad35b72fb2dbf7aefbff69f5c3dde0349c31254345db8b385cb7285a6fb9b",
            "4d070f4f523545aea4a11fd1c4bccfb468ae5116545deb070ca4f24a568"));
    }

    #[test]
    fn challenge_parser_accepts_only_the_bounded_token_element() {
        assert_eq!(extract_refresh_csrf("<div class='x' id = \"1-name\">  abc_-19 </div>"), Some("abc_-19".into()));
        assert_eq!(extract_refresh_csrf("<div id='other'>abc</div>"), None);
        assert_eq!(extract_refresh_csrf("<div id='1-name'><script>bad</script></div>"), None);
    }

    #[test]
    fn qr_secrets_are_redacted_from_debug_output() {
        let challenge = QrLoginChallenge { url: "https://account.bilibili.com/?secret=test".into(), key: "private-key".into() };
        let debug = format!("{challenge:?}");
        assert!(!debug.contains("private-key"));
        assert!(!debug.contains("secret"));
        assert!(valid_qr_url("https://account.bilibili.com/h5/account-h5/auth/scan-web?x=1"));
        assert!(!valid_qr_url("https://account.bilibili.com.evil.example/login"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_new_qr_login_can_recover_after_published_session_storage_uncertainty() {
        use serde_json::json;
        let directory = tempfile::tempdir().unwrap();
        let file = crate::source::SessionFile::new(directory.path());
        let (client, transport) = fixture(vec![
            ("/x/passport-login/web/qrcode/generate", reply(json!({"code":0,"data":{
                "qrcode_key":"recovery-key","url":"https://account.bilibili.com/h5/account-h5/auth/scan-web?recovery=1"}}), &[])),
            ("/x/passport-login/web/qrcode/poll", reply(json!({"code":0,"data":{"code":0,"refresh_token":"fresh-qr-refresh"}}), &[
                "SESSDATA=fresh-qr-session; Domain=.bilibili.com; Path=/",
                "bili_jct=fresh-qr-csrf; Domain=.bilibili.com; Path=/",
                "DedeUserID=42; Domain=.bilibili.com; Path=/",
            ])),
            ("/ductape/buvid", reply(json!({"code":0}), &["buvid3=fresh-qr-device; Path=/"])),
            ("/x/web-interface/nav", reply(json!({"code":0,"data":{"isLogin":true,"mid":42,"uname":"Recovered fixture"}}), &[])),
        ]);
        let client = client.with_session_file(file.clone());
        let original = with_device(pending_renewal());
        client.replace_session(Some(original.clone())).unwrap();
        file.fail_next_directory_sync();
        assert_eq!(client.replace_session(Some(original)).unwrap_err().code, "session_publish_sync_unknown");
        assert!(client.session().is_none());

        let challenge = client.begin_qr().await.unwrap();
        let QrLoginState::Confirmed(candidate) = client.poll_qr(&challenge.key).await.unwrap() else {
            panic!("A fresh QR candidate must remain usable while the old session is fenced");
        };
        assert!(client.session().is_none());
        client.replace_session(Some(candidate.clone())).unwrap();
        assert_eq!(client.session(), Some(candidate.clone()));
        assert_eq!(file.load().unwrap(), Some(candidate));
        let requests = transport.requests.lock().unwrap();
        for request in &requests[..3] { assert!(request_header(request, "cookie").is_none()); }
        let validation_cookie = request_header(&requests[3], "cookie").unwrap();
        assert!(validation_cookie.contains("SESSDATA=fresh-qr-session"));
        assert!(!validation_cookie.contains("fixture-session"));
        assert!(transport.replies.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn confirmed_login_initializes_the_site_and_returns_an_inactive_candidate() {
        use serde_json::json;
        let (client, transport) = fixture(vec![
            ("/x/passport-login/web/qrcode/generate", reply(json!({"code":0,"data":{
                "qrcode_key":"fixture-key","url":"https://account.bilibili.com/h5/account-h5/auth/scan-web?fixture=1"}}), &[])),
            ("/x/passport-login/web/qrcode/poll", reply(json!({"code":0,"data":{"code":0,"refresh_token":"fixture-refresh"}}), &[
                "SESSDATA=fixture-session; Domain=.bilibili.com; Path=/", "bili_jct=fixture-csrf; Domain=.bilibili.com; Path=/",
                "DedeUserID=42; Domain=.bilibili.com; Path=/"])),
            ("/ductape/buvid", reply(json!({"code":0}), &[
                "buvid3=fixture-device; Domain=.bilibili.com; Path=/", "SESSDATA=unrelated; Domain=.bilibili.com; Path=/"])),
            ("/x/web-interface/nav", reply(json!({"code":0,"data":{"isLogin":true,"mid":42,"uname":"Fixture"}}), &[])),
        ]);
        let qr = client.begin_qr().await.unwrap();
        let QrLoginState::Confirmed(session) = client.poll_qr(&qr.key).await.unwrap() else { panic!("Confirmed candidate"); };
        assert_eq!(session.account_key(), Some("bili_42"));
        assert!(session.has_device());
        assert_eq!(session.cookie_value("SESSDATA"), Some("fixture-session"));
        assert!(client.session().is_none());
        let requests = transport.requests.lock().unwrap();
        for request in &requests[..3] { assert!(request_header(request, "cookie").is_none()); }
        let cookie = requests[3].headers.iter().find(|(name, _)| name.eq_ignore_ascii_case("cookie")).unwrap().1.as_str();
        assert!(cookie.contains("buvid3=fixture-device"));
        assert!(!cookie.contains("unrelated"));
        assert!(transport.replies.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn qr_generation_and_polling_remain_anonymous_with_an_active_account() {
        use serde_json::json;
        let (_, transport) = fixture(vec![
            ("/x/passport-login/web/qrcode/generate", reply(json!({"code":0,"data":{
                "qrcode_key":"anonymous-key","url":"https://account.bilibili.com/h5/account-h5/auth/scan-web"}}), &[])),
            ("/x/passport-login/web/qrcode/poll", reply(json!({"code":0,"data":{"code":86101}}), &[])),
        ]);
        let initial = pending_renewal();
        let client = BiliClient::with_adapters(Some(initial.clone()), Path::new("unused-fixture"), transport.clone(),
            Arc::new(NativeCrypto::new(Path::new("unused-fixture"))));
        let challenge = client.begin_qr().await.unwrap();
        assert!(matches!(client.poll_qr(&challenge.key).await.unwrap(), QrLoginState::Waiting));
        assert_eq!(client.session().unwrap(), initial);
        for request in transport.requests.lock().unwrap().iter() {
            assert!(request_header(request, "cookie").is_none());
            assert!(request_header(request, "x-xsrf-token").is_none());
        }
    }

    #[tokio::test]
    async fn device_initialization_preserves_host_only_scope_and_expiry() {
        use serde_json::json;
        let (_, transport) = fixture(vec![("/ductape/buvid", reply(json!({"code":0}), &[
            "buvid3=host-device; Path=/; Max-Age=3600", "bili_jct=unrelated; Domain=.bilibili.com; Path=/",
        ]))]);
        let client = BiliClient::with_adapters(Some(pending_renewal()), Path::new("unused-fixture"), transport.clone(),
            Arc::new(NativeCrypto::new(Path::new("unused-fixture"))));
        let session = client.ensure_site_context().await.unwrap();
        assert_eq!(session.cookie_value_for(MANGA, "buvid3"), Some("host-device"));
        assert_eq!(session.cookie_value_for(API, "buvid3"), None);
        assert_eq!(session.cookie_value_for(PASSPORT, "bili_jct"), Some("fixture-csrf"));
        let serialized = serde_json::to_value(&session).unwrap();
        assert_eq!(serialized["cookie_host_only"]["buvid3"], true);
        assert!(serialized["cookie_expirations"]["buvid3"].as_i64().is_some_and(|deadline| deadline > now()));
        assert!(request_header(&transport.requests.lock().unwrap()[0], "cookie").is_none());
    }

    #[tokio::test]
    async fn cancellation_rejects_an_old_key_before_any_poll_request() {
        use serde_json::json;
        let (client, transport) = fixture(vec![("/x/passport-login/web/qrcode/generate", reply(json!({"code":0,"data":{
            "qrcode_key":"old-key","url":"https://account.bilibili.com/h5/account-h5/auth/scan-web"}}), &[]))]);
        let qr = client.begin_qr().await.unwrap();
        client.cancel_login();
        assert_eq!(client.poll_qr(&qr.key).await.unwrap_err().kind, ErrorKind::Cancelled);
        assert_eq!(transport.requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn cancellation_discards_a_poll_response_that_was_already_in_flight() {
        use serde_json::json;
        struct DelayedTransport {
            base: Arc<FixtureTransport>,
            entered: tokio::sync::Notify,
            release: tokio::sync::Notify,
        }
        impl Transport for DelayedTransport {
            fn send(&self, request: HttpRequest) -> ResponseFuture<'_> {
                Box::pin(async move {
                    if Url::parse(&request.url).unwrap().path().ends_with("/poll") {
                        self.entered.notify_one();
                        self.release.notified().await;
                    }
                    self.base.send(request).await
                })
            }
        }
        let (_, base) = fixture(vec![
            ("/x/passport-login/web/qrcode/generate", reply(json!({"code":0,"data":{
                "qrcode_key":"in-flight-key","url":"https://account.bilibili.com/h5/account-h5/auth/scan-web"}}), &[])),
            ("/x/passport-login/web/qrcode/poll", reply(json!({"code":0,"data":{"code":86101}}), &[])),
        ]);
        let transport = Arc::new(DelayedTransport { base, entered: tokio::sync::Notify::new(), release: tokio::sync::Notify::new() });
        let client = BiliClient::with_adapters(None, Path::new("unused-fixture"), transport.clone(),
            Arc::new(NativeCrypto::new(Path::new("unused-fixture"))));
        let challenge = client.begin_qr().await.unwrap();
        let polling_client = client.clone();
        let task = tokio::spawn(async move { polling_client.poll_qr(&challenge.key).await });
        transport.entered.notified().await;
        client.cancel_login();
        transport.release.notify_one();
        assert_eq!(task.await.unwrap().unwrap_err().kind, ErrorKind::Cancelled);
        assert!(client.session().is_none());
    }

    #[tokio::test]
    async fn pending_status_is_read_from_the_nested_code() {
        use serde_json::json;
        let (client, transport) = fixture(vec![
            ("/x/passport-login/web/qrcode/generate", reply(json!({"code":0,"data":{
                "qrcode_key":"waiting-key","url":"https://account.bilibili.com/h5/account-h5/auth/scan-web"}}), &[])),
            ("/x/passport-login/web/qrcode/poll", reply(json!({"code":0,"data":{"code":86101}}), &[])),
        ]);
        let qr = client.begin_qr().await.unwrap();
        assert!(matches!(client.poll_qr(&qr.key).await.unwrap(), QrLoginState::Waiting));
        assert!(client.session().is_none());
        assert!(transport.replies.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn successful_confirmation_cannot_be_replayed_before_the_candidate_is_activated() {
        use serde_json::json;
        let (_, transport) = fixture(vec![("/x/passport-login/web/confirm/refresh", reply(json!({"code":0}), &[]))]);
        let client = BiliClient::with_adapters(Some(pending_renewal()), Path::new("unused-fixture"), transport.clone(),
            Arc::new(NativeCrypto::new(Path::new("unused-fixture"))));
        let candidate = client.confirm_refresh().await.unwrap();
        assert!(candidate.pending_refresh_token.is_none());
        assert!(!candidate.confirmation_blocked);
        assert!(client.session().unwrap().confirmation_blocked);
        assert_eq!(client.confirm_refresh().await.unwrap_err().code, "renewal_confirmation_unresolved");
        assert_eq!(transport.requests.lock().unwrap().len(), 1);
        client.replace_session(Some(candidate)).unwrap();
        assert_eq!(client.confirm_refresh().await.unwrap_err().code, "renewal_confirmation_missing");
    }

    #[tokio::test]
    async fn an_unknown_confirmation_response_keeps_the_attempt_blocked() {
        let (_, transport) = fixture(vec![("/x/passport-login/web/confirm/refresh", HttpResponse {
            status: 200, headers: Vec::new(), body: b"invalid-json".to_vec(),
        })]);
        let client = BiliClient::with_adapters(Some(pending_renewal()), Path::new("unused-fixture"), transport.clone(),
            Arc::new(NativeCrypto::new(Path::new("unused-fixture"))));
        let error = client.confirm_refresh().await.unwrap_err();
        assert_eq!(error.code, "renewal_outcome_unknown");
        assert!(!error.definitive);
        assert!(client.session().unwrap().confirmation_blocked);
        assert_eq!(client.confirm_refresh().await.unwrap_err().code, "renewal_confirmation_unresolved");
        assert_eq!(transport.requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn confirmation_cookie_failures_keep_the_completed_attempt_blocked() {
        use serde_json::json;
        for cookie in [
            "bili_jct=; Domain=.bilibili.com; Path=/; Max-Age=0",
            "bili_jct=invalid value; Domain=.bilibili.com; Path=/",
        ] {
            let (_, transport) = fixture(vec![("/x/passport-login/web/confirm/refresh", reply(json!({"code":0}), &[cookie]))]);
            let client = BiliClient::with_adapters(Some(pending_renewal()), Path::new("unused-fixture"), transport.clone(),
                Arc::new(NativeCrypto::new(Path::new("unused-fixture"))));
            let error = client.confirm_refresh().await.unwrap_err();
            assert_eq!(error.code, "renewal_outcome_unknown");
            assert!(client.session().unwrap().confirmation_blocked);
            assert!(client.session().unwrap().needs_refresh_confirmation());
            assert_eq!(client.confirm_refresh().await.unwrap_err().code, "renewal_confirmation_unresolved");
            assert_eq!(transport.requests.lock().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn confirmation_revalidation_cannot_install_another_account() {
        use serde_json::json;
        let (_, transport) = fixture(vec![
            ("/x/passport-login/web/confirm/refresh", reply(json!({"code":0}), &[
                "bili_jct=confirmed-csrf; Domain=.bilibili.com; Path=/",
            ])),
            ("/x/web-interface/nav", reply(json!({"code":0,"data":{"isLogin":true,"mid":43,"uname":"Another fixture"}}), &[])),
        ]);
        let client = BiliClient::with_adapters(Some(pending_renewal()), Path::new("unused-fixture"), transport.clone(),
            Arc::new(NativeCrypto::new(Path::new("unused-fixture"))));
        let (account, epoch) = client.credential_context().unwrap();
        let error = client.confirm_and_activate_for_session(&account, epoch).await.unwrap_err();
        assert_eq!(error.kind, ErrorKind::AccountMismatch);
        let current = client.session().unwrap();
        assert_eq!(current.account_key(), Some("bili_42"));
        assert_eq!(current.cookie_value("bili_jct"), Some("fixture-csrf"));
        assert!(current.confirmation_blocked);
        assert!(current.needs_refresh_confirmation());
        assert_eq!(transport.requests.lock().unwrap().len(), 2);
    }
}
