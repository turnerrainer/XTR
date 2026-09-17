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
    // Audit LOG-v1 FN-LOG-1: emit ANSI colour codes only when stderr is
    // a TTY. Under Docker / systemd, ship plain-text logs for SIEM.
    use std::io::IsTerminal;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_ansi(std::io::stderr().is_terminal())
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

    let inter_service_token = router::inter_service_token::load_from_env();
    match &inter_service_token {
        Some(_) => tracing::info!("XTR_INTER_SERVICE_TOKEN is set — /:group/:service is bearer-gated"),
        None => tracing::info!("XTR_INTER_SERVICE_TOKEN is not set — /:group/:service is open (fine behind Ruuter; harden with the env var for direct exposure)"),
    }
    let state = router::AppState {
        cfg: Arc::new(cfg.clone()),
        services: Arc::new(services),
        executor,
        openapi_spec: Arc::new(openapi_spec),
        inter_service_token,
    };

    let app = router::build(state);
    let addr = format!("0.0.0.0:{}", cfg.port);
    tracing::info!("listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    // Fleet stronghold — graceful shutdown on SIGTERM / SIGINT.
    // axum stops accepting new connections and awaits every
    // in-flight future before returning. The handler-level
    // TimeoutLayer (request_timeout_secs + 5) bounds the drain
    // window, and Kubernetes' default terminationGracePeriodSeconds
    // (30s) comfortably covers it. Without this, SIGTERM kills the
    // process mid-request → mTLS-attributed calls left in an
    // indeterminate state upstream. Traced from h2ck.me T-20.
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    tracing::info!("shutdown complete");
    Ok(())
}

/// Await a shutdown signal — SIGINT (Ctrl-C) on all platforms and
/// SIGTERM on Unix. Emits an INFO tracing line when either fires so
/// the drain window is visible in logs / SIEM. Awaited by
/// `axum::serve(...).with_graceful_shutdown(...)`.
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!("failed to install SIGINT handler: {e}");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(e) => {
                tracing::error!("failed to install SIGTERM handler: {e}");
                // Never resolve — leaves ctrl_c as the only trigger.
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received SIGINT; initiating graceful shutdown (in-flight requests will complete within request_timeout_secs + 5s)"),
        _ = terminate => tracing::info!("received SIGTERM; initiating graceful shutdown (in-flight requests will complete within request_timeout_secs + 5s)"),
    }
}

#[cfg(all(test, unix))]
mod shutdown_tests {
    use super::shutdown_signal;
    use std::time::Duration;

    // Sends SIGTERM to the current process and asserts that
    // `shutdown_signal()` resolves promptly. Guards against a
    // future refactor that swaps the signal source or forgets to
    // gate on cfg(unix). Serialised — the signal is process-wide,
    // so a parallel test that also awaits SIGTERM would race.
    #[tokio::test]
    async fn shutdown_signal_resolves_on_sigterm() {
        let signal_fut = shutdown_signal();
        let sender = tokio::spawn(async {
            // Small delay so shutdown_signal has time to install
            // its SIGTERM handler before we raise the signal.
            tokio::time::sleep(Duration::from_millis(50)).await;
            // SAFETY: raising a signal to the current process is
            // a legal libc call; no threads read this signal state
            // outside the tokio signal handler installed above.
            unsafe {
                libc::raise(libc::SIGTERM);
            }
        });
        tokio::time::timeout(Duration::from_secs(2), signal_fut)
            .await
            .expect("shutdown_signal() did not resolve within 2s of SIGTERM");
        sender.await.unwrap();
    }
}
