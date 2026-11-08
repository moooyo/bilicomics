//! The transport boundary allows deterministic offline protocol fixtures.

use super::{ErrorKind, Result, SourceError};
use std::{future::Future, pin::Pin, time::Duration};
use url::Url;

pub type ResponseFuture<'a> = Pin<Box<dyn Future<Output = Result<HttpResponse>> + Send + 'a>>;

#[derive(Clone)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub max_bytes: usize,
}

impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("origin", &Url::parse(&self.url).ok().map(|value| value.origin().ascii_serialization()))
            .field("body_bytes", &self.body.len()).field("max_bytes", &self.max_bytes).finish()
    }
}

#[derive(Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
    }
}

impl std::fmt::Debug for HttpResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("HttpResponse").field("status", &self.status)
            .field("body_bytes", &self.body.len()).finish()
    }
}

pub trait Transport: Send + Sync {
    fn send(&self, request: HttpRequest) -> ResponseFuture<'_>;
}

pub struct ReqwestTransport { client: reqwest::Client }

impl ReqwestTransport {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .https_only(true).no_proxy().redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15)).timeout(Duration::from_secs(60))
            .build().map_err(|_| SourceError::capability("https_transport_unavailable"))?;
        Ok(Self { client })
    }
}

impl Transport for ReqwestTransport {
    fn send(&self, request: HttpRequest) -> ResponseFuture<'_> {
        Box::pin(async move {
            let url = Url::parse(&request.url).map_err(|_| SourceError::invalid("invalid_https_resource"))?;
            if url.scheme() != "https" || url.host_str().is_none() || !url.username().is_empty()
                || url.password().is_some() || url.port_or_known_default() != Some(443) {
                return Err(SourceError::invalid("invalid_https_resource"));
            }
            let method = reqwest::Method::from_bytes(request.method.as_bytes())
                .map_err(|_| SourceError::invalid("invalid_http_method"))?;
            let mut builder = self.client.request(method, url);
            for (name, value) in request.headers {
                let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| SourceError::invalid("invalid_http_header"))?;
                let value = reqwest::header::HeaderValue::from_str(&value)
                    .map_err(|_| SourceError::invalid("invalid_http_header"))?;
                builder = builder.header(name, value);
            }
            if !request.body.is_empty() { builder = builder.body(request.body); }
            let mut response = builder.send().await.map_err(network_error)?;
            let status = response.status().as_u16();
            let headers = response.headers().iter().filter_map(|(name, value)| {
                value.to_str().ok().map(|value| (name.as_str().to_owned(), value.to_owned()))
            }).collect();
            if response.content_length().is_some_and(|length| length > request.max_bytes as u64) {
                return Err(SourceError::protocol("response_size_limit"));
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(network_error)? {
                if chunk.len() > request.max_bytes.saturating_sub(body.len()) {
                    return Err(SourceError::protocol("response_size_limit"));
                }
                body.extend_from_slice(&chunk);
            }
            Ok(HttpResponse { status, headers, body })
        })
    }
}

fn network_error(error: reqwest::Error) -> SourceError {
    let code = if error.is_timeout() { "network_timeout" } else { "network_request_failed" };
    SourceError { retryable: true, transmitted: if error.is_connect() { Some(false) } else { None },
        ..SourceError::new(ErrorKind::Network, code) }
}

pub(crate) fn check_status(response: &HttpResponse) -> Result<()> {
    if (200..300).contains(&response.status) { return Ok(()); }
    Err(SourceError { status: Some(response.status), retryable: response.status == 429 || response.status >= 500,
        transmitted: Some(true), definitive: response.status < 500,
        ..SourceError::new(if response.status == 401 { ErrorKind::Authentication } else { ErrorKind::Http },
            "http_response_rejected") })
}
