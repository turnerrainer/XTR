//! XSD validation of raw-XML lane messages, in pure Rust.
//!
//! Own validation model, built from the XSD documents that the lane
//! loads. The codec model in `crate::inbound::contract` stays a codec
//! hint and is not changed. See the ADR in `book/src/`.
//!
//! The model covers a small, fixed subset of XSD 1.0. A construct
//! outside the subset that a message root reaches (also through type
//! inheritance, attribute groups or includes) stops the boot.

mod builder;
mod model;
mod pattern;
mod upa;
mod validate;

pub use model::{
    build, AttributeDecl, Builtin, ComplexType, ElementDecl, Model, Occurs, Particle, Problem,
    SimpleType, Step, TypeDef, WhiteSpace,
};
pub use pattern::compile as compile_pattern;
pub use validate::{validate, Violation, MAX_VIOLATIONS};
