//! Doc-rot guard.
//!
//! An LLM (or a human reading cold) walks the repo starting at
//! `README.md` or `CLAUDE.md`. From there it has to be able to
//! reach `MIGRATION.md` and the doctor recipe in a small number
//! of hops — otherwise the audit-v1 upgrade guidance is
//! invisible.
//!
//! Each assertion locks in one navigation link. If a future
//! doc edit removes any of them, this test flips red so we
//! notice before the LLM discovery experience silently
//! regresses.

use std::fs;
use std::path::Path;

fn read(path: &str) -> String {
    fs::read_to_string(Path::new(path)).unwrap_or_else(|e| panic!("failed to read {path}: {e}"))
}

#[test]
fn readme_points_at_migration_guide() {
    let readme = read("README.md");
    assert!(
        readme.contains("MIGRATION.md"),
        "README.md must reference MIGRATION.md so LLMs and \
         operators discover the 0.1→0.2 upgrade guide"
    );
    assert!(
        readme.contains("doctor"),
        "README.md must mention the doctor subcommand so \
         readers know a config validator exists"
    );
}

#[test]
fn migration_guide_names_every_breaking_change() {
    let migration = read("MIGRATION.md");
    // Each breaking change has a stable identifier the LLM
    // needs to find. Absence = the guide is incomplete.
    for phrase in [
        "SOAP fault response shape",
        "xroad_protocol_version",
        "sidecar identity",
        "URL guard",
    ] {
        assert!(
            migration.contains(phrase),
            "MIGRATION.md must document breaking change '{phrase}'"
        );
    }
    // And every doctor code the guide's rule catalogue promises
    // must actually exist in src/doctor.rs.
    for code in [
        "fatal-config-xroad-protocol-invalid",
        "fatal-client-data-placeholder-member_code",
        "weak-wsdl-allow-http",
        "weak-wsdl-allowlist-empty",
        "weak-error-expose-soap-fault-detail",
    ] {
        assert!(
            migration.contains(code),
            "MIGRATION.md rule catalogue promises code '{code}' \
             but the string is missing"
        );
        let doctor = read("src/doctor.rs");
        assert!(
            doctor.contains(code),
            "doctor code '{code}' is documented in MIGRATION.md \
             but doesn't exist in src/doctor.rs — the catalogue \
             would mislead LLMs / CI pipelines"
        );
    }
}

#[test]
fn claude_md_exists_and_points_at_migration_and_doctor() {
    let claude = read("CLAUDE.md");
    assert!(
        claude.contains("MIGRATION.md"),
        "CLAUDE.md must reference MIGRATION.md — it's the first \
         file Claude Code reads on repo entry"
    );
    assert!(
        claude.contains("doctor"),
        "CLAUDE.md must reference the doctor subcommand"
    );
    assert!(
        claude.contains("Breaking change"),
        "CLAUDE.md must have a 'What's the breaking change surface?' \
         section so LLMs surface it in the first turn"
    );
}

#[test]
fn changelog_has_breaking_changes_section() {
    let changelog = read("CHANGELOG.md");
    assert!(
        changelog.contains("[0.2.0-rc]"),
        "CHANGELOG.md must have a [0.2.0-rc] header"
    );
    assert!(
        changelog.contains("Breaking changes vs 0.1.0-rc.2"),
        "CHANGELOG.md [0.2.0-rc] must have an explicit 'Breaking \
         changes vs 0.1.0-rc.2' subsection — grep-discoverable"
    );
}

#[test]
fn configuration_doc_lists_the_new_fields() {
    let config_doc = read("book/src/configuration.md");
    for field in [
        "allow_http_upstream",
        "upstream_host_allowlist",
        "expose_soap_fault_detail",
    ] {
        assert!(
            config_doc.contains(field),
            "book/src/configuration.md must document '{field}' — \
             an LLM reading config docs must see the new fields"
        );
    }
}

#[test]
fn security_md_cross_links_migration_and_ssrf_recipe() {
    let sec = read("SECURITY.md");
    assert!(
        sec.contains("MIGRATION.md"),
        "SECURITY.md should cross-link to MIGRATION.md"
    );
    assert!(
        sec.contains("SSRF hardening"),
        "SECURITY.md must contain the 'SSRF hardening on shared \
         WSDL mounts' operator recipe (audit-v1 C1 follow-up)"
    );
}

#[test]
fn handoff_points_at_migration_for_upgraders() {
    let handoff = read("HANDOFF.md");
    assert!(
        handoff.contains("MIGRATION.md"),
        "HANDOFF.md must cross-link to MIGRATION.md so a \
         next-contributor coming in cold sees the upgrade guide"
    );
}

#[test]
fn book_summary_lists_doctor_and_migration_entries() {
    let summary = read("book/src/SUMMARY.md");
    assert!(
        summary.contains("./doctor.md"),
        "book SUMMARY.md must include the Doctor & migration chapter"
    );
    assert!(
        summary.contains("./reference/migration.md"),
        "book SUMMARY.md must include the Migration reference entry"
    );
}
