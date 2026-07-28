//! XTR-on-Rust — REST proxy for X-Road SOAP services.
//!
//! See [`docs/DESIGN.md`](https://github.com/turnerrainer/XTR/blob/dev/docs/DESIGN.md)
//! for the domain design.

pub mod config;
pub mod dsl;
pub mod error;
pub mod executor;
pub mod openapi;
pub mod router;
pub mod translate;
pub mod wsdl;
