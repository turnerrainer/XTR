//! `XRoadTemplate` — the parsed shape of one DSL file.
//! Full implementation in Phase B.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct XRoadTemplate {
    #[serde(default)]
    pub params: Vec<String>,
    pub service: Option<String>,
    pub method: String,
    pub envelope: String,
}
