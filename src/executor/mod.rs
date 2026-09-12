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
    /// Audit LOG-v1 FN-LOG-3 — test-safety / pentest-safety mode.
    /// When true, every dispatch short-circuits with `OfflineMode`
    /// before any outbound is issued. Sourced from `XTR_OFFLINE`
    /// env var at boot; also settable via test helper.
    offline: bool,
}

/// Audit LOG-v1 FN-LOG-3 — resolve XTR_OFFLINE from the env at boot.
/// Truthy values: "1", "true", "yes", "on" (case-insensitive). Any
/// other value (including empty) leaves the mode disabled — the
/// default posture is "make real outbound calls", so misspellings
/// don't accidentally activate offline mode in production.
fn resolve_offline_from_env() -> bool {
    match std::env::var("XTR_OFFLINE") {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

impl Executor {
    pub fn new(cfg: &AppConfig) -> Result<Self, XtrError> {
        let plain = plain::PlainExecutor::new(&cfg.limits)?;
        let offline = resolve_offline_from_env();
        // Loud boot-time WARN so an operator who accidentally left
        // XTR_OFFLINE=true on a real deployment sees it immediately.
        // Multiple lines because SIEM alerts key off individual lines.
        if offline {
            tracing::warn!(
                "XTR_OFFLINE is set — every outbound SOAP/REST call \
                 will be short-circuited with HTTP 599 xtr_offline. \
                 Test-safety mode; NEVER leave enabled on a \
                 production deployment"
            );
        }
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
            offline,
        })
    }

    /// Test helper: build an Executor whose SOAP/REST dispatch always
    /// short-circuits with `OfflineMode`, without needing to poke the
    /// XTR_OFFLINE env var (which would leak into other tests running
    /// in the same process).
    #[doc(hidden)]
    pub fn with_offline_for_tests(mut self, offline: bool) -> Self {
        self.offline = offline;
        self
    }

    /// Public accessor so the doctor tool can emit the INFO finding
    /// when the mode is active. Distinct from `resolve_offline_from_env`
    /// because a test-configured executor may set it programmatically.
    pub fn is_offline(&self) -> bool {
        self.offline
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
        // Audit LOG-v1 FN-LOG-3: refuse before ANY outbound touches
        // reqwest. Uses structured Debug (`?`) so any control chars
        // in an operator-supplied URI are escape-encoded before
        // landing in the log line.
        if self.offline {
            tracing::info!(
                target = ?template.service.as_deref(),
                "outbound SOAP blocked by XTR_OFFLINE"
            );
            return Err(XtrError::OfflineMode);
        }
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
        // Audit LOG-v1 FN-LOG-3: same short-circuit as dispatch_soap.
        if self.offline {
            tracing::info!(
                target = ?template.target,
                method = %method,
                "outbound REST blocked by XTR_OFFLINE"
            );
            return Err(XtrError::OfflineMode);
        }
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
