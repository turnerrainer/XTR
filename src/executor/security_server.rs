//! X-Road Security Server executor — mTLS via PKCS12 keystore.
//!
//! Uses the shared `build_mtls_client` helper (see
//! `super::build_mtls_client`) for the reqwest builder — that
//! function owns all the hardening (identity, TLS floor, redirect
//! policy, optional CA bundle). This module is now a thin URL /
//! method / envelope wrapper on top.
//!
//! Fixes JVM bug #6 (no trust-all TLS) and audit-v1 H4/M2 posture.

use crate::config::{Limits, SecurityServer};
use crate::error::XtrError;
use reqwest::Client;

use super::plain::{map_send_error, parse_method, read_bounded, truncate};

#[derive(Clone)]
pub struct SecurityServerExecutor {
    client: Client,
    url: String,
    max_response_bytes: usize,
}

impl SecurityServerExecutor {
    pub fn new(cfg: &SecurityServer, password: &str, limits: &Limits) -> Result<Self, XtrError> {
        let client = super::build_mtls_client(cfg, password, limits)?;
        tracing::info!(
            "Security Server executor initialised (keystore={}, url={})",
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
        tracing::debug!("Security Server mTLS {} {}", method, self.url);
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
