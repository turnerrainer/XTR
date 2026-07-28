//! XTR-on-Rust configuration.
//!
//! Loaded from a YAML file at boot. Search order (matches Ruuter):
//! 1. `--config <path>` CLI flag
//! 2. `XTR_CONFIG` env var
//! 3. `./xtr.yaml` or `./xtr.yml` in the working directory
//! 4. Built-in defaults
//!
//! See DESIGN.md §8.7 for the wire shape.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// Placeholder — full implementation lands in Phase B.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppConfig {
    #[serde(default = "default_dsl_path")]
    pub dsl_path: PathBuf,
    #[serde(default = "default_port")]
    pub port: u16,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            dsl_path: default_dsl_path(),
            port: default_port(),
        }
    }
}

fn default_dsl_path() -> PathBuf {
    PathBuf::from("./DSL")
}
fn default_port() -> u16 {
    8080
}
