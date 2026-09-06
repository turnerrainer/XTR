//! Dockerfile entrypoint contract test.
//!
//! Guards against the class of bug that shipped in v0.2.0-rc:
//! the Dockerfile had `ENTRYPOINT ["/usr/bin/tini", "--"]` and
//! `CMD ["/app/xtr-on-rust"]`, so `docker run <image> doctor`
//! REPLACED the CMD with `doctor` and tini tried to exec a
//! non-existent `doctor` binary. The MIGRATION.md recipe
//! (`docker run --rm turnerrainer/xtr:tag doctor`) was
//! documented as working but wasn't.
//!
//! Fix: pin the binary into ENTRYPOINT so extra args APPEND to
//! it. This test parses the Dockerfile and asserts that shape,
//! so a future edit that regresses to the old form flips red
//! before it ships to Docker Hub.

use std::fs;

#[test]
fn dockerfile_entrypoint_pins_binary_so_subcommands_append() {
    let dockerfile = fs::read_to_string("Dockerfile").expect("Dockerfile at repo root must exist");

    // ENTRYPOINT must include the binary path — otherwise
    // `docker run <image> doctor` replaces CMD and tini tries
    // to exec a `doctor` binary that doesn't exist.
    let entrypoint_line = dockerfile
        .lines()
        .find(|l| l.trim_start().starts_with("ENTRYPOINT"))
        .expect("Dockerfile must have an ENTRYPOINT directive");
    assert!(
        entrypoint_line.contains("/app/xtr-on-rust"),
        "ENTRYPOINT must include /app/xtr-on-rust so \
         `docker run <image> doctor` appends 'doctor' as an \
         arg instead of replacing CMD entirely. Got: {entrypoint_line}"
    );

    // CMD must be empty (or absent) so bare `docker run <image>`
    // runs the server (no args → server path in main.rs).
    // An old-style `CMD ["/app/xtr-on-rust"]` under the new
    // ENTRYPOINT would double the binary path in argv.
    if let Some(cmd_line) = dockerfile
        .lines()
        .find(|l| l.trim_start().starts_with("CMD"))
    {
        assert!(
            !cmd_line.contains("/app/xtr-on-rust"),
            "CMD must NOT re-name the binary — that produces \
             `tini -- /app/xtr-on-rust /app/xtr-on-rust` under \
             the new ENTRYPOINT. Got: {cmd_line}"
        );
    }
}
