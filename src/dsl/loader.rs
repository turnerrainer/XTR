//! DSL loader — walks `config.dsl_path` and populates the service map.
//! Full implementation in Phase B.

use crate::dsl::XRoadTemplate;
use std::collections::HashMap;
use std::sync::Arc;

/// Key: `(group, service)`. Group is the parent directory name;
/// service is the filename stem (before `.y*`).
pub type ServiceMap = HashMap<(String, String), Arc<XRoadTemplate>>;
