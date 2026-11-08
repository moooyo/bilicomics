//! Demonstrations use unavailable providers rather than a live client with UI guards.

use super::{ErrorKind, Result, SourceError};
use super::crypto::{ImageKey, ProtocolCrypto, RequestContext, Signature};
use super::transport::{HttpRequest, ResponseFuture, Transport};
use serde_json::Value;

pub(super) struct OfflineTransport;

impl Transport for OfflineTransport {
    fn send(&self, _request: HttpRequest) -> ResponseFuture<'_> {
        Box::pin(async { Err(network_disabled()) })
    }
}

pub(super) struct OfflineCrypto;

impl ProtocolCrypto for OfflineCrypto {
    fn sign(&self, _context: &RequestContext) -> Result<Signature> { Err(network_disabled()) }

    fn decode_response(&self, _context: &RequestContext, _envelope: Value) -> Result<Value> {
        Err(network_disabled())
    }

    fn new_image_key(&self) -> Result<ImageKey> { Err(network_disabled()) }

    fn decrypt_image(&self, _key: &ImageKey, _url: &str, _bytes: &[u8]) -> Result<Vec<u8>> {
        Err(network_disabled())
    }
}

fn network_disabled() -> SourceError {
    SourceError { transmitted: Some(false), definitive: true,
        ..SourceError::new(ErrorKind::Network, "network_disabled") }
}
