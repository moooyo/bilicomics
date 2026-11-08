//! Bilibili Comics source adapter with isolated credentials and bounded transport.

mod auth;
mod assets;
mod catalog;
mod client;
pub mod crypto;
mod images;
mod normalize;
mod offline;
mod payments;
mod session;
pub mod transport;
mod types;
pub(crate) mod work;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod cookie_tests;
#[cfg(all(test, target_os = "linux"))]
mod work_integration_tests;

pub use client::BiliClient;
pub use auth::{QrLoginChallenge, QrLoginState, RenewalInfo};
pub use session::{AccountIdentity, AuthSession, SessionFile};
pub use types::*;

pub const REFERENCE_REVISION: &str = "a05ffb987c2089ca389cb3462aafd3ee1c0fa36c";
pub type Result<T> = std::result::Result<T, SourceError>;

/// Safe source errors contain codes and status values, never request bodies or URLs.
#[derive(Clone, thiserror::Error, PartialEq, Eq)]
#[error("{code}")]
pub struct SourceError {
    pub kind: ErrorKind,
    pub code: String,
    pub status: Option<u16>,
    pub business_code: Option<i64>,
    pub retryable: bool,
    pub transmitted: Option<bool>,
    pub definitive: bool,
    /// A validated order identifier retained for explicit reconciliation; Debug redacts it.
    pub reconciliation_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidRequest,
    Authentication,
    RequestAuthentication,
    AccountMismatch,
    Capability,
    Network,
    Http,
    Business,
    Protocol,
    Locked,
    Unavailable,
    Storage,
    PurchaseRejected,
    PurchaseUnknown,
    Cancelled,
}

impl SourceError {
    pub fn new(kind: ErrorKind, code: impl Into<String>) -> Self {
        Self { kind, code: code.into(), status: None, business_code: None, retryable: false,
            transmitted: None, definitive: false, reconciliation_id: None }
    }

    pub fn invalid(code: &str) -> Self {
        Self { transmitted: Some(false), definitive: true, ..Self::new(ErrorKind::InvalidRequest, code) }
    }

    pub fn protocol(code: &str) -> Self { Self::new(ErrorKind::Protocol, code) }
    pub fn capability(code: &str) -> Self {
        Self { transmitted: Some(false), ..Self::new(ErrorKind::Capability, code) }
    }
    pub fn authentication(code: &str) -> Self { Self::new(ErrorKind::Authentication, code) }
    pub fn storage(code: &str) -> Self { Self::new(ErrorKind::Storage, code) }

    pub(crate) fn business(endpoint: &str, code: i64) -> Self {
        let (kind, safe_code, definitive) = match (endpoint, code) {
            (_, -101 | 401) => (ErrorKind::Authentication, "session_rejected", true),
            (_, -111) => (ErrorKind::RequestAuthentication, "request_authentication_rejected", true),
            ("GetImageIndex", 1) => (ErrorKind::Locked, "episode_locked", true),
            ("GetImageIndex", 501) => (ErrorKind::Unavailable, "episode_unavailable", true),
            ("BuyEpisode", 1) => (ErrorKind::PurchaseRejected, "insufficient_coupons", true),
            ("BuyEpisode", 2) => (ErrorKind::PurchaseRejected, "insufficient_currency", true),
            ("BuyEpisode", 3) => (ErrorKind::PurchaseRejected, "purchase_settings_rejected", true),
            ("BuyEpisode", 4) => (ErrorKind::PurchaseRejected, "purchase_price_changed", true),
            ("BuyEpisode", 5) => (ErrorKind::PurchaseRejected, "purchase_coupons_ineligible", true),
            ("BuyEpisode", _) => (ErrorKind::PurchaseUnknown, "purchase_outcome_unknown", false),
            _ => (ErrorKind::Business, "service_rejected", true),
        };
        Self { business_code: Some(code), transmitted: Some(true), definitive, ..Self::new(kind, safe_code) }
    }
}

impl std::fmt::Debug for SourceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("SourceError").field("kind", &self.kind).field("code", &self.code)
            .field("status", &self.status).field("business_code", &self.business_code)
            .field("retryable", &self.retryable).field("transmitted", &self.transmitted)
            .field("definitive", &self.definitive).field("reconciliation_id_present", &self.reconciliation_id.is_some())
            .finish()
    }
}
