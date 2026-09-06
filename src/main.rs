//! XTR-on-Rust entry point.
//!
//! Assembles: config → DSL loader → executor → OpenAPI cache →
//! router → axum server. Then serves on `0.0.0.0:<config.port>`.
//!
//! Subcommands:
//! - `xtr-on-rust`                       — normal server run
//! - `xtr-on-rust doctor [flags]`        — validate the loaded
//!   config against the audit-v1 ruleset + hardening
//!   recommendations. Exits 1 on any FATAL finding (or WEAK
//!   under `--strict`).

use std::sync::Arc;
use xtr_on_rust::{
    config::AppConfig, doctor, dsl::loader, executor::Executor, openapi, router, wsdl,
};

fn main() -> anyhow::Result<()> {
    // Parse subcommand BEFORE tokio init — `doctor` is sync and
    // doesn't need the runtime. Keeps the CLI snappy and avoids
    // spinning up a runtime we won't use.
    let cli = Cli::parse(std::env::args().skip(1));
    match cli.subcommand {
        Subcommand::Serve => run_server(),
        Subcommand::Doctor(opts) => run_doctor(opts),
    }
}

/// `xtr-on-rust doctor [--strict] [--format text|json] [--config PATH]`
struct DoctorOpts {
    strict: bool,
    format: DoctorFormat,
}

#[derive(Clone, Copy)]
enum DoctorFormat {
    Text,
    Json,
}

enum Subcommand {
    Serve,
    Doctor(DoctorOpts),
}

struct Cli {
    subcommand: Subcommand,
}

impl Cli {
    /// Consume the args iterator (excluding argv[0]) and decide
    /// the subcommand. Unknown flags on the server path are
    /// deferred to AppConfig's own arg parser via
    /// `config_search_paths` — we only claim the subcommand
    /// dispatch here.
    fn parse<I: Iterator<Item = String>>(mut args: I) -> Self {
        let Some(first) = args.next() else {
            return Self {
                subcommand: Subcommand::Serve,
            };
        };
        if first == "doctor" {
            let mut opts = DoctorOpts {
                strict: false,
                format: DoctorFormat::Text,
            };
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--strict" => opts.strict = true,
                    "--format" => {
                        if let Some(v) = args.next() {
                            opts.format = match v.as_str() {
                                "json" => DoctorFormat::Json,
                                _ => DoctorFormat::Text,
                            };
                        }
                    }
                    a if a.starts_with("--format=") => {
                        let v = a.trim_start_matches("--format=");
                        opts.format = match v {
                            "json" => DoctorFormat::Json,
                            _ => DoctorFormat::Text,
                        };
                    }
                    // --config is honoured by AppConfig::load_or_default's
                    // own arg parser; consume the next token so it
                    // doesn't leak into our own dispatch.
                    "--config" => {
                        let _ = args.next();
                    }
                    _ => { /* ignore unknown; keeps forward compat */ }
                }
            }
            return Self {
                subcommand: Subcommand::Doctor(opts),
            };
        }
        // `--help` / `-h` / anything else → fall through to the
        // server path. The server's own arg parsing will decide.
        Self {
            subcommand: Subcommand::Serve,
        }
    }
}

fn run_doctor(opts: DoctorOpts) -> anyhow::Result<()> {
    // Doctor writes to stdout in a shape consumers (LLMs, CI)
    // parse — do NOT install a tracing subscriber that would
    // dump structured logs at them. Silent unless we choose.
    let (cfg, cfg_source) = AppConfig::load_or_default()?;
    let findings = doctor::run(&cfg, cfg_source.as_deref());
    let output = match opts.format {
        DoctorFormat::Text => doctor::render_text(&findings),
        DoctorFormat::Json => doctor::render_json(&findings),
    };
    print!("{output}");
    let code = doctor::exit_code(&findings, opts.strict);
    std::process::exit(code);
}

fn run_server() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move { serve_inner().await })
}

async fn serve_inner() -> anyhow::Result<()> {
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
