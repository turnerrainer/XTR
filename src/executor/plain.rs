//! Plain HTTPS executor — used when a DSL specifies `service: <URI>`.
//!
//! No client certificate. Uses the system trust store (via
//! `reqwest`'s `native-tls` feature backing the default TLS
//! implementation).

use crate::error::XtrError;
use reqwest::{Client, Method};
use std::time::Duration;

#[derive(Clone)]
pub struct PlainExecutor {
    client: Client,
}

impl PlainExecutor {
    pub fn new() -> Result<Self, XtrError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| XtrError::Internal(format!("reqwest builder: {e}")))?;
        Ok(Self { client })
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
        let body = resp
            .text()
            .await
            .map_err(|e| XtrError::Internal(format!("reading upstream body: {e}")))?;

        if !status.is_success() {
            return Err(XtrError::UpstreamHttpError {
                status: status.as_u16(),
                body: truncate(&body, 1024),
            });
        }
        Ok(body)
    }
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
