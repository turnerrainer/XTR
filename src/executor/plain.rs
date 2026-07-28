//! Plain HTTPS executor — used when a DSL specifies `service: <URI>`.
//!
//! No client certificate. Uses the system trust store (via
//! `reqwest`'s `native-tls` feature backing the default TLS
//! implementation).

use crate::config::Limits;
use crate::error::XtrError;
use reqwest::{Client, Method};
use std::time::Duration;

#[derive(Clone)]
pub struct PlainExecutor {
    client: Client,
    max_response_bytes: usize,
}

impl PlainExecutor {
    pub fn new(limits: &Limits) -> Result<Self, XtrError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(limits.request_timeout_secs))
            .build()
            .map_err(|e| XtrError::Internal(format!("reqwest builder: {e}")))?;
        Ok(Self {
            client,
            max_response_bytes: limits.max_response_bytes,
        })
    }

    pub async fn execute(
        &self,
        uri: &str,
        method: &str,
        envelope: String,
    ) -> Result<String, XtrError> {
        let method = parse_method(method)?;
        tracing::debug!("plain HTTPS {} {}", method, uri);
        let resp = self
            .client
            .request(method, uri)
            .header("content-type", "text/xml; charset=utf-8")
            .body(envelope)
            .send()
            .await
            .map_err(map_send_error)?;

        let status = resp.status();
        let body = read_bounded(resp, self.max_response_bytes).await?;

        if !status.is_success() {
            return Err(XtrError::UpstreamHttpError {
                status: status.as_u16(),
                body: truncate(&body, 1024),
            });
        }
        Ok(body)
    }
}

/// Streams the upstream body chunk-by-chunk, refusing as soon as
/// the cumulative size crosses `limit`. Prevents a malicious or
/// misbehaving upstream from pinning arbitrary memory per request.
pub(crate) async fn read_bounded(
    mut resp: reqwest::Response,
    limit: usize,
) -> Result<String, XtrError> {
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| XtrError::Internal(format!("reading upstream body chunk: {e}")))?
    {
        if buf.len() + chunk.len() > limit {
            return Err(XtrError::UpstreamBodyTooLarge { limit });
        }
        buf.extend_from_slice(&chunk);
    }
    String::from_utf8(buf).map_err(|e| XtrError::XmlParseError(format!("upstream not UTF-8: {e}")))
}

pub(crate) fn parse_method(s: &str) -> Result<Method, XtrError> {
    s.to_uppercase()
        .parse::<Method>()
        .map_err(|_| XtrError::Internal(format!("unknown HTTP method: {s}")))
}

pub(crate) fn map_send_error(e: reqwest::Error) -> XtrError {
    if e.is_timeout() {
        XtrError::UpstreamTimeout
    } else {
        XtrError::Internal(format!("upstream request: {e}"))
    }
}

pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}… (truncated)", &s[..max])
    }
}
