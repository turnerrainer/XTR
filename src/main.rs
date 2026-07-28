//! XTR-on-Rust entry point.
//!
//! Assembles config → DSL loader → router → axum server.
//! Full wiring in Phase F (router) — this scaffold prints a boot
//! line and exits.

fn main() {
    tracing_subscriber::fmt::init();
    tracing::info!(
        "xtr-on-rust v{} — scaffold. Router wiring lands in Phase F.",
        env!("CARGO_PKG_VERSION")
    );
}
