//! XTR-on-Rust entry point.
//!
//! Assembles: config → DSL loader → executor → OpenAPI cache →
//! router → axum server. Then serves on `0.0.0.0:<config.port>`.

use std::sync::Arc;
use xtr_on_rust::{config::AppConfig, dsl::loader, executor::Executor, openapi, router, wsdl};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let version = env!("CARGO_PKG_VERSION");
    tracing::info!("xtr-on-rust v{} starting", version);

    let (cfg, cfg_source) = AppConfig::load_or_default()?;
    match cfg_source {
        Some(p) => tracing::info!("loaded config from {}", p.display()),
        None => tracing::info!("using built-in defaults (no xtr.yaml found)"),
    }

    // Task 013: ingest WSDLs from wsdl_watch_dir (if configured)
    // before the DSL loader runs — generated .yml files land under
    // cfg.dsl_path and the loader picks them up alongside any
    // hand-written DSLs.
    if let Some(watch) = &cfg.wsdl_watch_dir {
        wsdl::ingest_all(watch, &cfg.dsl_path, &cfg.wsdl, &cfg.client_data)?;
    }

    let services = loader::load_all(&cfg.dsl_path)?;
    let openapi_spec = openapi::build_spec(&services, version);
    let executor = Executor::new(&cfg)?;

    let state = router::AppState {
        cfg: Arc::new(cfg.clone()),
        services: Arc::new(services),
        executor,
        openapi_spec: Arc::new(openapi_spec),
    };

    let app = router::build(state);
    let addr = format!("0.0.0.0:{}", cfg.port);
    tracing::info!("listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
