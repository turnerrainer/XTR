//! Request executor — routes an expanded SOAP envelope or a
//! REST passthrough to the upstream and returns the raw response.
//!
//! Three backends selected per-DSL:
//! * `plain` — direct HTTPS for SOAP DSLs with `service:` set
//! * `security_server` — mTLS to X-Road Security Server (SOAP)
//! * `rest_lane` — mTLS to X-Road Security Server (REST passthrough,
//!   issue #5)
//!
//! `Executor::new` builds all applicable backends at startup; the
//! router picks one via `dispatch_soap` or `dispatch_rest`. Fixes
//! JVM bug #6 — no trust-all TLS; we use the system trust store
//! (plus an optional operator-supplied CA bundle for the X-Road
//! private PKI, per `security_server.trust_ca_path`).

use crate::config::{AppConfig, Limits, SecurityServer};
use crate::dsl::{RestTemplate, SoapTemplate};
use crate::error::XtrError;
use axum::http::{HeaderMap, Method};
use reqwest::{redirect::Policy, Certificate, Client, Identity};
use std::time::Duration;

pub mod plain;
pub mod rest_lane;
pub mod security_server;

/// Assembled request executor. Holds all backend clients so a
/// single `Executor` is enough for any DSL kind.
#[derive(Clone)]
pub struct Executor {
    plain: plain::PlainExecutor,
    security_server: Option<security_server::SecurityServerExecutor>,
    rest_lane: Option<rest_lane::RestLaneExecutor>,
}

impl Executor {
    pub fn new(cfg: &AppConfig) -> Result<Self, XtrError> {
        let plain = plain::PlainExecutor::new(&cfg.limits)?;
        let (security_server, rest_lane) = match &cfg.security_server {
            Some(server_cfg) => {
                let password = cfg
                    .keystore_password()?
                    .expect("security_server configured but keystore_password returned None");
                let ss = security_server::SecurityServerExecutor::new(
                    server_cfg,
                    &password,
                    &cfg.limits,
                )?;
                let rest = rest_lane::RestLaneExecutor::new(cfg, server_cfg, &password)?;
                (Some(ss), Some(rest))
            }
            None => (None, None),
        };
        Ok(Self {
            plain,
            security_server,
            rest_lane,
        })
    }

    /// Test-only: replace the REST-lane executor with one that
    /// points at a plain-HTTP mock. Lets integration tests exercise
    /// the router → REST dispatch path without provisioning a PKCS12
    /// keystore for mTLS.
    #[doc(hidden)]
    pub fn __with_rest_lane_for_tests(mut self, rl: rest_lane::RestLaneExecutor) -> Self {
        self.rest_lane = Some(rl);
        self
    }

    /// Send an expanded SOAP envelope to the DSL's target and return
    /// the raw XML response body. Content-Type is set to
    /// `text/xml; charset=utf-8` on the outbound.
    pub async fn dispatch_soap(
        &self,
        template: &SoapTemplate,
        method: &str,
        envelope: String,
    ) -> Result<String, XtrError> {
        match &template.service {
            Some(uri) => self.plain.execute(uri, method, envelope).await,
            None => {
                let server = self.security_server.as_ref().ok_or_else(|| {
                    XtrError::Internal(
                        "SOAP DSL has no `service:` field but no security_server \
                         is configured. Set security_server.url in xtr.yaml \
                         or add a service: URI to the DSL."
                            .into(),
                    )
                })?;
                server.execute(method, envelope).await
            }
        }
    }

    /// Forward a REST request to the X-Road Security Server per the
    /// DSL's `target:` block. Returns the upstream response with
    /// content-type + status preserved so the router can pass it
    /// through to the caller.
    pub async fn dispatch_rest(
        &self,
        template: &RestTemplate,
        method: &Method,
        query_pairs: Vec<(String, String)>,
        inbound_headers: &HeaderMap,
        body: Vec<u8>,
    ) -> Result<rest_lane::RestUpstreamResponse, XtrError> {
        let executor = self.rest_lane.as_ref().ok_or_else(|| {
            XtrError::Internal(
                "REST DSL requires security_server to be configured. \
                 Set security_server.url + keystore_path in xtr.yaml."
                    .into(),
            )
        })?;
        executor
            .execute(template, method, query_pairs, inbound_headers, body)
            .await
    }
}

/// Shared builder for the mTLS `reqwest::Client` used by the SOAP
/// Security Server lane and the REST lane. Applies all the
/// hardening we've accumulated:
///
/// * `identity(PKCS12)` — client-side mTLS
/// * `min_tls_version(TLS_1_2)` — audit-v1 H4
/// * `no_gzip/brotli/deflate` — audit-v1 M2 (response cap honesty)
/// * `redirect(Policy::none())` — spec §4.4 (X-Road never follows
///   redirects; the client shouldn't either)
/// * `add_root_certificate(ca)` when `security_server.trust_ca_path`
///   is set — real X-Road SS certs are behind an operator-private
///   CA that isn't in the system trust store
pub(crate) fn build_mtls_client(
    ss: &SecurityServer,
    password: &str,
    limits: &Limits,
) -> Result<Client, XtrError> {
    let pkcs12 = std::fs::read(&ss.keystore_path).map_err(|e| {
        XtrError::KeystoreLoadFailed(format!(
            "reading keystore {}: {}",
            ss.keystore_path.display(),
            e
        ))
    })?;
    let identity = Identity::from_pkcs12_der(&pkcs12, password)
        .map_err(|e| XtrError::KeystoreLoadFailed(format!("parsing PKCS12 keystore: {e}")))?;

    let mut builder = Client::builder()
        .identity(identity)
        .timeout(Duration::from_secs(limits.request_timeout_secs))
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .redirect(Policy::none());

    if let Some(ca_path) = &ss.trust_ca_path {
        let ca_bytes = std::fs::read(ca_path).map_err(|e| {
            XtrError::KeystoreLoadFailed(format!(
                "reading trust_ca_path {}: {}",
                ca_path.display(),
                e
            ))
        })?;
        let ca = Certificate::from_pem(&ca_bytes)
            .or_else(|_| Certificate::from_der(&ca_bytes))
            .map_err(|e| {
                XtrError::KeystoreLoadFailed(format!(
                    "parsing trust_ca_path {}: {}",
                    ca_path.display(),
                    e
                ))
            })?;
        builder = builder.add_root_certificate(ca);
    }

    builder
        .build()
        .map_err(|e| XtrError::Internal(format!("reqwest builder: {e}")))
}
