//! Domain-scoped account material and crash-safe private session persistence.

use super::{ErrorKind, Result, SourceError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_SESSION_BYTES: usize = 128 * 1024;
const MAX_COOKIE_BYTES: usize = 16 * 1024;
const MAX_COOKIE_HEADERS: usize = 128;
const SESSION_NAME: &str = "session.json";
const COOKIE_NAMES: &[&str] = &[
    "SESSDATA", "bili_jct", "DedeUserID", "DedeUserID__ckMd5", "buvid3", "buvid4",
    "buvid_fp", "b_nut", "b_lsid", "XSRF-TOKEN",
];
const HOSTS: &[&str] = &[
    "bilibili.com", "manga.bilibili.com", "api.bilibili.com",
    "passport.bilibili.com", "www.bilibili.com",
];
const CREDENTIAL_COOKIES: &[&str] = &["SESSDATA", "bili_jct", "DedeUserID"];
const MAX_COOKIE_TIMESTAMP: i64 = 253_402_300_799;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountIdentity {
    pub id: String,
    pub name: String,
    #[serde(alias = "avatar")]
    pub avatar_url: String,
}

/// Serialization is reserved for private persistence and bounded worker exchange.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthSession {
    pub account_key: String,
    pub identity: Option<AccountIdentity>,
    pub(crate) schema_version: u32,
    pub(crate) cookies: BTreeMap<String, String>,
    pub(crate) cookie_domains: BTreeMap<String, String>,
    pub(crate) cookie_host_only: BTreeMap<String, bool>,
    pub(crate) cookie_expirations: BTreeMap<String, i64>,
    pub(crate) refresh_token: Option<String>,
    pub(crate) pending_refresh_token: Option<String>,
    pub(crate) credential_generation: u64,
    pub(crate) validated_at: i64,
    pub(crate) imported_at: i64,
    pub(crate) refresh_checked_at: Option<i64>,
    pub(crate) last_refreshed_at: Option<i64>,
    pub(crate) cookie_expires_at: Option<i64>,
    pub(crate) refresh_blocked: bool,
    pub(crate) confirmation_blocked: bool,
}

impl Default for AuthSession {
    fn default() -> Self {
        Self {
            account_key: String::new(), identity: None, schema_version: 3,
            cookies: BTreeMap::new(), cookie_domains: BTreeMap::new(),
            cookie_host_only: BTreeMap::new(), cookie_expirations: BTreeMap::new(),
            refresh_token: None, pending_refresh_token: None, credential_generation: 0,
            validated_at: 0, imported_at: now_seconds(), refresh_checked_at: None,
            last_refreshed_at: None, cookie_expires_at: None, refresh_blocked: false,
            confirmation_blocked: false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct CookieRecord<'a> {
    value: &'a str,
    domain: &'a str,
    host_only: bool,
    expires_at: Option<i64>,
}

impl std::fmt::Debug for AuthSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("AuthSession")
            .field("identity", &self.identity)
            .field("account_key", &self.account_key)
            .field("has_session", &self.has_session())
            .field("renewable", &self.renewable())
            .field("credential_generation", &self.credential_generation)
            .finish_non_exhaustive()
    }
}

impl AuthSession {
    pub fn has_session(&self) -> bool {
        self.has_session_at(now_seconds())
    }

    fn has_session_at(&self, now: i64) -> bool {
        self.cookie_value_at("SESSDATA", now).is_some()
    }

    pub(crate) fn has_session_for(&self, host: &str) -> bool {
        self.cookie_value_for(host, "SESSDATA").is_some()
    }

    pub fn account_name(&self) -> &str {
        self.identity.as_ref().map_or("", |identity| identity.name.as_str())
    }

    pub fn account_id(&self) -> &str {
        self.identity.as_ref().map_or("", |identity| identity.id.as_str())
    }

    pub fn account_key(&self) -> Option<&str> {
        self.account_key_at(now_seconds())
    }

    fn account_key_at(&self, now: i64) -> Option<&str> {
        if !self.has_session_at(now) { return None; }
        self.identity.as_ref().filter(|identity| valid_account_id(&identity.id))
            .filter(|identity| self.validated_at > 0 && self.account_key == format!("bili_{}", identity.id))
            .map(|_| self.account_key.as_str())
    }

    pub fn renewable(&self) -> bool {
        self.refresh_token.as_deref().is_some_and(valid_credential)
            && !self.refresh_blocked && !self.confirmation_blocked
    }

    pub(crate) fn same_credentials(&self, other: &Self) -> bool {
        self.account_key == other.account_key
            && self.credential_generation == other.credential_generation
            && CREDENTIAL_COOKIES.iter().all(|name| self.same_cookie(other, name))
            && self.refresh_token == other.refresh_token
    }

    #[cfg(test)]
    pub(crate) fn cookie_value(&self, name: &str) -> Option<&str> {
        self.cookie_value_at(name, now_seconds())
    }

    fn cookie_value_at(&self, name: &str, now: i64) -> Option<&str> {
        let record = self.cookie_record(name)?;
        record.expires_at.is_none_or(|expires_at| expires_at > now).then_some(record.value)
    }

    pub(crate) fn cookie_value_for(&self, host: &str, name: &str) -> Option<&str> {
        self.cookie_value_for_at(host, name, now_seconds())
    }

    fn cookie_value_for_at(&self, host: &str, name: &str, now: i64) -> Option<&str> {
        if !HOSTS.contains(&host) || (name == "XSRF-TOKEN" && host != "manga.bilibili.com") {
            return None;
        }
        let record = self.cookie_record(name)?;
        if (record.host_only && host != record.domain)
            || (!record.host_only && !domain_matches(host, record.domain)) {
            return None;
        }
        self.cookie_value_at(name, now)
    }

    fn cookie_record(&self, name: &str) -> Option<CookieRecord<'_>> {
        let value = self.cookies.get(name).filter(|value| valid_cookie(name, value))?;
        let legacy = matches!(self.schema_version, 1 | 2);
        if !legacy && self.schema_version != 3 { return None; }
        let domain = self.cookie_domains.get(name).map(String::as_str)
            .or_else(|| legacy.then(|| default_cookie_domain(name)))?;
        if !HOSTS.contains(&domain) { return None; }
        let host_only = self.cookie_host_only.get(name).copied().or_else(|| legacy.then_some(false))?;
        let mut expires_at = self.cookie_expirations.get(name).copied();
        if name == "SESSDATA" {
            if legacy && expires_at.is_none() { expires_at = self.cookie_expires_at; }
            else if expires_at != self.cookie_expires_at { return None; }
        }
        if expires_at.is_some_and(|value| !valid_timestamp(value)) { return None; }
        Some(CookieRecord { value, domain, host_only, expires_at })
    }

    fn same_cookie(&self, other: &Self, name: &str) -> bool {
        match (self.cookies.get(name), other.cookies.get(name)) {
            (None, None) => true,
            (Some(_), Some(_)) => self.cookie_record(name).zip(other.cookie_record(name))
                .is_some_and(|(current, candidate)| current == candidate),
            _ => false,
        }
    }

    pub fn has_device(&self) -> bool {
        self.has_device_at(now_seconds())
    }

    fn has_device_at(&self, now: i64) -> bool {
        self.cookie_value_for_at("manga.bilibili.com", "buvid3", now).is_some_and(|value| {
            !value.is_empty() && value.len() <= 256
                && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
    }

    pub fn cookie_header(&self, host: &str) -> Result<String> {
        self.cookie_header_at(host, now_seconds())
    }

    fn cookie_header_at(&self, host: &str, now: i64) -> Result<String> {
        if !HOSTS.contains(&host) { return Err(SourceError::invalid("cookie_origin_unsupported")); }
        self.validate_cookie_metadata()?;
        let mut entries = Vec::new();
        for name in self.cookies.keys() {
            if let Some(value) = self.cookie_value_for_at(host, name, now) {
                entries.push(format!("{name}={value}"));
            }
        }
        Ok(entries.join("; "))
    }

    /// Explicit replacement keeps scope and lifetime metadata attached to the value.
    pub(crate) fn copy_cookie_from(&mut self, other: &Self, name: &str) -> Result<()> {
        self.validate_cookie_metadata()?;
        other.validate_cookie_metadata()?;
        if !COOKIE_NAMES.contains(&name) { return Err(SourceError::invalid("session_cookie_invalid")); }
        let mut candidate = self.clone();
        candidate.copy_cookie_record(other, name);
        *self = self.finish_cookie_update(candidate)?;
        Ok(())
    }

    /// Merge disjoint responses without replaying an older update to the same cookie.
    pub(crate) fn merge_cookie_updates(&self, original: &Self, updated: &Self) -> Result<Self> {
        self.validate_cookie_metadata()?;
        original.validate_cookie_metadata()?;
        updated.validate_cookie_metadata()?;
        let mut candidate = self.clone();
        for name in COOKIE_NAMES {
            if !original.same_cookie(updated, name) && self.same_cookie(original, name) {
                candidate.copy_cookie_record(updated, name);
            }
        }
        self.finish_cookie_update(candidate)
    }

    fn copy_cookie_record(&mut self, other: &Self, name: &str) {
        if let Some(record) = other.cookie_record(name) {
            self.store_cookie(name, record.value, record.domain.to_owned(), record.host_only, record.expires_at);
        } else { self.remove_cookie(name); }
    }

    fn store_cookie(&mut self, name: &str, value: &str, domain: String, host_only: bool, expires_at: Option<i64>) {
        self.cookies.insert(name.to_owned(), value.to_owned());
        self.cookie_domains.insert(name.to_owned(), domain);
        self.cookie_host_only.insert(name.to_owned(), host_only);
        if let Some(expires_at) = expires_at { self.cookie_expirations.insert(name.to_owned(), expires_at); }
        else { self.cookie_expirations.remove(name); }
        if name == "SESSDATA" { self.cookie_expires_at = expires_at; }
    }

    fn remove_cookie(&mut self, name: &str) {
        self.cookies.remove(name);
        self.cookie_domains.remove(name);
        self.cookie_host_only.remove(name);
        self.cookie_expirations.remove(name);
        if name == "SESSDATA" { self.cookie_expires_at = None; }
    }

    fn finish_cookie_update(&self, mut candidate: Self) -> Result<Self> {
        if CREDENTIAL_COOKIES.iter().any(|name| !self.same_cookie(&candidate, name)) {
            candidate.credential_generation = self.credential_generation.checked_add(1)
                .ok_or_else(|| SourceError::protocol("credential_generation_exhausted"))?;
        }
        if candidate.identity.as_ref().is_some_and(|identity| {
            candidate.cookies.get("DedeUserID").is_some_and(|value| value != &identity.id)
        }) {
            return Err(SourceError::new(ErrorKind::AccountMismatch, "cookie_account_identity_mismatch"));
        }
        candidate.validate_cookie_metadata()?;
        Ok(candidate)
    }

    /// Applying a complete response is transactional: malformed headers change nothing.
    pub fn apply_set_cookie(&mut self, headers: &[(String, String)], host: &str, now: i64) -> Result<()> {
        self.apply_set_cookie_for_path(headers, host, "/", now)
    }

    pub(crate) fn apply_set_cookie_for_path(&mut self, headers: &[(String, String)], host: &str,
        request_path: &str, now: i64) -> Result<()> {
        if !HOSTS.contains(&host) { return Err(SourceError::invalid("cookie_origin_unsupported")); }
        let default_path = default_cookie_path(request_path)?;
        let mut candidate = self.clone();
        candidate.migrate_legacy()?;
        candidate.validate_cookie_metadata()?;
        for entry in set_cookie_records(headers)? {
                let mut pieces = entry.trim().split(';');
                let Some((name, value)) = pieces.next().and_then(|pair| pair.split_once('=')) else { continue; };
                let name = name.trim();
                if !COOKIE_NAMES.contains(&name) { continue; }
                let value = value.trim();
                let mut domain = Some(host.to_owned());
                let mut host_only = true;
                let mut path = default_path.to_owned();
                let mut max_age = None;
                let mut expiry = None;
                for attribute in pieces {
                    let Some((field, value)) = attribute.trim().split_once('=') else { continue; };
                    let value = value.trim();
                    match field.trim().to_ascii_lowercase().as_str() {
                        "domain" => { domain = normalize_domain(value); host_only = false; }
                        "path" => path = value.to_owned(),
                        "max-age" => {
                            max_age = Some(value.parse::<i64>()
                                .map_err(|_| SourceError::protocol("cookie_lifetime_invalid"))?);
                        }
                        "expires" => expiry = parse_http_date(value),
                        _ => {}
                    }
                }
                let Some(domain) = domain else { continue; };
                if !domain_matches(host, &domain) || path != "/" { continue; }
                if !value.is_empty() && !valid_cookie(name, value) {
                    return Err(SourceError::protocol("session_cookie_invalid"));
                }
                let deadline = max_age.map(|age| now.saturating_add(age).min(MAX_COOKIE_TIMESTAMP)).or(expiry);
                if value.is_empty() || deadline.is_some_and(|deadline| deadline <= now) {
                    // Cookie identity uses name, domain, and the supported root path.
                    if candidate.cookie_domains.get(name) == Some(&domain) { candidate.remove_cookie(name); }
                } else {
                    if deadline.is_some_and(|value| !valid_timestamp(value)) {
                        return Err(SourceError::protocol("cookie_lifetime_invalid"));
                    }
                    if candidate.cookie_value_at(name, now).is_none() { candidate.remove_cookie(name); }
                    if candidate.cookie_domains.get(name).is_some_and(|existing| existing != &domain) {
                        return Err(SourceError::protocol("session_cookie_scope_conflict"));
                    }
                    candidate.store_cookie(name, value, domain, host_only, deadline);
                }
        }
        *self = self.finish_cookie_update(candidate)?;
        Ok(())
    }

    pub fn with_identity(mut self, identity: AccountIdentity, now: i64) -> Result<Self> {
        if !valid_account_id(&identity.id) || !self.has_session_at(now) {
            return Err(SourceError::authentication("account_identity_unconfirmed"));
        }
        if self.cookies.get("DedeUserID").is_some_and(|value| value != &identity.id) {
            return Err(SourceError::new(ErrorKind::AccountMismatch, "account_identity_mismatch"));
        }
        self.account_key = format!("bili_{}", identity.id);
        self.identity = Some(identity);
        self.validated_at = now;
        Ok(self)
    }

    pub fn from_cookie_text(input: &str) -> Result<Self> {
        Self::from_cookie_text_at(input, now_seconds())
    }

    fn from_cookie_text_at(input: &str, now: i64) -> Result<Self> {
        if input.is_empty() || input.len() > MAX_SESSION_BYTES {
            return Err(SourceError::invalid("session_import_size_invalid"));
        }
        let text = input.strip_prefix('\u{feff}').unwrap_or(input).trim();
        let mut result = Self::default();
        result.imported_at = now;
        if text.starts_with('{') || text.starts_with('[') {
            let value: Value = serde_json::from_str(text).map_err(|_| SourceError::invalid("session_import_json_invalid"))?;
            if let Some(token) = value.get("refresh_token").filter(|token| !token.is_null()) {
                let token = token.as_str().filter(|value| valid_credential(value))
                    .ok_or_else(|| SourceError::invalid("session_refresh_token_invalid"))?;
                result.refresh_token = Some(token.to_owned());
            }
            let cookies = value.get("cookies").unwrap_or(&value);
            match cookies {
                Value::Array(values) => {
                    if values.len() > MAX_COOKIE_HEADERS { return Err(SourceError::invalid("session_cookie_limit")); }
                    for entry in values {
                        let Some(name) = entry.get("name").and_then(Value::as_str) else { continue; };
                        let Some(cookie) = entry.get("value").and_then(Value::as_str) else { continue; };
                        if !COOKIE_NAMES.contains(&name) { continue; }
                        if let Some(path) = entry.get("path") {
                            let path = path.as_str().ok_or_else(|| SourceError::invalid("session_cookie_path_invalid"))?;
                            if path != "/" { continue; }
                        }
                        let supplied_domain = match entry.get("domain") {
                            None | Some(Value::Null) => None,
                            Some(value) => Some(value.as_str()
                                .ok_or_else(|| SourceError::invalid("session_cookie_domain_invalid"))?)
                                .filter(|domain| !domain.is_empty()),
                        };
                        if supplied_domain.is_some_and(|domain| normalize_domain(domain).is_none()) { continue; }
                        let domain = supplied_domain.and_then(normalize_domain)
                            .unwrap_or_else(|| default_cookie_domain(name).to_owned());
                        let host_only = match entry.get("hostOnly") {
                            Some(value) => value.as_bool().ok_or_else(|| SourceError::invalid("session_cookie_scope_invalid"))?,
                            None => supplied_domain.is_some_and(|domain| !domain.starts_with('.')),
                        };
                        let expires_at = entry.get("expirationDate").or_else(|| entry.get("expires"))
                            .map(json_timestamp).transpose()?.flatten();
                        if expires_at.is_some_and(|expires_at| expires_at <= now) { continue; }
                        result.insert_import_scoped(name, cookie, domain, host_only, expires_at)?;
                    }
                }
                Value::Object(values) => {
                    for (name, value) in values {
                        if COOKIE_NAMES.contains(&name.as_str()) {
                            let cookie = value.as_str().ok_or_else(|| SourceError::invalid("session_cookie_invalid"))?;
                            result.insert_import(name, cookie, None)?;
                        }
                    }
                    result.import_map_metadata(&value, now)?;
                }
                _ => return Err(SourceError::invalid("session_import_json_invalid")),
            }
        } else if text.contains('\t') {
            for line in text.lines() {
                let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
                if line.is_empty() || line.starts_with('#') { continue; }
                let fields: Vec<&str> = line.splitn(7, '\t').collect();
                if fields.len() != 7 { continue; }
                if !COOKIE_NAMES.contains(&fields[5]) { continue; }
                let Some(domain) = normalize_domain(fields[0]) else { continue; };
                if fields[2] != "/" { continue; }
                let host_only = if fields[1].eq_ignore_ascii_case("TRUE") { false }
                    else if fields[1].eq_ignore_ascii_case("FALSE") { true }
                    else { return Err(SourceError::invalid("session_cookie_scope_invalid")); };
                let expires_at = fields[4].parse::<i64>()
                    .map_err(|_| SourceError::invalid("session_cookie_lifetime_invalid"))?;
                let expires_at = (expires_at != 0).then_some(expires_at);
                if expires_at.is_some_and(|expires_at| expires_at <= now) { continue; }
                result.insert_import_scoped(fields[5], fields[6], domain, host_only, expires_at)?;
            }
        } else {
            let text = if text.get(..7).is_some_and(|prefix| prefix.eq_ignore_ascii_case("cookie:")) {
                text[7..].trim_start()
            } else { text };
            if text.contains('\r') || text.contains('\n') { return Err(SourceError::invalid("session_cookie_header_multiline")); }
            for entry in text.split(';') {
                if let Some((name, value)) = entry.trim().split_once('=') { result.insert_import(name, value, None)?; }
            }
        }
        result.validate_material(false)?;
        Ok(result)
    }

    fn insert_import(&mut self, name: &str, value: &str, domain: Option<String>) -> Result<()> {
        self.insert_import_scoped(name, value, domain.unwrap_or_else(|| default_cookie_domain(name).to_owned()), false, None)
    }

    fn insert_import_scoped(&mut self, name: &str, value: &str, domain: String, host_only: bool, expires_at: Option<i64>) -> Result<()> {
        if !COOKIE_NAMES.contains(&name) { return Ok(()); }
        if !valid_cookie(name, value) { return Err(SourceError::invalid("session_cookie_invalid")); }
        if expires_at.is_some_and(|expires_at| !valid_timestamp(expires_at)) {
            return Err(SourceError::invalid("session_cookie_lifetime_invalid"));
        }
        if self.cookie_record(name).is_some_and(|record| {
            record.value != value || record.domain != domain.as_str()
                || record.host_only != host_only || record.expires_at != expires_at
        }) {
            return Err(SourceError::invalid("session_cookie_conflict"));
        }
        self.store_cookie(name, value, domain, host_only, expires_at);
        Ok(())
    }

    fn import_map_metadata(&mut self, value: &Value, now: i64) -> Result<()> {
        if let Some(domains) = value.get("cookie_domains") {
            let domains = domains.as_object().ok_or_else(|| SourceError::invalid("session_cookie_domain_invalid"))?;
            for (name, domain) in domains {
                if !self.cookies.contains_key(name) { return Err(SourceError::invalid("session_cookie_domain_invalid")); }
                let domain = domain.as_str().and_then(normalize_domain)
                    .ok_or_else(|| SourceError::invalid("session_cookie_domain_invalid"))?;
                self.cookie_domains.insert(name.to_owned(), domain);
            }
        }
        if let Some(scopes) = value.get("cookie_host_only") {
            let scopes = scopes.as_object().ok_or_else(|| SourceError::invalid("session_cookie_scope_invalid"))?;
            for (name, host_only) in scopes {
                if !self.cookies.contains_key(name) { return Err(SourceError::invalid("session_cookie_scope_invalid")); }
                let host_only = host_only.as_bool().ok_or_else(|| SourceError::invalid("session_cookie_scope_invalid"))?;
                self.cookie_host_only.insert(name.to_owned(), host_only);
            }
        }
        if let Some(expirations) = value.get("cookie_expirations") {
            let expirations = expirations.as_object().ok_or_else(|| SourceError::invalid("session_cookie_lifetime_invalid"))?;
            for (name, expires_at) in expirations {
                if !self.cookies.contains_key(name) { return Err(SourceError::invalid("session_cookie_lifetime_invalid")); }
                let expires_at = json_timestamp(expires_at)?.filter(|value| valid_timestamp(*value))
                    .ok_or_else(|| SourceError::invalid("session_cookie_lifetime_invalid"))?;
                self.cookie_expirations.insert(name.to_owned(), expires_at);
            }
        }
        if let Some(expires_at) = value.get("cookie_expires_at") {
            let expires_at = json_timestamp(expires_at)?;
            if self.cookie_expirations.get("SESSDATA").is_some_and(|existing| Some(*existing) != expires_at) {
                return Err(SourceError::invalid("session_cookie_lifetime_invalid"));
            }
            if let Some(expires_at) = expires_at {
                if !self.cookies.contains_key("SESSDATA") || !valid_timestamp(expires_at) {
                    return Err(SourceError::invalid("session_cookie_lifetime_invalid"));
                }
                self.cookie_expirations.insert("SESSDATA".into(), expires_at);
            }
        }
        self.cookie_expires_at = self.cookie_expirations.get("SESSDATA").copied();
        let expired: Vec<String> = self.cookie_expirations.iter()
            .filter(|(_, expires_at)| **expires_at <= now).map(|(name, _)| name.clone()).collect();
        for name in expired { self.remove_cookie(&name); }
        Ok(())
    }

    fn migrate_legacy(&mut self) -> Result<()> {
        if self.schema_version == 3 { return Ok(()); }
        if !matches!(self.schema_version, 1 | 2) { return Err(SourceError::invalid("session_schema_unsupported")); }
        // Legacy imports had domain-cookie semantics; missing scope meant the Bilibili root.
        for name in self.cookies.keys() {
            self.cookie_domains.entry(name.clone()).or_insert_with(|| default_cookie_domain(name).to_owned());
            self.cookie_host_only.entry(name.clone()).or_insert(false);
        }
        if let Some(expires_at) = self.cookie_expires_at {
            if !valid_timestamp(expires_at)
                || self.cookie_expirations.get("SESSDATA").is_some_and(|existing| *existing != expires_at) {
                return Err(SourceError::invalid("session_cookie_lifetime_invalid"));
            }
            self.cookie_expirations.insert("SESSDATA".into(), expires_at);
        } else { self.cookie_expires_at = self.cookie_expirations.get("SESSDATA").copied(); }
        self.schema_version = 3;
        Ok(())
    }

    fn validate_cookie_metadata(&self) -> Result<()> {
        if !matches!(self.schema_version, 1 | 2 | 3) { return Err(SourceError::invalid("session_schema_unsupported")); }
        let legacy = self.schema_version != 3;
        for (name, value) in &self.cookies {
            if !valid_cookie(name, value) { return Err(SourceError::invalid("session_cookie_invalid")); }
            match self.cookie_domains.get(name) {
                Some(domain) if normalize_domain(domain).as_deref() == Some(domain.as_str()) => {},
                None if legacy => {},
                _ => return Err(SourceError::invalid("session_cookie_domain_invalid")),
            }
            if !legacy && !self.cookie_host_only.contains_key(name) {
                return Err(SourceError::invalid("session_cookie_scope_invalid"));
            }
        }
        if self.cookie_domains.keys().any(|name| !self.cookies.contains_key(name)) {
            return Err(SourceError::invalid("session_cookie_domain_invalid"));
        }
        if self.cookie_host_only.keys().any(|name| !self.cookies.contains_key(name)) {
            return Err(SourceError::invalid("session_cookie_scope_invalid"));
        }
        if self.cookie_expirations.iter().any(|(name, expires_at)| {
            !self.cookies.contains_key(name) || !valid_timestamp(*expires_at)
        }) || self.cookie_expires_at.is_some_and(|value| !valid_timestamp(value)) {
            return Err(SourceError::invalid("session_cookie_lifetime_invalid"));
        }
        if (!legacy && self.cookie_expires_at != self.cookie_expirations.get("SESSDATA").copied())
            || (legacy && self.cookie_expires_at.is_some() && self.cookie_expirations.contains_key("SESSDATA")
                && self.cookie_expires_at != self.cookie_expirations.get("SESSDATA").copied())
            || (self.cookie_expires_at.is_some() && !self.cookies.contains_key("SESSDATA")) {
            return Err(SourceError::invalid("session_cookie_lifetime_invalid"));
        }
        Ok(())
    }

    fn validate_material(&self, identity_required: bool) -> Result<()> {
        self.validate_cookie_metadata()?;
        if !self.cookies.get("SESSDATA").is_some_and(|value| valid_cookie("SESSDATA", value)) {
            return Err(SourceError::authentication("session_missing"));
        }
        if self.cookies.get("DedeUserID").is_some_and(|id| !id.bytes().all(|byte| byte.is_ascii_digit())) {
            return Err(SourceError::invalid("session_account_id_invalid"));
        }
        if self.refresh_token.as_deref().is_some_and(|value| !valid_credential(value))
            || self.pending_refresh_token.as_deref().is_some_and(|value| !valid_credential(value)) {
            return Err(SourceError::invalid("session_refresh_token_invalid"));
        }
        if identity_required {
            let identity = self.identity.as_ref().filter(|identity| valid_account_id(&identity.id))
                .ok_or_else(|| SourceError::authentication("session_identity_unconfirmed"))?;
            if self.account_key != format!("bili_{}", identity.id) || self.validated_at <= 0 {
                return Err(SourceError::invalid("session_identity_inconsistent"));
            }
            if self.cookies.get("DedeUserID").is_some_and(|id| id != &identity.id) {
                return Err(SourceError::new(ErrorKind::AccountMismatch, "session_identity_mismatch"));
            }
        }
        Ok(())
    }
}

fn valid_cookie(name: &str, value: &str) -> bool {
    COOKIE_NAMES.contains(&name) && !value.is_empty() && value.len() <= MAX_COOKIE_BYTES
        && value.bytes().all(|byte| byte.is_ascii_graphic() && !matches!(byte, b';' | b','))
}

fn valid_credential(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_COOKIE_BYTES && value.bytes().all(|byte| byte.is_ascii_graphic())
}

fn valid_timestamp(value: i64) -> bool { (1..=MAX_COOKIE_TIMESTAMP).contains(&value) }
fn valid_account_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 20 && !value.starts_with('0') && value.bytes().all(|byte| byte.is_ascii_digit())
}
fn json_timestamp(value: &Value) -> Result<Option<i64>> {
    if value.is_null() { return Ok(None); }
    let value = value.as_f64().or_else(|| value.as_str().and_then(|value| value.parse::<f64>().ok()))
        .filter(|value| value.is_finite() && *value >= i64::MIN as f64 && *value <= MAX_COOKIE_TIMESTAMP as f64)
        .ok_or_else(|| SourceError::invalid("session_cookie_lifetime_invalid"))?;
    // Browser exports use fractional seconds and zero/-1 for session cookies.
    if value == 0.0 || value == -1.0 { return Ok(None); }
    Ok(Some(value.floor() as i64))
}
fn default_cookie_domain(name: &str) -> &'static str {
    if name == "XSRF-TOKEN" { "manga.bilibili.com" } else { "bilibili.com" }
}
fn default_cookie_path(request_path: &str) -> Result<&str> {
    let path = request_path.split(['?', '#']).next().unwrap_or("/");
    if !path.starts_with('/') || request_path.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(SourceError::invalid("cookie_request_path_invalid"));
    }
    let separator = path.rfind('/').unwrap_or(0);
    Ok(if separator == 0 { "/" } else { &path[..separator] })
}
fn normalize_domain(domain: &str) -> Option<String> {
    let domain = domain.trim_start_matches('.').to_ascii_lowercase();
    HOSTS.contains(&domain.as_str()).then_some(domain)
}
fn domain_matches(host: &str, domain: &str) -> bool {
    host == domain || host.strip_suffix(domain).is_some_and(|prefix| prefix.ends_with('.'))
}
fn parse_http_date(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc2822(value).ok().map(|value| value.timestamp())
}
fn now_seconds() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |value| value.as_secs().min(i64::MAX as u64) as i64)
}

/// A comma is a separator only when a new cookie name and '=' follow it.
fn split_cookie_headers(line: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0;
    for (index, byte) in line.bytes().enumerate() {
        if byte != b',' { continue; }
        let tail = line[index + 1..].trim_start();
        if let Some((name, _)) = tail.split_once('=') {
            if !name.is_empty() && name.bytes().all(is_cookie_name_byte) {
                result.push(&line[start..index]);
                start = index + 1;
            }
        }
    }
    result.push(&line[start..]);
    result
}

pub(crate) fn set_cookie_records(headers: &[(String, String)]) -> Result<Vec<String>> {
    let mut records = Vec::new();
    let mut total = 0usize;
    for (_, line) in headers.iter().filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie")) {
        total = total.saturating_add(line.len());
        if total > MAX_SESSION_BYTES || line.bytes().any(|byte| byte.is_ascii_control()) {
            return Err(SourceError::protocol("cookie_header_invalid"));
        }
        for record in split_cookie_headers(line) {
            if records.len() >= MAX_COOKIE_HEADERS { return Err(SourceError::protocol("cookie_header_limit")); }
            records.push(record.trim().to_owned());
        }
    }
    Ok(records)
}

fn is_cookie_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

#[derive(Clone)]
pub struct SessionFile {
    directory: PathBuf,
    #[cfg(test)]
    fail_directory_sync: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl SessionFile {
    pub fn new(data_directory: &Path) -> Self {
        Self {
            directory: data_directory.join("private"),
            #[cfg(test)]
            fail_directory_sync: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
    pub fn path(&self) -> PathBuf { self.directory.join(SESSION_NAME) }

    #[cfg(test)]
    pub(crate) fn fail_next_directory_sync(&self) {
        self.fail_directory_sync.store(true, Ordering::SeqCst);
    }

    #[cfg(unix)]
    fn sync_directory(&self, directory: &File) -> std::io::Result<()> {
        #[cfg(test)]
        if self.fail_directory_sync.swap(false, Ordering::SeqCst) {
            return Err(std::io::Error::from_raw_os_error(libc::EIO));
        }
        directory.sync_all()
    }

    pub fn load(&self) -> Result<Option<AuthSession>> {
        self.load_at(now_seconds())
    }

    fn load_at(&self, now: i64) -> Result<Option<AuthSession>> {
        #[cfg(unix)]
        let file = self.open_saved_unix()?;
        #[cfg(not(unix))]
        let file = self.open_saved_other()?;
        let Some(mut file) = file else { return Ok(None); };
        let mut bytes = Vec::new();
        Read::by_ref(&mut file).take((MAX_SESSION_BYTES + 1) as u64).read_to_end(&mut bytes)
            .map_err(|_| SourceError::storage("session_read_failed"))?;
        if bytes.len() > MAX_SESSION_BYTES { return Err(SourceError::invalid("session_file_size_invalid")); }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| SourceError::invalid("session_file_invalid"))?;
        if !matches!(value.get("schema_version").and_then(Value::as_u64), Some(1) | Some(2) | Some(3)) {
            return Err(SourceError::invalid("session_schema_unsupported"));
        }
        let mut session: AuthSession = serde_json::from_value(value)
            .map_err(|_| SourceError::invalid("session_file_invalid"))?;
        session.migrate_legacy()?;
        session.validate_material(true)?;
        if !session.has_session_at(now) { return Ok(None); }
        Ok(Some(session))
    }

    pub fn save(&self, session: &AuthSession) -> Result<()> {
        let mut session = session.clone();
        session.migrate_legacy()?;
        session.validate_material(true)?;
        let bytes = serde_json::to_vec(&session).map_err(|_| SourceError::storage("session_serialize_failed"))?;
        if bytes.len() > MAX_SESSION_BYTES { return Err(SourceError::invalid("session_file_size_invalid")); }
        #[cfg(unix)]
        { self.save_unix(&bytes) }
        #[cfg(not(unix))]
        { self.save_other(&bytes) }
    }

    pub fn clear(&self) -> Result<()> {
        #[cfg(unix)]
        { self.clear_unix() }
        #[cfg(not(unix))]
        {
            if self.directory.try_exists().map_err(|_| SourceError::storage("session_directory_failed"))? {
                reject_symlink(&self.directory)?;
                remove_if_present(&self.path())?;
                for entry in fs::read_dir(&self.directory).map_err(|_| SourceError::storage("session_clear_sync_unknown"))? {
                    let entry = entry.map_err(|_| SourceError::storage("session_clear_sync_unknown"))?;
                    if entry.file_name().to_str().is_some_and(is_temporary_name) {
                        remove_if_present(&entry.path()).map_err(|_| SourceError::storage("session_clear_sync_unknown"))?;
                    }
                }
            }
            Ok(())
        }
    }

    #[cfg(unix)]
    fn directory_unix(&self, create: bool) -> Result<Option<File>> {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
        if create {
            fs::DirBuilder::new().recursive(true).mode(0o700).create(&self.directory)
                .map_err(|_| SourceError::storage("session_directory_create_failed"))?;
        }
        let directory = match OpenOptions::new().read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW).open(&self.directory) {
            Ok(directory) => directory,
            Err(error) if !create && error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(SourceError::storage("session_directory_open_failed")),
        };
        let metadata = directory.metadata().map_err(|_| SourceError::storage("session_directory_metadata_failed"))?;
        if metadata.uid() != unsafe { libc::geteuid() } { return Err(SourceError::storage("session_directory_owner_invalid")); }
        if create {
            if let Err(error) = directory.set_permissions(fs::Permissions::from_mode(0o700)) {
                if !permission_mode_unavailable(&directory, &error) {
                    return Err(SourceError::storage("session_directory_permissions_failed"));
                }
            }
        } else if metadata.mode() & 0o077 != 0 && !is_fat_volume(&directory) {
            return Err(SourceError::storage("session_directory_permissions_invalid"));
        }
        Ok(Some(directory))
    }

    #[cfg(unix)]
    fn open_saved_unix(&self) -> Result<Option<File>> {
        use std::os::{fd::{AsRawFd, FromRawFd}, unix::fs::MetadataExt};
        let Some(directory) = self.directory_unix(false)? else { return Ok(None); };
        let name = std::ffi::CString::new(SESSION_NAME).map_err(|_| SourceError::storage("session_name_invalid"))?;
        let descriptor = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK) };
        if descriptor < 0 {
            return if std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound { Ok(None) }
                else { Err(SourceError::storage("session_open_failed")) };
        }
        let file = unsafe { File::from_raw_fd(descriptor) };
        let metadata = file.metadata().map_err(|_| SourceError::storage("session_metadata_failed"))?;
        if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() }
            || (metadata.mode() & 0o077 != 0 && !is_fat_volume(&file)) {
            return Err(SourceError::storage("session_file_permissions_invalid"));
        }
        Ok(Some(file))
    }

    #[cfg(unix)]
    fn save_unix(&self, bytes: &[u8]) -> Result<()> {
        use std::os::{fd::{AsRawFd, FromRawFd}, unix::fs::PermissionsExt};
        let directory = self.directory_unix(true)?.ok_or_else(|| SourceError::storage("session_directory_missing"))?;
        let temporary = temporary_name();
        let temporary_c = std::ffi::CString::new(temporary).map_err(|_| SourceError::storage("session_name_invalid"))?;
        let final_c = std::ffi::CString::new(SESSION_NAME).map_err(|_| SourceError::storage("session_name_invalid"))?;
        let descriptor = unsafe { libc::openat(directory.as_raw_fd(), temporary_c.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW, 0o600) };
        if descriptor < 0 { return Err(SourceError::storage("session_temporary_create_failed")); }
        let result = (|| {
            let mut file = unsafe { File::from_raw_fd(descriptor) };
            if let Err(error) = file.set_permissions(fs::Permissions::from_mode(0o600)) {
                if !permission_mode_unavailable(&file, &error) {
                    return Err(SourceError::storage("session_file_permissions_failed"));
                }
            }
            file.write_all(bytes).map_err(|_| SourceError::storage("session_write_failed"))?;
            file.sync_all().map_err(|_| SourceError::storage("session_sync_failed"))?;
            drop(file);
            if unsafe { libc::renameat(directory.as_raw_fd(), temporary_c.as_ptr(), directory.as_raw_fd(), final_c.as_ptr()) } != 0 {
                return Err(SourceError::storage("session_publish_failed"));
            }
            // Publication already happened; a failed directory sync needs read-back reconciliation.
            self.sync_directory(&directory).map_err(|_| SourceError::storage("session_publish_sync_unknown"))
        })();
        if result.is_err() { unsafe { libc::unlinkat(directory.as_raw_fd(), temporary_c.as_ptr(), 0); } }
        result
    }

    #[cfg(unix)]
    fn clear_unix(&self) -> Result<()> {
        use std::os::fd::AsRawFd;
        let Some(directory) = self.directory_unix(false)? else { return Ok(()); };
        let session_name = std::ffi::CString::new(SESSION_NAME).map_err(|_| SourceError::storage("session_name_invalid"))?;
        if unsafe { libc::unlinkat(directory.as_raw_fd(), session_name.as_ptr(), 0) } != 0
            && std::io::Error::last_os_error().kind() != std::io::ErrorKind::NotFound {
            return Err(SourceError::storage("session_remove_failed"));
        }
        // From this point, a failed cleanup or sync cannot restore the removed session.
        for entry in fs::read_dir(&self.directory).map_err(|_| SourceError::storage("session_clear_sync_unknown"))? {
            let entry = entry.map_err(|_| SourceError::storage("session_clear_sync_unknown"))?;
            let Some(name) = entry.file_name().to_str().filter(|name| is_temporary_name(name)).map(str::to_owned) else { continue; };
            let name = std::ffi::CString::new(name).map_err(|_| SourceError::storage("session_clear_sync_unknown"))?;
            if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::NotFound {
                return Err(SourceError::storage("session_clear_sync_unknown"));
            }
        }
        self.sync_directory(&directory).map_err(|_| SourceError::storage("session_clear_sync_unknown"))
    }

    #[cfg(not(unix))]
    fn open_saved_other(&self) -> Result<Option<File>> {
        if !self.directory.try_exists().map_err(|_| SourceError::storage("session_directory_failed"))? { return Ok(None); }
        reject_symlink(&self.directory)?;
        let path = self.path();
        if !path.try_exists().map_err(|_| SourceError::storage("session_metadata_failed"))? { return Ok(None); }
        reject_symlink(&path)?;
        File::open(path).map(Some).map_err(|_| SourceError::storage("session_open_failed"))
    }

    #[cfg(not(unix))]
    fn save_other(&self, bytes: &[u8]) -> Result<()> {
        fs::create_dir_all(&self.directory).map_err(|_| SourceError::storage("session_directory_create_failed"))?;
        reject_symlink(&self.directory)?;
        match fs::symlink_metadata(self.path()) {
            Ok(_) => reject_symlink(&self.path())?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(SourceError::storage("session_metadata_failed")),
        }
        let temporary = self.directory.join(temporary_name());
        let result = (|| {
            let mut file = OpenOptions::new().write(true).create_new(true).open(&temporary)
                .map_err(|_| SourceError::storage("session_temporary_create_failed"))?;
            file.write_all(bytes).map_err(|_| SourceError::storage("session_write_failed"))?;
            file.sync_all().map_err(|_| SourceError::storage("session_sync_failed"))?;
            drop(file);
            atomic_replace(&temporary, &self.path())
        })();
        if result.is_err() { let _ = fs::remove_file(&temporary); }
        result
    }
}

fn temporary_name() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |value| value.as_nanos());
    format!(".session-{}-{stamp}-{}.pending", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))
}
fn is_temporary_name(name: &str) -> bool { name.starts_with(".session-") && name.ends_with(".pending") }

#[cfg(unix)]
fn permission_mode_unavailable(file: &File, error: &std::io::Error) -> bool {
    #[cfg(target_os = "linux")]
    { file_system_kind(file).is_some_and(|kind| is_permissionless_fat(kind, error.raw_os_error())) }
    #[cfg(not(target_os = "linux"))]
    { let _ = (file, error); false }
}

#[cfg(unix)]
fn is_fat_volume(file: &File) -> bool {
    #[cfg(target_os = "linux")]
    {
        // FAT mode bits are mount-level values, not a private per-file ACL.
        file_system_kind(file).is_some_and(|kind| matches!(kind, 0x4d44 | 0x2011_bab0))
    }
    #[cfg(not(target_os = "linux"))]
    { let _ = file; false }
}

#[cfg(target_os = "linux")]
fn file_system_kind(file: &File) -> Option<u64> {
    use std::{mem::MaybeUninit, os::fd::AsRawFd};
    let mut information = MaybeUninit::<libc::statfs>::uninit();
    // The descriptor pins the checked inode; fstatfs initializes one structure.
    if unsafe { libc::fstatfs(file.as_raw_fd(), information.as_mut_ptr()) } != 0 { return None; }
    // A successful fstatfs call initialized the structure above.
    Some(unsafe { information.assume_init() }.f_type as u64)
}

#[cfg(target_os = "linux")]
fn is_permissionless_fat(kind: u64, error: Option<i32>) -> bool {
    matches!(kind, 0x4d44 | 0x2011_bab0)
        && matches!(error, Some(libc::EPERM) | Some(libc::EOPNOTSUPP))
}

#[cfg(not(unix))]
fn reject_symlink(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| SourceError::storage("session_metadata_failed"))?;
    if metadata.file_type().is_symlink() { return Err(SourceError::storage("session_symlink_rejected")); }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 { return Err(SourceError::storage("session_symlink_rejected")); }
    }
    Ok(())
}

#[cfg(not(unix))]
fn remove_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(SourceError::storage("session_remove_failed")),
    }
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" { fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32; }
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0x1 | 0x8) } == 0 {
        return Err(SourceError::storage("session_publish_failed"));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn atomic_replace(_source: &Path, _destination: &Path) -> Result<()> {
    Err(SourceError::capability("private_session_storage_unavailable"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_known_cookies_without_trusting_an_account_identifier() {
        let session = AuthSession::from_cookie_text("Cookie: SESSDATA=fixture; bili_jct=csrf; DedeUserID=42; ignored=value").unwrap();
        assert!(session.has_session());
        assert!(session.identity.is_none());
        assert!(session.account_key.is_empty());
        assert!(!session.cookies.contains_key("ignored"));
        assert!(!format!("{session:?}").contains("fixture"));
    }

    #[test]
    fn domains_and_xsrf_do_not_leak_to_other_origins() {
        let mut session = AuthSession::from_cookie_text("SESSDATA=fixture").unwrap();
        session.apply_set_cookie(&[("Set-Cookie".into(), "XSRF-TOKEN=xsrf; Domain=manga.bilibili.com; Path=/".into())], "manga.bilibili.com", 1).unwrap();
        assert!(session.cookie_header("manga.bilibili.com").unwrap().contains("XSRF-TOKEN="));
        assert!(!session.cookie_header("api.bilibili.com").unwrap().contains("XSRF-TOKEN="));
        assert!(session.cookie_header("i0.hdslb.com").is_err());
    }

    #[test]
    fn repeated_cookie_fields_preserve_expires_and_apply_transactionally() {
        let mut session = AuthSession::from_cookie_text("SESSDATA=old").unwrap();
        session.apply_set_cookie(&[("set-cookie".into(), "SESSDATA=new; Domain=.bilibili.com; Expires=Wed, 21 Oct 2037 07:28:00 GMT; Path=/, bili_jct=csrf; Domain=.bilibili.com; Path=/".into())], "passport.bilibili.com", 1).unwrap();
        assert_eq!(session.cookie_value("SESSDATA"), Some("new"));
        assert_eq!(session.cookie_value("bili_jct"), Some("csrf"));
        assert!(session.cookie_expires_at.is_some());
        let original = session.clone();
        assert!(session.apply_set_cookie(&[("set-cookie".into(), "SESSDATA=changed; Path=/".into()), ("set-cookie".into(), "bili_jct=bad value; Path=/".into())], "passport.bilibili.com", 2).is_err());
        assert_eq!(session, original);
    }

    #[test]
    fn conflicting_imports_and_identity_are_rejected() {
        assert!(AuthSession::from_cookie_text("SESSDATA=a; SESSDATA=b").is_err());
        let session = AuthSession::from_cookie_text("SESSDATA=fixture; DedeUserID=42").unwrap();
        assert!(session.with_identity(AccountIdentity { id: "43".into(), ..Default::default() }, 1).is_err());
        let mut session = AuthSession::from_cookie_text("SESSDATA=fixture; DedeUserID=42").unwrap()
            .with_identity(AccountIdentity { id: "42".into(), ..Default::default() }, 1).unwrap();
        let original = session.clone();
        assert!(session.apply_set_cookie(&[("set-cookie".into(), "DedeUserID=43; Domain=.bilibili.com; Path=/".into())], "passport.bilibili.com", 2).is_err());
        assert_eq!(session, original);
    }

    #[test]
    fn browser_json_and_netscape_preserve_cookie_scope() {
        let session = AuthSession::from_cookie_text(r#"{"cookies":[{"name":"SESSDATA","value":"fixture","domain":".bilibili.com"},{"name":"XSRF-TOKEN","value":"xsrf","domain":"manga.bilibili.com"}],"refresh_token":null}"#).unwrap();
        assert_eq!(session.cookie_domains.get("SESSDATA").map(String::as_str), Some("bilibili.com"));
        let session = AuthSession::from_cookie_text("# Netscape HTTP Cookie File\n#HttpOnly_.bilibili.com\tTRUE\t/\tTRUE\t2000000000\tSESSDATA\tfixture").unwrap();
        assert_eq!(session.cookie_expires_at, Some(2_000_000_000));
        assert!(session.identity.is_none());
    }

    #[test]
    fn host_only_browser_and_netscape_cookies_remain_on_the_exact_host() {
        for input in [
            r#"[{"name":"SESSDATA","value":"fixture","domain":"bilibili.com","hostOnly":true,"path":"/"}]"#,
            "bilibili.com\tFALSE\t/\tTRUE\t0\tSESSDATA\tfixture",
        ] {
            let session = AuthSession::from_cookie_text_at(input, 1000).unwrap();
            assert_eq!(session.cookie_value_for_at("bilibili.com", "SESSDATA", 1000), Some("fixture"));
            assert_eq!(session.cookie_value_for_at("manga.bilibili.com", "SESSDATA", 1000), None);
            assert!(session.cookie_header_at("api.bilibili.com", 1000).unwrap().is_empty());
        }
        let session = AuthSession::from_cookie_text_at("bilibili.com\tTRUE\t/\tTRUE\t0\tSESSDATA\tfixture", 1000).unwrap();
        assert_eq!(session.cookie_value_for_at("manga.bilibili.com", "SESSDATA", 1000), Some("fixture"));
    }

    #[test]
    fn raw_imports_have_explicit_legacy_scope_and_json_nonroot_paths_are_ignored() {
        let session = AuthSession::from_cookie_text_at("SESSDATA=fixture; XSRF-TOKEN=xsrf", 1000).unwrap();
        assert_eq!(session.cookie_domains.get("SESSDATA").map(String::as_str), Some("bilibili.com"));
        assert_eq!(session.cookie_domains.get("XSRF-TOKEN").map(String::as_str), Some("manga.bilibili.com"));
        assert_eq!(session.cookie_host_only.get("SESSDATA"), Some(&false));
        assert_eq!(session.cookie_value_for_at("api.bilibili.com", "XSRF-TOKEN", 1000), None);
        let session = AuthSession::from_cookie_text_at(r#"[
            {"name":"SESSDATA","value":"fixture","domain":".bilibili.com","path":"/"},
            {"name":"XSRF-TOKEN","value":"private","domain":"manga.bilibili.com","path":"/private"}
        ]"#, 1000).unwrap();
        assert!(!session.cookies.contains_key("XSRF-TOKEN"));
        assert!(AuthSession::from_cookie_text_at(r#"[
            {"name":"SESSDATA","value":"fixture","domain":".bilibili.com"},
            {"name":"SESSDATA","value":"fixture","domain":"passport.bilibili.com"}
        ]"#, 1000).is_err());
    }

    #[test]
    fn imported_fractional_expiry_controls_headers_devices_and_account_identity() {
        let session = AuthSession::from_cookie_text_at(r#"[
            {"name":"SESSDATA","value":"fixture","domain":".bilibili.com","expirationDate":1002.75},
            {"name":"buvid3","value":"fixture-device","domain":".bilibili.com","expirationDate":1001.8},
            {"name":"XSRF-TOKEN","value":"xsrf","domain":"manga.bilibili.com","expirationDate":1001.5}
        ]"#, 1000).unwrap().with_identity(AccountIdentity { id: "42".into(), ..Default::default() }, 1000).unwrap();
        assert_eq!(session.cookie_expires_at, Some(1002));
        assert!(session.has_device_at(1000));
        assert!(!session.has_device_at(1001));
        assert_eq!(session.cookie_value_for_at("manga.bilibili.com", "XSRF-TOKEN", 1001), None);
        assert_eq!(session.account_key_at(1001), Some("bili_42"));
        assert!(!session.has_session_at(1002));
        assert_eq!(session.account_key_at(1002), None);
        assert!(session.cookie_header_at("manga.bilibili.com", 1002).unwrap().is_empty());
    }

    #[test]
    fn expired_imports_are_omitted_and_invalid_lifetimes_are_rejected() {
        let session = AuthSession::from_cookie_text_at(r#"[
            {"name":"SESSDATA","value":"fixture","domain":".bilibili.com"},
            {"name":"buvid3","value":"expired","domain":".bilibili.com","expirationDate":999}
        ]"#, 1000).unwrap();
        assert!(!session.cookies.contains_key("buvid3"));
        assert!(AuthSession::from_cookie_text_at(".bilibili.com\tTRUE\t/\tTRUE\t999\tSESSDATA\texpired", 1000).is_err());
        assert!(AuthSession::from_cookie_text_at(r#"[
            {"name":"SESSDATA","value":"fixture","expirationDate":"invalid"}
        ]"#, 1000).is_err());
        assert!(AuthSession::from_cookie_text_at(r#"{
            "cookies":{"SESSDATA":"fixture"},"cookie_expirations":{"SESSDATA":999},"cookie_expires_at":999
        }"#, 1000).is_err());
    }

    #[test]
    fn response_expiry_applies_to_every_cookie_and_max_age_takes_precedence() {
        let mut session = AuthSession::from_cookie_text_at("SESSDATA=fixture", 1000).unwrap();
        session.apply_set_cookie(&[("set-cookie".into(),
            "buvid3=fixture-device; Domain=.bilibili.com; Path=/; Max-Age=2; Expires=Wed, 21 Oct 2037 07:28:00 GMT".into())],
            "manga.bilibili.com", 1000).unwrap();
        assert!(session.has_device_at(1001));
        assert!(!session.has_device_at(1002));
        assert!(!session.cookie_header_at("manga.bilibili.com", 1002).unwrap().contains("buvid3="));
    }

    #[test]
    fn omitted_response_paths_use_the_request_directory_before_root_path_filtering() {
        let mut session = AuthSession::from_cookie_text_at("SESSDATA=fixture", 1000).unwrap();
        let header = [("set-cookie".into(), "buvid3=fixture-device; Domain=.bilibili.com".into())];
        session.apply_set_cookie_for_path(&header, "manga.bilibili.com", "/ductape/buvid?next=/", 1000).unwrap();
        assert!(!session.cookies.contains_key("buvid3"));
        session.apply_set_cookie_for_path(&header, "manga.bilibili.com", "/twirp/service/method?csrf=a/b#fragment", 1000).unwrap();
        assert!(!session.cookies.contains_key("buvid3"));
        session.apply_set_cookie_for_path(&header, "manga.bilibili.com", "/buvid?next=/ductape/buvid", 1000).unwrap();
        assert!(session.has_device_at(1000));
        let mut session = AuthSession::from_cookie_text_at("SESSDATA=fixture", 1000).unwrap();
        session.apply_set_cookie_for_path(&[("set-cookie".into(),
            "buvid3=fixture-device; Domain=.bilibili.com; Path=/".into())],
            "manga.bilibili.com", "/ductape/buvid", 1000).unwrap();
        assert!(session.has_device_at(1000));
    }

    #[test]
    fn response_deletion_matches_domain_but_not_the_host_only_flag() {
        let mut session = AuthSession::from_cookie_text_at("SESSDATA=fixture", 1000).unwrap();
        let original = session.clone();
        session.apply_set_cookie(&[("set-cookie".into(), "SESSDATA=; Path=/; Max-Age=0".into())],
            "manga.bilibili.com", 1000).unwrap();
        assert_eq!(session, original);
        session.apply_set_cookie(&[("set-cookie".into(), "SESSDATA=; Domain=.bilibili.com; Path=/; Max-Age=0".into())],
            "manga.bilibili.com", 1000).unwrap();
        assert!(!session.cookies.contains_key("SESSDATA"));
        assert!(!session.cookie_domains.contains_key("SESSDATA"));
        assert!(!session.cookie_host_only.contains_key("SESSDATA"));
        assert!(!session.cookie_expirations.contains_key("SESSDATA"));
        let mut session = AuthSession::default();
        session.apply_set_cookie(&[("set-cookie".into(), "SESSDATA=fixture; Path=/".into())], "bilibili.com", 1000).unwrap();
        assert!(session.cookie_header_at("manga.bilibili.com", 1000).unwrap().is_empty());
        session.apply_set_cookie(&[("set-cookie".into(), "SESSDATA=; Domain=.bilibili.com; Path=/; Max-Age=0".into())],
            "bilibili.com", 1000).unwrap();
        assert!(!session.has_session_at(1000));
    }

    #[test]
    fn different_domain_response_conflicts_leave_all_cookies_unchanged() {
        let mut session = AuthSession::from_cookie_text_at("SESSDATA=fixture", 1000).unwrap();
        let original = session.clone();
        assert!(session.apply_set_cookie(&[
            ("set-cookie".into(), "buvid3=fixture-device; Domain=.bilibili.com; Path=/".into()),
            ("set-cookie".into(), "SESSDATA=host-session; Path=/".into()),
        ], "manga.bilibili.com", 1000).is_err());
        assert_eq!(session, original);
    }

    #[test]
    fn credential_scope_and_lifetime_changes_advance_the_generation_once() {
        let mut session = AuthSession::from_cookie_text_at("SESSDATA=fixture; bili_jct=csrf", 1000).unwrap();
        let original = session.clone();
        session.apply_set_cookie(&[("set-cookie".into(), "SESSDATA=fixture; Path=/".into())], "bilibili.com", 1000).unwrap();
        assert_eq!(session.credential_generation, original.credential_generation + 1);
        assert!(!session.same_credentials(&original));
        let original = session.clone();
        session.apply_set_cookie(&[
            ("set-cookie".into(), "SESSDATA=fixture; Path=/; Max-Age=100".into()),
            ("set-cookie".into(), "bili_jct=csrf; Domain=.bilibili.com; Path=/; Max-Age=100".into()),
        ], "bilibili.com", 1000).unwrap();
        assert_eq!(session.credential_generation, original.credential_generation + 1);
        assert!(!session.same_credentials(&original));
    }

    #[test]
    fn copying_and_merging_cookies_preserves_metadata_and_newer_parallel_values() {
        let original = AuthSession::from_cookie_text_at("SESSDATA=fixture; XSRF-TOKEN=old; buvid3=original-device", 1000).unwrap();
        let mut current = original.clone();
        current.apply_set_cookie(&[("set-cookie".into(), "XSRF-TOKEN=newer; Path=/; Max-Age=100".into())],
            "manga.bilibili.com", 1000).unwrap();
        let mut updated = original.clone();
        updated.apply_set_cookie(&[
            ("set-cookie".into(), "XSRF-TOKEN=older-response; Path=/".into()),
            ("set-cookie".into(), "buvid3=updated-device; Domain=.bilibili.com; Path=/; Max-Age=50".into()),
        ], "manga.bilibili.com", 1000).unwrap();
        let merged = current.merge_cookie_updates(&original, &updated).unwrap();
        assert_eq!(merged.cookie_value_for_at("manga.bilibili.com", "XSRF-TOKEN", 1000), Some("newer"));
        assert_eq!(merged.cookie_expirations.get("XSRF-TOKEN"), Some(&1100));
        assert_eq!(merged.cookie_value_for_at("manga.bilibili.com", "buvid3", 1000), Some("updated-device"));
        assert_eq!(merged.cookie_expirations.get("buvid3"), Some(&1050));
        assert!(merged.same_credentials(&original));
        let mut target = AuthSession::from_cookie_text_at("SESSDATA=fixture", 1000).unwrap();
        target.copy_cookie_from(&current, "XSRF-TOKEN").unwrap();
        assert!(target.same_cookie(&current, "XSRF-TOKEN"));
        assert_eq!(target.cookie_value_for_at("manga.bilibili.com", "XSRF-TOKEN", 1100), None);
    }

    #[test]
    fn merging_core_changes_advances_once_and_keeps_parallel_deletions() {
        let original = AuthSession::from_cookie_text_at("SESSDATA=fixture; bili_jct=csrf; buvid3=fixture-device", 1000).unwrap();
        let mut current = original.clone();
        current.apply_set_cookie(&[("set-cookie".into(), "buvid3=; Domain=.bilibili.com; Path=/; Max-Age=0".into())],
            "manga.bilibili.com", 1000).unwrap();
        let mut updated = original.clone();
        updated.apply_set_cookie(&[
            ("set-cookie".into(), "SESSDATA=rotated; Domain=.bilibili.com; Path=/; Max-Age=100".into()),
            ("set-cookie".into(), "bili_jct=rotated-csrf; Domain=.bilibili.com; Path=/; Max-Age=100".into()),
            ("set-cookie".into(), "buvid3=older-device; Domain=.bilibili.com; Path=/".into()),
        ], "manga.bilibili.com", 1000).unwrap();
        let merged = current.merge_cookie_updates(&original, &updated).unwrap();
        assert_eq!(merged.credential_generation, current.credential_generation + 1);
        assert_eq!(merged.cookie_value_at("SESSDATA", 1000), Some("rotated"));
        assert_eq!(merged.cookie_expirations.get("bili_jct"), Some(&1100));
        assert!(!merged.cookies.contains_key("buvid3"));
        assert!(!merged.cookie_host_only.contains_key("buvid3"));
    }

    #[cfg(unix)]
    #[test]
    fn legacy_session_files_migrate_explicit_scope_and_save_the_current_schema() {
        let directory = std::env::temp_dir().join(temporary_name());
        let file = SessionFile::new(&directory);
        let session = AuthSession::from_cookie_text_at("SESSDATA=fixture; DedeUserID=42; XSRF-TOKEN=xsrf", 1000).unwrap()
            .with_identity(AccountIdentity { id: "42".into(), ..Default::default() }, 1000).unwrap();
        file.save(&session).unwrap();
        for version in [1, 2] {
            let mut value = serde_json::to_value(&session).unwrap();
            value["schema_version"] = serde_json::json!(version);
            value["cookie_expires_at"] = serde_json::json!(1100);
            let object = value.as_object_mut().unwrap();
            object.remove("cookie_domains");
            object.remove("cookie_host_only");
            object.remove("cookie_expirations");
            fs::write(file.path(), serde_json::to_vec(&value).unwrap()).unwrap();
            let loaded = file.load_at(1000).unwrap().unwrap();
            assert_eq!(loaded.schema_version, 3);
            assert_eq!(loaded.cookie_domains.get("SESSDATA").map(String::as_str), Some("bilibili.com"));
            assert_eq!(loaded.cookie_domains.get("XSRF-TOKEN").map(String::as_str), Some("manga.bilibili.com"));
            assert_eq!(loaded.cookie_host_only.get("SESSDATA"), Some(&false));
            assert_eq!(loaded.cookie_expirations.get("SESSDATA"), Some(&1100));
            assert_eq!(loaded.account_key_at(1000), Some("bili_42"));
            assert_eq!(loaded.cookie_value_for_at("api.bilibili.com", "XSRF-TOKEN", 1000), None);
            file.save(&loaded).unwrap();
            let saved: Value = serde_json::from_slice(&fs::read(file.path()).unwrap()).unwrap();
            assert_eq!(saved.get("schema_version").and_then(Value::as_u64), Some(3));
            assert!(file.load_at(1100).unwrap().is_none());
        }
        file.clear().unwrap();
        fs::remove_dir(&file.directory).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn malformed_persisted_metadata_is_rejected_before_expiry_can_hide_it() {
        let directory = std::env::temp_dir().join(temporary_name());
        let file = SessionFile::new(&directory);
        let session = AuthSession::from_cookie_text_at("SESSDATA=fixture; DedeUserID=42", 1000).unwrap()
            .with_identity(AccountIdentity { id: "42".into(), ..Default::default() }, 1000).unwrap();
        file.save(&session).unwrap();
        let base = serde_json::to_value(&session).unwrap();
        let mut missing_domains = base.clone();
        missing_domains.as_object_mut().unwrap().remove("cookie_domains");
        let mut missing_scope = base.clone();
        missing_scope.as_object_mut().unwrap().remove("cookie_host_only");
        let mut invalid_expiry = base.clone();
        invalid_expiry["cookie_expirations"]["SESSDATA"] = serde_json::json!(0);
        invalid_expiry["cookie_expires_at"] = serde_json::json!(0);
        let mut mismatched_expiry = base.clone();
        mismatched_expiry["cookie_expirations"]["SESSDATA"] = serde_json::json!(999);
        mismatched_expiry["cookie_expires_at"] = serde_json::json!(998);
        let mut orphan_expiry = base.clone();
        orphan_expiry["cookie_expirations"]["buvid3"] = serde_json::json!(999);
        let mut invalid_scope = base.clone();
        invalid_scope["cookie_host_only"]["SESSDATA"] = serde_json::json!("false");
        let mut invalid_domain = base.clone();
        invalid_domain["cookie_domains"]["SESSDATA"] = serde_json::json!("outside.example");
        for value in [missing_domains, missing_scope, invalid_expiry, mismatched_expiry, orphan_expiry, invalid_scope, invalid_domain] {
            fs::write(file.path(), serde_json::to_vec(&value).unwrap()).unwrap();
            assert!(file.load_at(1000).is_err());
        }
        file.clear().unwrap();
        fs::remove_dir(&file.directory).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn directory_sync_failure_is_shared_only_by_clones_after_publication() {
        let directory = std::env::temp_dir().join(temporary_name());
        let file = SessionFile::new(&directory);
        let original = AuthSession::from_cookie_text_at("SESSDATA=fixture; DedeUserID=42", 1000).unwrap()
            .with_identity(AccountIdentity { id: "42".into(), ..Default::default() }, 1000).unwrap();
        file.save(&original).unwrap();
        let mut replacement = original.clone();
        replacement.apply_set_cookie(&[("set-cookie".into(), "SESSDATA=rotated; Domain=.bilibili.com; Path=/".into())],
            "passport.bilibili.com", 1000).unwrap();
        file.fail_next_directory_sync();
        SessionFile::new(&directory).save(&original).unwrap();
        assert_eq!(file.clone().save(&replacement).unwrap_err().code, "session_publish_sync_unknown");
        assert_eq!(file.load_at(1000).unwrap(), Some(replacement));
        file.clone().fail_next_directory_sync();
        assert_eq!(file.clear().unwrap_err().code, "session_clear_sync_unknown");
        assert!(file.load_at(1000).unwrap().is_none());
        file.save(&original).unwrap();
        file.clear().unwrap();
        fs::remove_dir(&file.directory).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn pending_cleanup_failure_after_session_removal_has_an_uncertain_clear_code() {
        let directory = std::env::temp_dir().join(temporary_name());
        let file = SessionFile::new(&directory);
        let session = AuthSession::from_cookie_text_at("SESSDATA=fixture; DedeUserID=42", 1000).unwrap()
            .with_identity(AccountIdentity { id: "42".into(), ..Default::default() }, 1000).unwrap();
        file.save(&session).unwrap();
        let pending = file.directory.join(".session-fixture.pending");
        fs::create_dir(&pending).unwrap();
        assert_eq!(file.clear().unwrap_err().code, "session_clear_sync_unknown");
        assert!(file.load_at(1000).unwrap().is_none());
        fs::remove_dir(pending).unwrap();
        file.clear().unwrap();
        fs::remove_dir(&file.directory).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn only_known_fat_permission_mode_failures_are_admitted() {
        for kind in [0x4d44, 0x2011_bab0] {
            assert!(is_permissionless_fat(kind, Some(libc::EPERM)));
            assert!(is_permissionless_fat(kind, Some(libc::EOPNOTSUPP)));
            assert!(!is_permissionless_fat(kind, Some(libc::EACCES)));
            assert!(!is_permissionless_fat(kind, Some(libc::EIO)));
            assert!(!is_permissionless_fat(kind, None));
        }
        assert!(!is_permissionless_fat(0xef53, Some(libc::EPERM)));
        assert!(!is_permissionless_fat(0x6969, Some(libc::EOPNOTSUPP)));
    }

    #[cfg(unix)]
    #[test]
    fn private_file_replacement_preserves_the_saved_session_on_rejection() {
        use std::os::unix::fs::PermissionsExt;
        let directory = std::env::temp_dir().join(temporary_name());
        let file = SessionFile::new(&directory);
        let original = AuthSession::from_cookie_text("SESSDATA=fixture; bili_jct=csrf; DedeUserID=42").unwrap()
            .with_identity(AccountIdentity { id: "42".into(), name: "Fixture".into(), ..Default::default() }, 1).unwrap();
        file.save(&original).unwrap();
        assert_eq!(file.load().unwrap(), Some(original.clone()));
        assert_eq!(fs::metadata(file.path()).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(&file.directory).unwrap().permissions().mode() & 0o777, 0o700);
        let mut invalid = original.clone();
        invalid.account_key = "bili_43".into();
        assert!(file.save(&invalid).is_err());
        assert_eq!(file.load().unwrap(), Some(original));
        file.clear().unwrap();
        assert!(file.load().unwrap().is_none());
        fs::remove_dir(&file.directory).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_session_symlink_is_rejected_without_reading_its_target() {
        use std::os::unix::fs::symlink;
        let directory = std::env::temp_dir().join(temporary_name());
        let file = SessionFile::new(&directory);
        let session = AuthSession::from_cookie_text("SESSDATA=fixture; DedeUserID=42").unwrap()
            .with_identity(AccountIdentity { id: "42".into(), ..Default::default() }, 1).unwrap();
        file.save(&session).unwrap();
        let target = directory.join("untouched.json");
        fs::rename(file.path(), &target).unwrap();
        symlink(&target, file.path()).unwrap();
        assert!(file.load().is_err());
        file.clear().unwrap();
        assert!(target.exists());
        fs::remove_file(target).unwrap();
        fs::remove_dir(&file.directory).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_session_fifo_is_rejected_before_reading() {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let directory = std::env::temp_dir().join(temporary_name());
        let file = SessionFile::new(&directory);
        let session = AuthSession::from_cookie_text("SESSDATA=fixture; DedeUserID=42").unwrap()
            .with_identity(AccountIdentity { id: "42".into(), ..Default::default() }, 1).unwrap();
        file.save(&session).unwrap();
        fs::remove_file(file.path()).unwrap();
        let path = CString::new(file.path().as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        assert!(file.load().is_err());
        file.clear().unwrap();
        fs::remove_dir(&file.directory).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
