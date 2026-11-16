//! Integration pin for #43 — a symlink cycle in the WSDL mount
//! must never produce phantom SOAP lanes or stall boot.
//!
//! The bug on `dev` before this fix: `src/inbound/mod.rs`'s
//! `sidecar_files` used `fs::read_dir` without checking for
//! symlinks or capping entries. A `loop -> .` under a group dir
//! created ~33 phantom paths (before ELOOP broke the walk at the
//! OS layer); two cycles stalled boot at ~2^32 paths. The same
//! walk pattern lived in `src/doctor.rs::count_dsl_generating_wsdls`.
//!
//! This test covers the inbound/check_all seam — the integration
//! surface a pentest would probe. The unit-level symlink behaviour
//! lives in `src/fswalk.rs`'s inline test module (9 tests across
//! cycles, out-of-root escapes, file symlinks, the entry cap, and
//! Kubernetes ConfigMap layout).

#![cfg(unix)]

use std::os::unix::fs::symlink;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use xtr_on_rust::config::AppConfig;

/// A sidecar-cycle in the mount must not create phantom lanes
/// *and* must not stall the walk.
#[test]
fn one_symlink_cycle_in_wsdl_mount_does_not_create_phantom_lanes() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // One legit lane: a sidecar + its WSDL (the sidecar may still
    // fail further validation — that's fine; we're testing walk
    // behaviour, not sidecar correctness).
    let group_dir = root.join("g");
    std::fs::create_dir_all(&group_dir).unwrap();
    std::fs::write(group_dir.join("x.soap.yaml"), b"inbound: {}\n").unwrap();
    std::fs::write(group_dir.join("x.wsdl"), b"<not-really-a-wsdl/>").unwrap();

    // The cycle: `g/loop -> .` would yield `g/loop/x.soap.yaml`,
    // `g/loop/loop/x.soap.yaml`, etc. — until the OS breaks the
    // walk with ELOOP after ~32 hops.
    symlink(".", group_dir.join("loop")).unwrap();

    let cfg = AppConfig::default();
    let start = Instant::now();
    let check = xtr_on_rust::inbound::check_all(Some(root), &cfg);
    let elapsed = start.elapsed();

    // Finite time. A pre-fix walk ran until ELOOP — ~2s on Linux
    // in practice — but a two-cycle variant stalled indefinitely.
    // 5s is huge headroom for the single-cycle case under load.
    assert!(
        elapsed < Duration::from_secs(5),
        "walk should finish fast with the fix; took {elapsed:?}"
    );

    // No phantom lanes: even with a cycle, exactly one sidecar
    // was discovered. (It may be reported as invalid because the
    // test WSDL is a stub — that's the existing lenient-sidecar
    // path; the test asserts the walker does not multiply it.)
    let sidecar_problems = check
        .problems
        .iter()
        .filter(|p| p.file.to_string_lossy().contains("x.soap.yaml"))
        .count();
    assert!(
        sidecar_problems <= 1,
        "exactly one or zero sidecar validation problems expected; got {sidecar_problems}: \
         walker created phantom copies of the sidecar through the symlink cycle"
    );
}

/// Multi-cycle variant: pre-fix this stalled boot. Must finish
/// within the same tight budget.
#[test]
fn multiple_symlink_cycles_do_not_stall_boot() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let g = root.join("g");
    std::fs::create_dir_all(&g).unwrap();
    std::fs::write(g.join("a.soap.yaml"), b"inbound: {}\n").unwrap();

    // Six cycles in one dir. Pre-fix this exploded to ~2^(6*32)
    // paths and stalled boot.
    for i in 0..6 {
        symlink(".", g.join(format!("loop{i}"))).unwrap();
        symlink("..", g.join(format!("up{i}"))).unwrap();
    }

    let cfg = AppConfig::default();
    let start = Instant::now();
    let _check = xtr_on_rust::inbound::check_all(Some(root), &cfg);
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(5),
        "multi-cycle walk should still finish fast; took {elapsed:?}"
    );
}

/// A symlink pointing outside the WSDL root must not leak
/// operator-sensitive files into the walker's view. Pre-fix the
/// walker followed `escape -> /` or `escape -> /etc` and tried to
/// read every file there as a potential sidecar / WSDL.
#[test]
fn symlink_out_of_wsdl_root_is_not_followed() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("wsdl");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(root.join("g")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();

    // One legit sidecar inside.
    std::fs::write(root.join("g/real.soap.yaml"), b"inbound: {}\n").unwrap();
    std::fs::write(root.join("g/real.wsdl"), b"<not-really-a-wsdl/>").unwrap();

    // A plausible operator-sensitive file outside the root.
    std::fs::write(outside.join("secret.soap.yaml"), b"inbound: {}\n").unwrap();

    // The escape: symlinks back out of the root.
    symlink("../../outside", root.join("g/escape")).unwrap();

    let cfg = AppConfig::default();
    let check = xtr_on_rust::inbound::check_all(Some(&root), &cfg);

    // The sidecar outside the root must not surface as a problem
    // (which would mean the walker read it).
    let touched_outside = check
        .problems
        .iter()
        .any(|p| p.file.to_string_lossy().contains("secret.soap.yaml"));
    assert!(
        !touched_outside,
        "walker must not follow a symlink out of the root; problems: {:?}",
        check.problems
    );
}
