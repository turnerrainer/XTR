//! X-Road Security Server executor — mTLS via PKCS12 keystore.
//!
//! Uses the system trust store (`reqwest` `native-tls` default) —
//! fixes JVM bug #6 (Spring version installed a trust-all
//! X509TrustManager, which silently accepts every cert).

use crate::config::{Limits, SecurityServer};
use crate::error::XtrError;
use reqwest::{Client, Identity};
use std::time::Duration;

use super::plain::{map_send_error, parse_method, read_bounded, truncate};

#[derive(Clone)]
pub struct SsExecutor {
    client: Client,
    url: String,
    max_response_bytes: usize,
}

impl SsExecutor {
    pub fn new(cfg: &SecurityServer, password: &str, limits: &Limits) -> Result<Self, XtrError> {
        let pkcs12 = std::fs::read(&cfg.keystore_path).map_err(|e| {
            XtrError::KeystoreLoadFailed(format!(
                "reading keystore {}: {}",
                cfg.keystore_path.display(),
                e
            ))
        })?;
        let identity = Identity::from_pkcs12_der(&pkcs12, password)
            .map_err(|e| XtrError::KeystoreLoadFailed(format!("parsing PKCS12 keystore: {e}")))?;
        let client = Client::builder()
            .identity(identity)
            .timeout(Duration::from_secs(limits.request_timeout_secs))
            .build()
            .map_err(|e| XtrError::Internal(format!("reqwest builder: {e}")))?;

        tracing::info!(
            "SS executor initialised (keystore={}, url={})",
            cfg.keystore_path.display(),
            cfg.url
        );

        Ok(Self {
            client,
            url: cfg.url.clone(),
            max_response_bytes: limits.max_response_bytes,
        })
    }

    pub async fn execute(&self, method: &str, envelope: String) -> Result<String, XtrError> {
        let method = parse_method(method)?;
        tracing::debug!("SS mTLS {} {}", method, self.url);
        let resp = self
            .client
            .request(method, &self.url)
            .header("content-type", "text/xml; charset=utf-8")
            .body(envelope)
            .send()
            .await
            .map_err(map_send_error)?;

        let status = resp.status();
        let body = read_bounded(resp, self.max_response_bytes).await?;

        if !status.is_success() {
            // Task 010 follow-up (see plain.rs) — recognise SOAP
            // Fault bodies on non-2xx responses.
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
