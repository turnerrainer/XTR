//! XTR-on-Rust configuration.
//!
//! Wire shape (`xtr.yaml`, DESIGN.md §8.7):
//!
//! ```yaml
//! dsl_path: /DSL
//! port: 8080
//! xroad_instance: ee-test
//! client_data:
//!   member_class: GOV
//!   member_code: "70006317"
//!   subsystem_code: byrokratt          # correctly spelled
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

    #[serde(default)]
    pub client_data: ClientData,

    #[serde(default)]
    pub security_server: Option<SecurityServer>,
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
/// `service:` will error at request time (they need the SS).
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
fn default_password_env() -> String {
    "XTR_KEYSTORE_PASSWORD".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            dsl_path: default_dsl_path(),
            port: default_port(),
            xroad_instance: default_xroad_instance(),
            client_data: ClientData::default(),
            security_server: None,
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
                return Ok((cfg, Some(path)));
            }
        }
        Ok((Self::default(), None))
    }

    /// Resolve the keystore password from the configured env var.
    /// Returns None if no security server is configured; errors if
    /// the SS is configured but the env var is unset.
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
