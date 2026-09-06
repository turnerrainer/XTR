//! XTR-on-Rust configuration.
//!
//! Wire shape (`xtr.yaml`, DESIGN.md §8.7):
//!
//! ```yaml
//! dsl_path: /DSL
//! port: 8080
//! xroad_instance: ee-test
//! client_data:
//!   # These MUST reflect YOUR organisation's actual X-Road
//!   # registration — never inherit example values from another
//!   # project. See docs/DESIGN.md §2.7 and task 006 for how to
//!   # register with RIA and obtain your codes.
//!   member_class: GOV
//!   member_code: "<your-registry-code>"
//!   subsystem_code: <your-subsystem>   # correctly spelled
//! security_server:
//!   url: "https://out.test.x-tee.ee:443/"
//!   keystore_path: /app/ssl/keystore.p12
//!   keystore_password_env: XTR_KEYSTORE_PASSWORD
//! ```
//!
//! Search order for the file:
//! 1. `--config <path>` CLI flag
//! 2. `XTR_CONFIG` env var
//! 3. `./xtr.yaml` or `./xtr.yml`
//! 4. Built-in defaults if nothing is found

use crate::error::XtrError;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppConfig {
    #[serde(default = "default_dsl_path")]
    pub dsl_path: PathBuf,

    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default = "default_xroad_instance")]
    pub xroad_instance: String,

    /// X-Road SOAP protocol version. Sent as
    /// `{{generate.protocol_version}}` in every envelope's
    /// `<xroad:protocolVersion>` element. Task 005 — before this,
    /// each shipped DSL hardcoded "4.0"; when X-Road moves
    /// versions, only this line changes.
    #[serde(default = "default_xroad_protocol_version")]
    pub xroad_protocol_version: String,

    #[serde(default)]
    pub client_data: ClientData,

    #[serde(default)]
    pub security_server: Option<SecurityServer>,

    /// Task 011 — request/response size caps + timeout.
    /// Applied to every request path; overrun becomes a structured
    /// 413 (inbound) or 502 (upstream) with a JSON error body.
    #[serde(default)]
    pub limits: Limits,

    /// Task 013 — WSDL folder drop. When set, XTR scans this
    /// directory at boot for `<group>/*.wsdl` files, parses each,
    /// and writes generated DSLs into `dsl_path`. Generated files
    /// carry a marker header; hand-written DSLs (no marker) win
    /// any name collision.
    #[serde(default)]
    pub wsdl_watch_dir: Option<PathBuf>,

    /// Audit-v1 — WSDL ingestion trust-boundary controls
    /// (SSRF guard on `<soap:address location=…/>` and metadata
    /// sidecar `service_url:` overrides).
    #[serde(default)]
    pub wsdl: WsdlIngest,

    /// Audit-v1 H3 — when true, upstream SOAP fault `detail` and
    /// full `faultstring` are echoed to REST callers. Default
    /// false; full detail is always kept in `tracing::warn!` for
    /// operator debugging.
    #[serde(default)]
    pub expose_soap_fault_detail: bool,
}

/// WSDL ingestion trust-boundary controls. WSDL and metadata
/// sidecar files live in an operator-controlled directory and
/// are treated as trusted for structure but never for the URL
/// they name — the audit-v1 SSRF finding was that a malicious
/// WSDL could dial the AWS metadata endpoint.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct WsdlIngest {
    /// Allow `http://` upstream URLs. Default false — public
    /// SOAP producers (Ariregister, Ministry of Climate) all
    /// serve HTTPS. Set true only in local test setups.
    #[serde(default)]
    pub allow_http_upstream: bool,

    /// Optional hostname allowlist for upstream URLs discovered
    /// in WSDLs or sidecars. Empty = no pinning (still subject
    /// to the private-IP / scheme guards). Non-empty = every
    /// upstream URL must resolve to a host on this list.
    #[serde(default)]
    pub upstream_host_allowlist: Vec<String>,
}

/// Resource ceilings. Defaults chosen for typical X-Road payload
/// sizes: real-world envelopes are single-digit KB, responses under
/// 1 MB. Ceilings are conservative but generous to survive the
/// occasional bulky business response without a config change.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Limits {
    #[serde(default = "default_max_request_bytes")]
    pub max_request_bytes: usize,

    #[serde(default = "default_max_response_bytes")]
    pub max_response_bytes: usize,

    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_request_bytes: default_max_request_bytes(),
            max_response_bytes: default_max_response_bytes(),
            request_timeout_secs: default_request_timeout_secs(),
        }
    }
}

fn default_max_request_bytes() -> usize {
    1_048_576 // 1 MiB
}
fn default_max_response_bytes() -> usize {
    16_777_216 // 16 MiB
}
fn default_request_timeout_secs() -> u64 {
    30
}

/// Client identity injected into every X-Road envelope's
/// `<xroad:client>` element (via `{{generate.client}}` in DSLs).
/// Fixes JVM bug #1 — `subsystem_code` (correctly spelled) not
/// `sybsystem-code`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ClientData {
    #[serde(default)]
    pub member_class: String,
    #[serde(default)]
    pub member_code: String,
    #[serde(default)]
    pub subsystem_code: String,
}

/// Security Server routing config. Absent → DSLs that omit
/// `service:` will error at request time (they need the Security Server).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SecurityServer {
    pub url: String,
    pub keystore_path: PathBuf,
    /// Env var name that holds the keystore password. Fixes JVM
    /// bug #16 — no default password baked into config.
    #[serde(default = "default_password_env")]
    pub keystore_password_env: String,
}

fn default_dsl_path() -> PathBuf {
    PathBuf::from("./DSL")
}
fn default_port() -> u16 {
    8080
}
fn default_xroad_instance() -> String {
    "ee-test".to_string()
}
fn default_xroad_protocol_version() -> String {
    "4.0".to_string()
}
fn default_password_env() -> String {
    "XTR_KEYSTORE_PASSWORD".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            dsl_path: default_dsl_path(),
            port: default_port(),
            xroad_instance: default_xroad_instance(),
            xroad_protocol_version: default_xroad_protocol_version(),
            client_data: ClientData::default(),
            security_server: None,
            limits: Limits::default(),
            wsdl_watch_dir: None,
            wsdl: WsdlIngest::default(),
            expose_soap_fault_detail: false,
        }
    }
}

impl AppConfig {
    /// Resolve, load, and return the operator's AppConfig. Falls
    /// back to defaults if no file is found on any conventional
    /// path. Returns `(config, source_path_or_none)` — caller logs
    /// which took effect.
    pub fn load_or_default() -> Result<(Self, Option<PathBuf>), XtrError> {
        for path in config_search_paths() {
            if path.exists() {
                let body = std::fs::read_to_string(&path).map_err(|e| {
                    XtrError::Internal(format!("reading config {}: {}", path.display(), e))
                })?;
                let cfg: AppConfig = serde_yaml_ng::from_str(&body).map_err(|e| {
                    XtrError::Internal(format!("parsing config {}: {}", path.display(), e))
                })?;
                cfg.validate()?;
                return Ok((cfg, Some(path)));
            }
        }
        Ok((Self::default(), None))
    }

    /// Audit-v1 M1 — catch typos in `xroad_protocol_version` at
    /// boot instead of failing every request cryptically at the
    /// Security Server. The accepted set is small and stable
    /// (X-Road message-protocol 4.0 has been the only shipping
    /// version for years; 4.1 is the next slot).
    pub fn validate(&self) -> Result<(), XtrError> {
        const ACCEPTED: &[&str] = &["4.0", "4.1"];
        if !ACCEPTED.contains(&self.xroad_protocol_version.as_str()) {
            return Err(XtrError::Internal(format!(
                "xroad_protocol_version '{}' is not one of the accepted values {:?}",
                self.xroad_protocol_version, ACCEPTED
            )));
        }
        Ok(())
    }

    /// Resolve the keystore password from the configured env var.
    /// Returns None if no security server is configured; errors if
    /// the Security Server is configured but the env var is unset.
    pub fn keystore_password(&self) -> Result<Option<String>, XtrError> {
        let Some(ss) = &self.security_server else {
            return Ok(None);
        };
        std::env::var(&ss.keystore_password_env)
            .map(Some)
            .map_err(|_| {
                XtrError::KeystoreLoadFailed(format!(
                    "env var {} is unset but security_server is configured",
                    ss.keystore_password_env
                ))
            })
    }
}

fn config_search_paths() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--config" && i + 1 < args.len() {
            out.push(PathBuf::from(&args[i + 1]));
            break;
        }
        if let Some(rest) = args[i].strip_prefix("--config=") {
            out.push(PathBuf::from(rest));
            break;
        }
        i += 1;
    }
    if let Ok(p) = std::env::var("XTR_CONFIG") {
        if !p.is_empty() {
            out.push(PathBuf::from(p));
        }
    }
    out.push(PathBuf::from("./xtr.yaml"));
    out.push(PathBuf::from("./xtr.yml"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with_protocol(v: &str) -> AppConfig {
        AppConfig {
            xroad_protocol_version: v.into(),
            ..Default::default()
        }
    }

    #[test]
    fn audit_m1_default_config_validates() {
        AppConfig::default().validate().unwrap();
    }

    #[test]
    fn audit_m1_typo_in_protocol_version_rejected() {
        let err = cfg_with_protocol("9.9").validate().unwrap_err();
        assert!(
            matches!(&err, XtrError::Internal(m) if m.contains("9.9") && m.contains("accepted")),
            "expected validation error naming bad value and accepted set, got {err:?}"
        );
    }

    #[test]
    fn audit_m1_accepts_4_1() {
        cfg_with_protocol("4.1").validate().unwrap();
    }

    #[test]
    fn audit_m1_empty_string_rejected() {
        assert!(cfg_with_protocol("").validate().is_err());
    }
}
