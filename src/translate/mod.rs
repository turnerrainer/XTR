//! SOAP XML → JSON translation.
//!
//! Exposes both SOAP `<Body>` and `<Header>` sections in the JSON
//! response (fixes JVM bug #7 which extracted only Body).
//!
//! Full implementation in Phase E.

pub mod xml_to_json;
