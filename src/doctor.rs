//! Config validator + migration/hardening checker.
//!
//! Runs the loaded `AppConfig` through a fixed ruleset and
//! emits one `Finding` per issue. The four finding severities
//! map onto a deployment lifecycle:
//!
//! - **FATAL** — the config as-written will not boot, or would
//!   silently degrade a critical property. Fix before deploying.
//! - **BREAK** — behaviour changed vs 0.1.0-rc.2 and this config
//!   is on the losing side of that change. Set the recovery flag
//!   if you need exact-equivalence behaviour, otherwise accept
//!   the new default consciously.
//! - **WEAK** — currently allowed but a stronger posture is
//!   available. Recommended for public deployments; not enforced.
//! - **INFO** — positive observations (successful checks, useful
//!   context). Never causes a non-zero exit code.
//!
//! The exit-code contract is:
//!
//! - `exit 1` on any FATAL
//! - `exit 1` on any WEAK *when* `--strict` is passed
//! - `exit 0` otherwise
//!
//! The renderer is deliberately plain-text (no colour, no JSON
//! by default) so operators can pipe it into logs, CI reports,
//! or LLM prompts. `--format json` emits structured findings for
//! programmatic consumers.

use crate::config::AppConfig;
use serde::Serialize;
use std::fmt::Write as _;
use std::path::Path;

/// One diagnostic against the operator's config. `field` is a
/// dotted config path (e.g. `wsdl.allow_http_upstream`) so
/// automated fixers can locate the offending line.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    /// Short stable identifier, useful for grep + CI rules.
    /// Format: `<severity>-<area>-<slug>` (e.g. `weak-wsdl-allowlist-empty`).
    pub code: String,
    /// Config field this applies to (dotted path) or `None`
    /// when the finding is about missing/global state.
    pub field: Option<String>,
    /// Short, imperative headline (< 80 chars).
    pub headline: String,
    /// Multi-line rationale — the WHY, not the WHAT.
    pub rationale: String,
    /// Concrete YAML/CLI snippet the operator can copy to fix.
    pub recovery: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "UPPERCASE")]
pub enum Severity {
    Fatal,
    Break,
    Weak,
    Info,
}

impl Severity {
    fn label(self) -> &'static str {
        match self {
            Self::Fatal => "FATAL",
            Self::Break => "BREAK",
            Self::Weak => "WEAK",
            Self::Info => "INFO",
        }
    }
}

/// Run every check against `cfg`. `cfg_path` is only used for
/// human-facing output ("loaded config from …"); pass `None`
/// when running against a defaults-only config.
pub fn run(cfg: &AppConfig, cfg_path: Option<&Path>) -> Vec<Finding> {
    let mut findings = Vec::new();
    check_protocol_version(cfg, &mut findings);
    check_wsdl_url_guard_posture(cfg, &mut findings);
    check_soap_fault_detail_exposure(cfg, &mut findings);
    check_client_data_placeholders(cfg, &mut findings);
    check_security_server_env(cfg, &mut findings);
    check_rest_lane_readiness(cfg, &mut findings);
    check_limits(cfg, &mut findings);
    check_paths_exist(cfg, &mut findings);
    check_wsdl_watch_dir_writable_rootfs(cfg, &mut findings);
    check_no_caller_auth(&mut findings);
    check_offline_mode(&mut findings);
    add_context_info(cfg, cfg_path, &mut findings);
    findings
}

/// Audit RUNTIME v1 FN4 — WSDL folder-drop needs a writable DSL
/// directory, which precludes the container's rootfs being mounted
/// read-only. Emit a WEAK when `wsdl_watch_dir` is set so the
/// operator sees the trade-off explicitly (the FLEET-STRONGHOLDS §7
/// baseline is `read_only: true`; XTR opts out to support boot-time
/// DSL generation).
fn check_wsdl_watch_dir_writable_rootfs(cfg: &AppConfig, out: &mut Vec<Finding>) {
    if cfg.wsdl_watch_dir.is_some() {
        out.push(Finding {
            severity: Severity::Weak,
            code: "weak-writable-rootfs-wsdl-folder-drop".into(),
            field: Some("wsdl_watch_dir".into()),
            headline: "wsdl_watch_dir is set — container rootfs cannot be read-only".into(),
            rationale: "WSDL folder-drop regenerates DSL/*.yml under `dsl_path` on\n\
                 every boot. This precludes the fleet-baseline\n\
                 `read_only: true` container posture (FLEET-STRONGHOLDS §7)\n\
                 — an attacker with code execution inside the container\n\
                 can persist to /app because the rootfs is writable.\n\
                 The RUNTIME break-test at h2ck.me flagged this as FN4 (MED)."
                .into(),
            recovery: Some(
                "Recommended prod posture:\n\
                 1. Generate DSLs on the host: `xtr-on-rust ...` in a build\n\
                    stage that runs before the container boots.\n\
                 2. Bake the generated DSLs into a read-only volume.\n\
                 3. In xtr.yaml: `wsdl_watch_dir: null` (or omit).\n\
                 4. In compose: `read_only: true`, plus a `tmpfs: /tmp:64M`\n\
                    for scratch.\n\
                 See FLEET-STRONGHOLDS §7 for the full container hardening\n\
                 block."
                    .into(),
            ),
        });
    }
}

/// Audit PUBLIC-EXPOSURE v1 F-XTR-2 — XTR ships zero caller auth on
/// `/:group/:service`. When XTR is behind a reverse proxy (typical),
/// the proxy MUST gate the route; when XTR is exposed directly, every
/// hit becomes a real upstream call carrying the operator's mTLS
/// identity. Emit an INFO reminder so operators plan for it — this is
/// a design property, not a fixable-in-config finding.
fn check_no_caller_auth(out: &mut Vec<Finding>) {
    out.push(Finding {
        severity: Severity::Info,
        code: "info-no-caller-auth".into(),
        field: None,
        headline: "XTR has no built-in caller authentication on /:group/:service".into(),
        rationale: "Every unauth request to /:group/:service triggers an outbound\n\
             SOAP or REST call to the DSL's configured upstream, with the\n\
             operator's mTLS identity attached. Attribution attaches to the\n\
             operator for anything an attacker sends through XTR. When XTR\n\
             is deployed in Buerostack, Ruuter is the intended gate; when\n\
             XTR is deployed standalone, a reverse proxy or service mesh\n\
             MUST authenticate every hit before it reaches XTR."
            .into(),
        recovery: Some(
            "If XTR is behind Ruuter: ensure Ruuter's DSL guards cover every\n\
             route that forwards to XTR (Ruuter F-RTR-3 posture).\n\
             If XTR is standalone: place an auth-enforcing reverse proxy\n\
             (nginx auth_request, Kong, Traefik ForwardAuth, ...) in front\n\
             and never bind XTR on a public interface.\n\
             During pentest / break-test engagements: set XTR_OFFLINE=true\n\
             (see LOG-FINDINGS FN-LOG-3) so probes cannot reach real\n\
             upstreams."
                .into(),
        ),
    });
}

/// Audit LOG-v1 FN-LOG-3 — surface the XTR_OFFLINE env var state so
/// operators can't accidentally leave it enabled on a real deployment
/// (or leave it disabled during a pentest engagement without noticing).
/// Emits WEAK when active; the env-lane is the same rule
/// `Executor::new` uses at boot.
fn check_offline_mode(out: &mut Vec<Finding>) {
    let active = match std::env::var("XTR_OFFLINE") {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    };
    if active {
        out.push(Finding {
            severity: Severity::Weak,
            code: "weak-offline-mode-active".into(),
            field: Some("env:XTR_OFFLINE".into()),
            headline: "XTR_OFFLINE is active — every outbound is stubbed with HTTP 599".into(),
            rationale: "Offline / test-safety mode is intended for pentests and\n\
                        break-tests to prevent unauthorized outbound calls. Every\n\
                        SOAP + REST dispatch short-circuits with HTTP 599\n\
                        `xtr_offline`; NO real X-Road upstream is contacted."
                .into(),
            recovery: Some(
                "Unset the env var (or set it to 'false'/'0') to restore\n\
                 normal outbound behaviour. Only leave XTR_OFFLINE set for\n\
                 pentest / test-harness deployments."
                    .into(),
            ),
        });
    }
}
/// Format a `Vec<Finding>` for human consumption. Groups by
/// severity, orders FATAL → BREAK → WEAK → INFO, and prints a
/// trailing summary line with counts.
pub fn render_text(findings: &[Finding]) -> String {
    let mut out = String::new();
    let version = env!("CARGO_PKG_VERSION");
    writeln!(&mut out, "xtr-on-rust doctor — v{version}").unwrap();
    writeln!(&mut out, "{}", "-".repeat(60)).unwrap();

    for sev in [
        Severity::Fatal,
        Severity::Break,
        Severity::Weak,
        Severity::Info,
    ] {
        let bucket: Vec<&Finding> = findings.iter().filter(|f| f.severity == sev).collect();
        if bucket.is_empty() {
            continue;
        }
        writeln!(&mut out).unwrap();
        writeln!(&mut out, "{} ({})", sev.label(), bucket.len()).unwrap();
        for f in bucket {
            writeln!(&mut out, "  • [{}] {}", f.code, f.headline).unwrap();
            if let Some(field) = &f.field {
                writeln!(&mut out, "    field:    {field}").unwrap();
            }
            for line in f.rationale.lines() {
                writeln!(&mut out, "    why:      {line}").unwrap();
            }
            if let Some(recovery) = &f.recovery {
                writeln!(&mut out, "    recover:").unwrap();
                for line in recovery.lines() {
                    writeln!(&mut out, "      {line}").unwrap();
                }
            }
        }
    }

    let counts = counts(findings);
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "{}", "-".repeat(60)).unwrap();
    writeln!(
        &mut out,
        "Summary: {} FATAL, {} BREAK, {} WEAK, {} INFO",
        counts.fatal, counts.brk, counts.weak, counts.info,
    )
    .unwrap();
    out
}

/// Emit findings as JSON — for CI + programmatic consumers.
/// Stable schema: `[{"severity": "...", "code": "...", ...}]`.
pub fn render_json(findings: &[Finding]) -> String {
    serde_json::to_string_pretty(findings).unwrap_or_else(|_| "[]".to_string())
}

/// Whether the finding set should cause a non-zero exit.
/// `--strict` promotes WEAK to fatal-equivalent.
pub fn exit_code(findings: &[Finding], strict: bool) -> i32 {
    let c = counts(findings);
    if c.fatal > 0 {
        return 1;
    }
    if strict && c.weak > 0 {
        return 1;
    }
    0
}

#[derive(Default, Debug, Clone, Copy)]
pub struct Counts {
    pub fatal: usize,
    pub brk: usize,
    pub weak: usize,
    pub info: usize,
}

pub fn counts(findings: &[Finding]) -> Counts {
    let mut c = Counts::default();
    for f in findings {
        match f.severity {
            Severity::Fatal => c.fatal += 1,
            Severity::Break => c.brk += 1,
            Severity::Weak => c.weak += 1,
            Severity::Info => c.info += 1,
        }
    }
    c
}

// ---------- individual checks ----------

fn check_protocol_version(cfg: &AppConfig, out: &mut Vec<Finding>) {
    // Audit-v1 M1 enum. AppConfig::validate() would already
    // refuse to load a bad value, but doctor runs against
    // configs that are being *drafted* — surface the check
    // even when validate() wasn't called.
    const ACCEPTED: &[&str] = &["4.0", "4.1"];
    if !ACCEPTED.contains(&cfg.xroad_protocol_version.as_str()) {
        out.push(Finding {
            severity: Severity::Fatal,
            code: "fatal-config-xroad-protocol-invalid".into(),
            field: Some("xroad_protocol_version".into()),
            headline: format!(
                "xroad_protocol_version '{}' is not accepted",
                cfg.xroad_protocol_version
            ),
            rationale: format!(
                "M1 enum validation refuses any value outside {ACCEPTED:?} at boot.\n\
                 The service would fail startup with the same message."
            ),
            recovery: Some("xtr.yaml:\n  xroad_protocol_version: \"4.0\"   # or \"4.1\"".into()),
        });
    } else {
        out.push(Finding {
            severity: Severity::Info,
            code: "info-config-xroad-protocol-ok".into(),
            field: Some("xroad_protocol_version".into()),
            headline: format!(
                "xroad_protocol_version '{}' is accepted",
                cfg.xroad_protocol_version
            ),
            rationale: format!("Value is one of {ACCEPTED:?}."),
            recovery: None,
        });
    }
}

fn check_wsdl_url_guard_posture(cfg: &AppConfig, out: &mut Vec<Finding>) {
    if cfg.wsdl.allow_http_upstream {
        out.push(Finding {
            severity: Severity::Weak,
            code: "weak-wsdl-allow-http".into(),
            field: Some("wsdl.allow_http_upstream".into()),
            headline: "http:// upstreams are permitted".into(),
            rationale: "Real X-Road producers all serve HTTPS. Allowing http\n\
                        makes it possible for a compromised WSDL to route\n\
                        traffic through a plaintext MITM lane."
                .into(),
            recovery: Some("xtr.yaml:\n  wsdl:\n    allow_http_upstream: false".into()),
        });
    }
    if cfg.wsdl.upstream_host_allowlist.is_empty() {
        // Only worth warning about when wsdl_watch_dir is set —
        // otherwise there are no WSDL URLs to constrain.
        if cfg.wsdl_watch_dir.is_some() {
            out.push(Finding {
                severity: Severity::Weak,
                code: "weak-wsdl-allowlist-empty".into(),
                field: Some("wsdl.upstream_host_allowlist".into()),
                headline: "wsdl.upstream_host_allowlist is empty".into(),
                rationale: "Without a pinned host list, a WSDL that resolves\n\
                            an attacker-controlled hostname to a metadata IP\n\
                            still slips past the url_guard's literal-IP check.\n\
                            Pinning the set of upstreams closes the DNS lane."
                    .into(),
                recovery: Some(
                    "xtr.yaml:\n  wsdl:\n    upstream_host_allowlist:\n      - ariregxmlv6.rik.ee\n      - jvis.envir.ee".into(),
                ),
            });
        }
    } else {
        out.push(Finding {
            severity: Severity::Info,
            code: "info-wsdl-allowlist-pinned".into(),
            field: Some("wsdl.upstream_host_allowlist".into()),
            headline: format!(
                "{} upstream host(s) pinned",
                cfg.wsdl.upstream_host_allowlist.len()
            ),
            rationale: "DNS-rebinding lane is closed for these hosts.".into(),
            recovery: None,
        });
    }
}

fn check_soap_fault_detail_exposure(cfg: &AppConfig, out: &mut Vec<Finding>) {
    if cfg.expose_soap_fault_detail {
        out.push(Finding {
            severity: Severity::Weak,
            code: "weak-error-expose-soap-fault-detail".into(),
            field: Some("expose_soap_fault_detail".into()),
            headline: "SOAP fault detail is echoed to REST callers".into(),
            rationale: "Upstream faults can leak internal hostnames, stack\n\
                        traces, or accidentally-quoted credentials. Server\n\
                        logs already carry the full detail at warn! level."
                .into(),
            recovery: Some("xtr.yaml:\n  expose_soap_fault_detail: false".into()),
        });
    }
}

fn check_client_data_placeholders(cfg: &AppConfig, out: &mut Vec<Finding>) {
    // The shipped xtr.yaml uses placeholder text like
    // "<your-registry-code>" — if that survived into deployment,
    // sidecar identity validation (H2) will reject every real
    // sidecar and every X-Road envelope goes out with a
    // nonsense identity.
    let placeholders = [
        (
            "client_data.member_code",
            cfg.client_data.member_code.as_str(),
        ),
        (
            "client_data.subsystem_code",
            cfg.client_data.subsystem_code.as_str(),
        ),
    ];
    for (field, value) in placeholders {
        if value.contains('<') || value.contains('>') {
            out.push(Finding {
                severity: Severity::Fatal,
                code: format!(
                    "fatal-client-data-placeholder-{}",
                    field.split('.').next_back().unwrap_or("field")
                ),
                field: Some(field.into()),
                headline: format!("{field} still holds placeholder text '{value}'"),
                rationale: "Sidecar identity validation (H2) refuses to load a\n\
                            sidecar whose member_class/code/subsystem_code\n\
                            doesn't match config. A '<placeholder>' value will\n\
                            reject every real sidecar and disable those WSDLs."
                    .into(),
                recovery: Some(
                    "Set your real X-Road registration values from RIA.\n\
                     See docs/DESIGN.md §2.7 and task 006."
                        .into(),
                ),
            });
        }
    }
    if cfg.client_data.member_class.is_empty()
        && cfg.client_data.member_code.is_empty()
        && cfg.client_data.subsystem_code.is_empty()
    {
        out.push(Finding {
            severity: Severity::Weak,
            code: "weak-client-data-empty".into(),
            field: Some("client_data".into()),
            headline: "client_data is empty (X-Road envelope has no identity)".into(),
            rationale: "H2 sidecar identity check is skipped per-field when\n\
                        config is empty, so any WSDL loads — but every X-Road\n\
                        request goes out with empty <memberClass> etc, which\n\
                        the Security Server will reject at fire time."
                .into(),
            recovery: Some(
                "xtr.yaml:\n  client_data:\n    member_class: GOV\n    member_code: \"<from RIA>\"\n    subsystem_code: <your-subsystem>".into(),
            ),
        });
    }
}

fn check_security_server_env(cfg: &AppConfig, out: &mut Vec<Finding>) {
    if let Some(ss) = &cfg.security_server {
        match std::env::var(&ss.keystore_password_env) {
            Ok(v) if v.is_empty() => out.push(Finding {
                severity: Severity::Fatal,
                code: "fatal-keystore-env-empty".into(),
                field: Some("security_server.keystore_password_env".into()),
                headline: format!("env var {} is set but empty", ss.keystore_password_env),
                rationale: "Empty password will fail PKCS12 identity load;\n\
                            executor construction will error at boot."
                    .into(),
                recovery: Some(format!(
                    "export {}=<real password>",
                    ss.keystore_password_env
                )),
            }),
            Ok(_) => out.push(Finding {
                severity: Severity::Info,
                code: "info-keystore-env-present".into(),
                field: Some("security_server.keystore_password_env".into()),
                headline: format!("env var {} is set", ss.keystore_password_env),
                rationale: "PKCS12 identity load will use this password.".into(),
                recovery: None,
            }),
            Err(_) => out.push(Finding {
                severity: Severity::Fatal,
                code: "fatal-keystore-env-missing".into(),
                field: Some("security_server.keystore_password_env".into()),
                headline: format!("env var {} is not set", ss.keystore_password_env),
                rationale: "security_server is configured but the env var\n\
                            named by keystore_password_env is absent.\n\
                            AppConfig::keystore_password() would return\n\
                            KeystoreLoadFailed at boot."
                    .into(),
                recovery: Some(format!(
                    "export {}=<PKCS12 password>",
                    ss.keystore_password_env
                )),
            }),
        }
        if !ss.keystore_path.exists() {
            out.push(Finding {
                severity: Severity::Fatal,
                code: "fatal-keystore-file-missing".into(),
                field: Some("security_server.keystore_path".into()),
                headline: format!("keystore file not found: {}", ss.keystore_path.display()),
                rationale: "Executor construction reads this file at boot;\n\
                            missing path → KeystoreLoadFailed."
                    .into(),
                recovery: Some(
                    "Mount your PKCS12 keystore at the configured path, or\n\
                     update security_server.keystore_path to point at it."
                        .into(),
                ),
            });
        }
    }
}

/// Load the DSL tree and, for each REST-kind template, run the
/// spec-compliance checks that can't be done from AppConfig alone.
/// If loading fails, we emit a single WEAK finding — the loader
/// will surface the real error at boot; doctor's job is triage,
/// not enforcement.
fn check_rest_lane_readiness(cfg: &AppConfig, out: &mut Vec<Finding>) {
    let services = match crate::dsl::loader::load_all(&cfg.dsl_path) {
        Ok(m) => m,
        Err(_) => {
            // The paths check runs separately; skip when loader
            // failed for path reasons.
            return;
        }
    };

    let rest_count = services
        .values()
        .filter(|t| matches!(t.kind, crate::dsl::TemplateKind::Rest(_)))
        .count();

    if rest_count == 0 {
        return;
    }

    // 1) FATAL if REST DSLs are present but security_server isn't
    //    configured — every REST request would 500 with "REST DSL
    //    requires security_server to be configured".
    if cfg.security_server.is_none() {
        out.push(Finding {
            severity: Severity::Fatal,
            code: "fatal-rest-no-security-server".into(),
            field: Some("security_server".into()),
            headline: format!("{rest_count} REST DSL(s) loaded but security_server is unset"),
            rationale: "REST DSLs (kind: rest) always route through the\n\
                        X-Road Security Server via mTLS. Without\n\
                        security_server.url + keystore_path in xtr.yaml,\n\
                        every REST request returns 500 at fire time."
                .into(),
            recovery: Some(
                "xtr.yaml:\n  security_server:\n    url: https://out.test.x-tee.ee:443/\n    keystore_path: /app/ssl/keystore.p12\n    keystore_password_env: XTR_KEYSTORE_PASSWORD".into(),
            ),
        });
    } else {
        out.push(Finding {
            severity: Severity::Info,
            code: "info-rest-lane-ready".into(),
            field: Some("security_server".into()),
            headline: format!("{rest_count} REST DSL(s) will route via Security Server"),
            rationale: "REST passthrough active per issue #5.".into(),
            recovery: None,
        });
    }

    // 2) FATAL if the SS URL isn't HTTPS. X-Road REST §4.7 says
    //    "Secure REST services should only provide HTTPS endpoints"
    //    and mTLS requires TLS by definition.
    if let Some(ss) = &cfg.security_server {
        if !ss.url.starts_with("https://") {
            out.push(Finding {
                severity: Severity::Fatal,
                code: "fatal-rest-ss-not-https".into(),
                field: Some("security_server.url".into()),
                headline: format!("security_server.url must be https://, got '{}'", ss.url),
                rationale: "X-Road REST §4.7 requires HTTPS. Plain http would\n\
                            skip TLS entirely — no mTLS, no encryption, no\n\
                            server authentication."
                    .into(),
                recovery: Some(format!(
                    "xtr.yaml:\n  security_server:\n    url: https://<host>{}",
                    ss.url.trim_start_matches("http://")
                )),
            });
        }
        // 3) INFO — mention when trust_ca_path is absent. Not a
        //    finding per se; real X-Road SS certs are behind an
        //    operator private CA and the system trust store won't
        //    verify them.
        if ss.trust_ca_path.is_none() {
            out.push(Finding {
                severity: Severity::Info,
                code: "info-rest-trust-ca-system".into(),
                field: Some("security_server.trust_ca_path".into()),
                headline: "using system trust store for Security Server TLS".into(),
                rationale: "Real X-Road Security Server certs are typically\n\
                            issued by an operator-managed private CA. If\n\
                            the mTLS handshake fails with 'unknown issuer',\n\
                            set security_server.trust_ca_path to the CA\n\
                            bundle PEM."
                    .into(),
                recovery: None,
            });
        }
    }

    // 4) WEAK per-REST-DSL findings — enumerate spec violations.
    for ((group, service), tpl) in services.iter() {
        let crate::dsl::TemplateKind::Rest(rest) = &tpl.kind else {
            continue;
        };
        let field_prefix = format!("dsl:{}/{}", group, service);
        if !rest.target.required_fields_present() {
            out.push(Finding {
                severity: Severity::Fatal,
                code: "fatal-rest-target-fields-missing".into(),
                field: Some(format!("{field_prefix}.target")),
                headline: format!(
                    "REST DSL {group}/{service} has empty target field(s)"
                ),
                rationale: "target.member_class / .member_code / .subsystem_code\n\
                            / .service_code are all required by X-Road REST §4.1;\n\
                            missing values produce a malformed serviceId."
                    .into(),
                recovery: Some(
                    "target:\n  member_class: GOV\n  member_code: \"<from RIA>\"\n  subsystem_code: <sub>\n  service_code: <svc>".into(),
                ),
            });
        }
        if !rest.target.identifier_charset_ok() {
            out.push(Finding {
                severity: Severity::Weak,
                code: "weak-rest-identifier-charset".into(),
                field: Some(format!("{field_prefix}.target")),
                headline: format!(
                    "REST DSL {group}/{service} target contains chars outside spec §4.8"
                ),
                rationale: "X-Road REST §4.8 restricts identifiers to\n\
                            A-Za-z0-9'()+,-.=?. Non-conforming chars are\n\
                            accepted by parsing but real Security Servers\n\
                            may reject them."
                    .into(),
                recovery: Some(
                    "Restrict identifiers to the character set A-Za-z0-9'()+,-.=? per §4.8.".into(),
                ),
            });
        }
    }
}

fn check_limits(cfg: &AppConfig, out: &mut Vec<Finding>) {
    // Wildly permissive limits are a WEAK finding — real X-Road
    // envelopes are single-digit KB request / single-digit MB
    // response. 100+ MiB caps invite memory-pressure DoS.
    const REQ_HARD_CAP_MIB: usize = 16;
    const RESP_HARD_CAP_MIB: usize = 128;
    let req_mib = cfg.limits.max_request_bytes / (1024 * 1024);
    let resp_mib = cfg.limits.max_response_bytes / (1024 * 1024);
    if req_mib > REQ_HARD_CAP_MIB {
        out.push(Finding {
            severity: Severity::Weak,
            code: "weak-limits-request-too-generous".into(),
            field: Some("limits.max_request_bytes".into()),
            headline: format!(
                "max_request_bytes is {req_mib} MiB (recommend ≤ {REQ_HARD_CAP_MIB} MiB)"
            ),
            rationale: "A single request can pin this many bytes of memory\n\
                        in the buffer before validation. Real X-Road payloads\n\
                        are single-digit KB."
                .into(),
            recovery: Some("xtr.yaml:\n  limits:\n    max_request_bytes: 1048576   # 1 MiB".into()),
        });
    }
    if resp_mib > RESP_HARD_CAP_MIB {
        out.push(Finding {
            severity: Severity::Weak,
            code: "weak-limits-response-too-generous".into(),
            field: Some("limits.max_response_bytes".into()),
            headline: format!(
                "max_response_bytes is {resp_mib} MiB (recommend ≤ {RESP_HARD_CAP_MIB} MiB)"
            ),
            rationale: "Response body is buffered in memory before parsing.\n\
                        16 MiB is the shipped default; anything above 128\n\
                        MiB invites per-request memory pressure DoS."
                .into(),
            recovery: Some(
                "xtr.yaml:\n  limits:\n    max_response_bytes: 16777216  # 16 MiB".into(),
            ),
        });
    }
    if cfg.limits.request_timeout_secs > 300 {
        out.push(Finding {
            severity: Severity::Weak,
            code: "weak-limits-timeout-too-long".into(),
            field: Some("limits.request_timeout_secs".into()),
            headline: format!(
                "request_timeout_secs is {}s (recommend ≤ 300s)",
                cfg.limits.request_timeout_secs
            ),
            rationale: "Long timeouts let a slow upstream hold a connection\n\
                        slot indefinitely. Default is 30s."
                .into(),
            recovery: Some("xtr.yaml:\n  limits:\n    request_timeout_secs: 30".into()),
        });
    }
}

fn check_paths_exist(cfg: &AppConfig, out: &mut Vec<Finding>) {
    if !cfg.dsl_path.exists() {
        out.push(Finding {
            severity: Severity::Weak,
            code: "weak-paths-dsl-missing".into(),
            field: Some("dsl_path".into()),
            headline: format!("dsl_path does not exist: {}", cfg.dsl_path.display()),
            rationale: "No DSLs will be loaded; every REST request returns\n\
                        404 template_not_found. Fine if you rely entirely\n\
                        on wsdl_watch_dir generation into a fresh dir at\n\
                        boot; otherwise, create the directory."
                .into(),
            recovery: Some(format!("mkdir -p {}", cfg.dsl_path.display())),
        });
    }
    if let Some(wsdl_dir) = &cfg.wsdl_watch_dir {
        if !wsdl_dir.exists() {
            out.push(Finding {
                severity: Severity::Weak,
                code: "weak-paths-wsdl-watch-missing".into(),
                field: Some("wsdl_watch_dir".into()),
                headline: format!("wsdl_watch_dir does not exist: {}", wsdl_dir.display()),
                rationale: "WSDL ingestion is a no-op; only hand-written DSLs\n\
                            under dsl_path will be served."
                    .into(),
                recovery: Some(format!("mkdir -p {}", wsdl_dir.display())),
            });
        }
    }
}

fn add_context_info(cfg: &AppConfig, cfg_path: Option<&Path>, out: &mut Vec<Finding>) {
    if let Some(p) = cfg_path {
        out.push(Finding {
            severity: Severity::Info,
            code: "info-config-source".into(),
            field: None,
            headline: format!("config loaded from {}", p.display()),
            rationale: "Path resolved via --config → XTR_CONFIG env → \
                        ./xtr.yaml → built-in defaults."
                .into(),
            recovery: None,
        });
    } else {
        out.push(Finding {
            severity: Severity::Info,
            code: "info-config-defaults".into(),
            field: None,
            headline: "using built-in defaults (no xtr.yaml found)".into(),
            rationale: "All findings below are relative to the built-in\n\
                        defaults, not an operator-authored file."
                .into(),
            recovery: None,
        });
    }
    out.push(Finding {
        severity: Severity::Info,
        code: "info-limits-summary".into(),
        field: Some("limits".into()),
        headline: format!(
            "limits: req≤{}B  resp≤{}B  timeout={}s",
            cfg.limits.max_request_bytes,
            cfg.limits.max_response_bytes,
            cfg.limits.request_timeout_secs
        ),
        rationale: "Snapshot of the resource ceilings that will apply.".into(),
        recovery: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ClientData, Limits, SecurityServer, WsdlIngest};
    use std::path::PathBuf;

    fn empty_findings_of(sev: Severity, findings: &[Finding]) -> Vec<&Finding> {
        findings.iter().filter(|f| f.severity == sev).collect()
    }

    fn has_code(findings: &[Finding], code: &str) -> bool {
        findings.iter().any(|f| f.code == code)
    }

    #[test]
    fn default_config_has_no_fatal_findings() {
        // Defaults ship a safe posture. weak findings are ok
        // (allowlist empty, etc), fatal is not.
        let cfg = AppConfig::default();
        let findings = run(&cfg, None);
        let fatal = empty_findings_of(Severity::Fatal, &findings);
        assert!(
            fatal.is_empty(),
            "defaults should not trip any FATAL: {fatal:?}"
        );
    }

    #[test]
    fn placeholder_member_code_is_fatal() {
        let cfg = AppConfig {
            client_data: ClientData {
                member_class: "GOV".into(),
                member_code: "<your-registry-code>".into(),
                subsystem_code: "myservice".into(),
            },
            ..Default::default()
        };
        let findings = run(&cfg, None);
        assert!(has_code(
            &findings,
            "fatal-client-data-placeholder-member_code"
        ));
    }

    #[test]
    fn bad_protocol_version_is_fatal() {
        let cfg = AppConfig {
            xroad_protocol_version: "9.9".into(),
            ..Default::default()
        };
        let findings = run(&cfg, None);
        assert!(has_code(&findings, "fatal-config-xroad-protocol-invalid"));
        assert_eq!(exit_code(&findings, false), 1);
    }

    #[test]
    fn allow_http_and_empty_allowlist_produce_weak_findings() {
        let cfg = AppConfig {
            wsdl_watch_dir: Some(PathBuf::from("/tmp/xtr-doc-test-wsdl")),
            wsdl: WsdlIngest {
                allow_http_upstream: true,
                upstream_host_allowlist: vec![],
            },
            ..Default::default()
        };
        let findings = run(&cfg, None);
        assert!(has_code(&findings, "weak-wsdl-allow-http"));
        assert!(has_code(&findings, "weak-wsdl-allowlist-empty"));
        // No FATAL — this posture works, it's just wide-open.
        assert_eq!(exit_code(&findings, false), 0);
        // Strict flips WEAK to fatal.
        assert_eq!(exit_code(&findings, true), 1);
    }

    #[test]
    fn no_wsdl_watch_dir_suppresses_allowlist_warning() {
        // Without wsdl_watch_dir, no WSDLs are ingested, so the
        // allowlist is irrelevant.
        let cfg = AppConfig {
            wsdl_watch_dir: None,
            ..Default::default()
        };
        let findings = run(&cfg, None);
        assert!(!has_code(&findings, "weak-wsdl-allowlist-empty"));
    }

    #[test]
    fn expose_soap_fault_detail_is_weak() {
        let cfg = AppConfig {
            expose_soap_fault_detail: true,
            ..Default::default()
        };
        let findings = run(&cfg, None);
        assert!(has_code(&findings, "weak-error-expose-soap-fault-detail"));
    }

    #[test]
    fn missing_keystore_env_is_fatal_when_ss_configured() {
        // Guarantee the env var isn't set for the test.
        // SAFETY: single-threaded per #[test]; no other test
        // reads this specific env var by name here.
        unsafe {
            std::env::remove_var("XTR_TEST_MISSING_ENV_VAR_ABC123");
        }
        let cfg = AppConfig {
            security_server: Some(SecurityServer {
                url: "https://ss.example/".into(),
                keystore_path: PathBuf::from("/tmp/doesnt-matter-here.p12"),
                keystore_password_env: "XTR_TEST_MISSING_ENV_VAR_ABC123".into(),
                trust_ca_path: None,
            }),
            ..Default::default()
        };
        let findings = run(&cfg, None);
        assert!(has_code(&findings, "fatal-keystore-env-missing"));
        // Also flags the missing keystore file.
        assert!(has_code(&findings, "fatal-keystore-file-missing"));
    }

    #[test]
    fn oversized_limits_are_weak() {
        let cfg = AppConfig {
            limits: Limits {
                max_request_bytes: 100 * 1024 * 1024,
                max_response_bytes: 2 * 1024 * 1024 * 1024,
                request_timeout_secs: 3600,
            },
            ..Default::default()
        };
        let findings = run(&cfg, None);
        assert!(has_code(&findings, "weak-limits-request-too-generous"));
        assert!(has_code(&findings, "weak-limits-response-too-generous"));
        assert!(has_code(&findings, "weak-limits-timeout-too-long"));
    }

    #[test]
    fn render_text_contains_all_sections_when_all_severities_present() {
        let findings = vec![
            Finding {
                severity: Severity::Fatal,
                code: "test-fatal".into(),
                field: None,
                headline: "F".into(),
                rationale: "r".into(),
                recovery: None,
            },
            Finding {
                severity: Severity::Break,
                code: "test-break".into(),
                field: None,
                headline: "B".into(),
                rationale: "r".into(),
                recovery: None,
            },
            Finding {
                severity: Severity::Weak,
                code: "test-weak".into(),
                field: None,
                headline: "W".into(),
                rationale: "r".into(),
                recovery: None,
            },
            Finding {
                severity: Severity::Info,
                code: "test-info".into(),
                field: None,
                headline: "I".into(),
                rationale: "r".into(),
                recovery: None,
            },
        ];
        let text = render_text(&findings);
        assert!(text.contains("FATAL (1)"));
        assert!(text.contains("BREAK (1)"));
        assert!(text.contains("WEAK (1)"));
        assert!(text.contains("INFO (1)"));
        assert!(text.contains("Summary: 1 FATAL, 1 BREAK, 1 WEAK, 1 INFO"));
    }

    #[test]
    fn render_json_is_valid_json_array() {
        let findings = run(&AppConfig::default(), None);
        let json = render_json(&findings);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_array());
    }

    #[test]
    fn exit_code_is_zero_for_default_config() {
        let cfg = AppConfig::default();
        let findings = run(&cfg, None);
        assert_eq!(exit_code(&findings, false), 0);
    }

    // ---- REST-lane readiness checks (issue #5) ----

    fn write_rest_dsl(dir: &std::path::Path, group: &str, service: &str, yaml: &str) {
        let d = dir.join(group);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(format!("{service}.yml")), yaml).unwrap();
    }

    #[test]
    fn rest_dsl_present_but_no_security_server_is_fatal() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_rest_dsl(
            tmp.path(),
            "rr",
            "isikud",
            "kind: rest\nmethod: GET\ntarget:\n  member_class: GOV\n  member_code: '1'\n  subsystem_code: s\n  service_code: c\n",
        );
        let cfg = AppConfig {
            dsl_path: tmp.path().to_path_buf(),
            security_server: None,
            ..Default::default()
        };
        let findings = run(&cfg, None);
        assert!(has_code(&findings, "fatal-rest-no-security-server"));
    }

    #[test]
    fn rest_dsl_with_non_https_security_server_is_fatal() {
        unsafe { std::env::set_var("XTR_TEST_REST_HTTP_PW", "x") };
        let tmp = tempfile::TempDir::new().unwrap();
        write_rest_dsl(
            tmp.path(),
            "rr",
            "isikud",
            "kind: rest\nmethod: GET\ntarget:\n  member_class: GOV\n  member_code: '1'\n  subsystem_code: s\n  service_code: c\n",
        );
        let cfg = AppConfig {
            dsl_path: tmp.path().to_path_buf(),
            security_server: Some(SecurityServer {
                url: "http://ss.example/".into(),
                keystore_path: PathBuf::from("/tmp/no.p12"),
                keystore_password_env: "XTR_TEST_REST_HTTP_PW".into(),
                trust_ca_path: None,
            }),
            ..Default::default()
        };
        let findings = run(&cfg, None);
        assert!(has_code(&findings, "fatal-rest-ss-not-https"));
        unsafe { std::env::remove_var("XTR_TEST_REST_HTTP_PW") };
    }

    #[test]
    fn rest_dsl_with_bad_identifier_chars_is_weak() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_rest_dsl(
            tmp.path(),
            "rr",
            "isikud",
            "kind: rest\nmethod: GET\ntarget:\n  member_class: GOV\n  member_code: '1'\n  subsystem_code: has_underscore\n  service_code: c\n",
        );
        let cfg = AppConfig {
            dsl_path: tmp.path().to_path_buf(),
            ..Default::default()
        };
        let findings = run(&cfg, None);
        assert!(has_code(&findings, "weak-rest-identifier-charset"));
    }

    #[test]
    fn soap_only_dsl_tree_produces_no_rest_findings() {
        // Confirms that check_rest_lane_readiness stays silent
        // when no REST DSLs are loaded — no false FATAL from
        // pre-issue-#5 configs.
        let tmp = tempfile::TempDir::new().unwrap();
        write_rest_dsl(
            tmp.path(),
            "ar",
            "svc",
            "params: []\nmethod: POST\nenvelope: <x/>\n",
        );
        let cfg = AppConfig {
            dsl_path: tmp.path().to_path_buf(),
            ..Default::default()
        };
        let findings = run(&cfg, None);
        assert!(!has_code(&findings, "fatal-rest-no-security-server"));
        assert!(!has_code(&findings, "info-rest-lane-ready"));
    }

    #[test]
    fn audit_fn4_wsdl_watch_dir_triggers_weak_writable_rootfs() {
        let cfg = AppConfig {
            wsdl_watch_dir: Some(PathBuf::from("./wsdl")),
            ..Default::default()
        };
        let findings = run(&cfg, None);
        assert!(has_code(&findings, "weak-writable-rootfs-wsdl-folder-drop"));
    }

    #[test]
    fn no_wsdl_watch_dir_suppresses_writable_rootfs_finding() {
        let cfg = AppConfig {
            wsdl_watch_dir: None,
            ..Default::default()
        };
        let findings = run(&cfg, None);
        assert!(!has_code(
            &findings,
            "weak-writable-rootfs-wsdl-folder-drop"
        ));
    }

    #[test]
    fn audit_f_xtr_2_no_caller_auth_info_always_present() {
        // The reverse-proxy-gate reminder is unconditional.
        let cfg = AppConfig::default();
        let findings = run(&cfg, None);
        assert!(has_code(&findings, "info-no-caller-auth"));
    }
}
