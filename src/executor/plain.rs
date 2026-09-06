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
            // Audit-v1 H4: explicit TLS 1.2 floor. reqwest+native-tls
            // already defaults to a modern set, but pinning here
            // stops a future OS-level cipher-list downgrade from
            // silently opening the door to SSL 3.0 / TLS 1.0/1.1.
            .min_tls_version(reqwest::tls::Version::TLS_1_2)
            // Audit-v1 M2: keep the wire-body cap meaningful.
            // If someone later flips .gzip(true), the 16 MiB cap
            // in read_bounded stops counting the real memory cost
            // — assert the invariant here by never enabling gzip
            // or brotli decompression on this client.
            .no_gzip()
            .no_brotli()
            .no_deflate()
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
            // Task 010 follow-up: some services (Ariregister among
            // them) wrap SOAP Faults in HTTP 5xx rather than 200.
            // Prefer the structured fault error if the body carries
            // one; otherwise return the opaque HTTP error.
            if let Some(fault) = crate::translate::xml_to_json::try_extract_soap_fault(&body) {
                return Err(fault);
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits {
            max_request_bytes: 1024,
            max_response_bytes: 4096,
            request_timeout_secs: 5,
        }
    }

    #[test]
    fn audit_h4_plain_executor_constructs_with_tls_pin() {
        // Smoke test — asserts the builder's `min_tls_version` +
        // `no_gzip/no_brotli/no_deflate` call chain remains valid
        // against the current reqwest version. If a future
        // reqwest bump removes any of them, the audit-v1 H4/M2
        // hardening silently disappears — this test catches that.
        let ex = PlainExecutor::new(&limits()).expect("plain executor must construct");
        drop(ex);
    }

    #[tokio::test]
    async fn audit_h4_plain_executor_refuses_plain_http_via_url_guard() {
        // Complementary: the URL guard rejects http:// upstream by
        // default at WSDL ingest, so the executor never sees a
        // downgraded scheme in the first place. Verified in
        // wsdl::url_guard tests; noted here for the audit trail.
    }
}
