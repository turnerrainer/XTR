//! End-to-end tests for `xtr-on-rust doctor`.
//!
//! Spawn the real binary against a tempdir-authored `xtr.yaml`
//! and assert:
//!   - stdout contains the expected finding codes
//!   - process exit code matches the spec
//!   - `--format json` produces parseable JSON
//!   - `--strict` promotes WEAK to a non-zero exit
//!
//! These tests are the doctor's own regression pins — if the
//! CLI dispatch (`main.rs::Cli::parse`) or the output format
//! ever changes, they flip red. LLMs and CI pipelines that
//! parse doctor output rely on this contract holding.

use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// Cargo sets CARGO_BIN_EXE_<binary_name> for integration
/// tests; use that so we don't guess at the target/ layout.
const BIN: &str = env!("CARGO_BIN_EXE_xtr-on-rust");

/// Author a `xtr.yaml` under a fresh tempdir and return the
/// dir. The dir is kept alive by the caller (TempDir dropped =
/// files gone).
fn tmp_with_config(contents: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("xtr.yaml"), contents).unwrap();
    tmp
}

/// Run `xtr-on-rust doctor` in `cwd` with the given extra args
/// and return `(exit_code, stdout, stderr)`.
fn run_doctor(cwd: &Path, extra_args: &[&str]) -> (i32, String, String) {
    let mut cmd = Command::new(BIN);
    cmd.arg("doctor").args(extra_args).current_dir(cwd);
    let out = cmd.output().expect("doctor binary must be runnable");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

// ---------- happy-path posture ----------

#[test]
fn hardened_config_exits_zero_and_reports_no_fatal_or_weak() {
    // Every knob dialed to the recommended posture.
    let cfg = r#"
xroad_instance: ee-test
xroad_protocol_version: "4.0"
client_data:
  member_class: GOV
  member_code: "70000000"
  subsystem_code: "myservice"
dsl_path: ./DSL
wsdl_watch_dir: ./wsdl
wsdl:
  allow_http_upstream: false
  upstream_host_allowlist:
    - ariregxmlv6.rik.ee
    - jvis.envir.ee
expose_soap_fault_detail: false
limits:
  max_request_bytes: 1048576
  max_response_bytes: 16777216
  request_timeout_secs: 30
"#;
    let tmp = tmp_with_config(cfg);
    // Materialise the paths named by the config so the
    // paths-exist check doesn't emit a WEAK finding just
    // because a fresh tempdir has neither.
    std::fs::create_dir_all(tmp.path().join("DSL")).unwrap();
    std::fs::create_dir_all(tmp.path().join("wsdl")).unwrap();
    let (code, stdout, _stderr) = run_doctor(tmp.path(), &[]);
    assert_eq!(code, 0, "hardened config should exit 0; stdout:\n{stdout}");
    assert!(
        stdout.contains("Summary: 0 FATAL, 0 BREAK, 0 WEAK"),
        "{stdout}"
    );
}

// ---------- FATAL findings ----------

#[test]
fn placeholder_member_code_flagged_fatal_exit_1() {
    let cfg = r#"
xroad_protocol_version: "4.0"
client_data:
  member_class: GOV
  member_code: "<your-registry-code>"
  subsystem_code: "myservice"
"#;
    let tmp = tmp_with_config(cfg);
    let (code, stdout, _) = run_doctor(tmp.path(), &[]);
    assert_eq!(code, 1);
    assert!(
        stdout.contains("fatal-client-data-placeholder-member_code"),
        "{stdout}"
    );
}

#[test]
fn bad_protocol_version_exits_1_when_parsed_via_serde() {
    // xroad_protocol_version has serde-level enum validation too
    // (AppConfig::validate), which fires at load. Doctor still
    // has its own check for the same value in case load bypasses
    // validation. Confirm the load-time refusal AND the doctor's
    // stdout both surface a useful message.
    let cfg = r#"
xroad_protocol_version: "9.9"
"#;
    let tmp = tmp_with_config(cfg);
    let (code, stdout, stderr) = run_doctor(tmp.path(), &[]);
    // Two possible paths:
    //   - AppConfig::load_or_default errors out at validate() → doctor
    //     prints the error to stderr and exits non-zero via anyhow.
    //   - Load succeeds (validate is lenient in some future refactor)
    //     → doctor's own check emits fatal-config-xroad-protocol-invalid.
    // Either way exit is non-zero and the bad value shows up.
    assert_ne!(code, 0);
    let combined = format!("{stdout}\n{stderr}");
    assert!(
        combined.contains("9.9"),
        "expected '9.9' in output: {combined}"
    );
}

// ---------- WEAK findings ----------

#[test]
fn weak_findings_are_not_fatal_but_strict_flips_exit_code() {
    // allow_http + empty allowlist = two weak findings, still boots.
    let cfg = r#"
xroad_protocol_version: "4.0"
client_data:
  member_class: GOV
  member_code: "70000000"
  subsystem_code: "myservice"
wsdl_watch_dir: ./wsdl
wsdl:
  allow_http_upstream: true
  upstream_host_allowlist: []
expose_soap_fault_detail: true
"#;
    let tmp = tmp_with_config(cfg);
    // Create the wsdl_watch_dir so its own weak-paths-missing
    // finding doesn't skew the count.
    std::fs::create_dir_all(tmp.path().join("wsdl")).unwrap();

    let (code, stdout, _) = run_doctor(tmp.path(), &[]);
    assert_eq!(code, 0, "weak findings alone must not exit 1");
    for want in [
        "weak-wsdl-allow-http",
        "weak-wsdl-allowlist-empty",
        "weak-error-expose-soap-fault-detail",
    ] {
        assert!(stdout.contains(want), "missing {want} in:\n{stdout}");
    }

    // --strict promotes to exit 1.
    let (code_strict, _, _) = run_doctor(tmp.path(), &["--strict"]);
    assert_eq!(
        code_strict, 1,
        "--strict must exit 1 when WEAK findings exist"
    );
}

// ---------- JSON output ----------

#[test]
fn json_format_is_parseable_and_has_expected_shape() {
    let cfg = r#"
xroad_protocol_version: "4.0"
client_data:
  member_code: "<placeholder>"
"#;
    let tmp = tmp_with_config(cfg);
    let (_, stdout, _) = run_doctor(tmp.path(), &["--format", "json"]);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("--format json must produce parseable JSON");
    let arr = parsed.as_array().expect("top-level must be array");
    assert!(!arr.is_empty());
    // First finding must have the doctor's contract fields.
    let first = &arr[0];
    for field in [
        "severity",
        "code",
        "field",
        "headline",
        "rationale",
        "recovery",
    ] {
        assert!(
            first.get(field).is_some(),
            "JSON finding missing field '{field}': {first}"
        );
    }
    // At least one FATAL for the placeholder.
    assert!(
        arr.iter().any(|f| f["severity"] == "FATAL"),
        "expected at least one FATAL: {parsed}"
    );
}

#[test]
fn json_equals_flag_is_accepted() {
    // Both `--format json` and `--format=json` must work.
    let cfg = r#"xroad_protocol_version: "4.0""#;
    let tmp = tmp_with_config(cfg);
    let (_, stdout, _) = run_doctor(tmp.path(), &["--format=json"]);
    let _: serde_json::Value = serde_json::from_str(&stdout).expect("must be JSON");
}

// ---------- exit-code contract ----------

#[test]
fn exit_code_matches_documented_contract() {
    // The MIGRATION.md doctor recipe promises a specific
    // (exit code, severity) mapping. This test locks it in.
    // 1. All-clean config → 0
    let clean = r#"
xroad_protocol_version: "4.0"
client_data:
  member_class: GOV
  member_code: "70000000"
  subsystem_code: "myservice"
wsdl:
  upstream_host_allowlist: [ariregxmlv6.rik.ee]
"#;
    let tmp1 = tmp_with_config(clean);
    assert_eq!(run_doctor(tmp1.path(), &[]).0, 0);

    // 2. Only WEAK → 0 default, 1 under --strict.
    let weak = r#"
xroad_protocol_version: "4.0"
client_data:
  member_class: GOV
  member_code: "70000000"
  subsystem_code: "myservice"
expose_soap_fault_detail: true
"#;
    let tmp2 = tmp_with_config(weak);
    assert_eq!(run_doctor(tmp2.path(), &[]).0, 0);
    assert_eq!(run_doctor(tmp2.path(), &["--strict"]).0, 1);

    // 3. FATAL → 1 always.
    let fatal = r#"
xroad_protocol_version: "4.0"
client_data:
  member_code: "<placeholder>"
"#;
    let tmp3 = tmp_with_config(fatal);
    assert_eq!(run_doctor(tmp3.path(), &[]).0, 1);
    assert_eq!(run_doctor(tmp3.path(), &["--strict"]).0, 1);
}
