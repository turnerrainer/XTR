//! DSL loading + expansion.
//!
//! DSL format is unchanged from JVM XTR (DESIGN.md §8.3):
//!
//! ```yaml
//! params:
//!   - reg_code
//! service: https://ariregxmlv6.rik.ee/    # optional — None → Security Server route
//! method: POST
//! envelope: >
//!   <soapenv:Envelope>…{{reg_code}}…</soapenv:Envelope>
//! ```

pub mod handlebars;
pub mod loader;
pub mod template;

pub use template::XRoadTemplate;
