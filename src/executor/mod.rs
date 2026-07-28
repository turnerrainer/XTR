//! Request executor — two backends selected per-DSL:
//! * `plain` — direct HTTPS for services with `service:` set
//! * `ss` — mTLS via PKCS12 identity to the X-Road Security Server
//!
//! Full implementation in Phase D.

pub mod plain;
pub mod ss;
