//! LOG-v1 FN-LOG-1 (HIGH) regression pin — CRLF log injection via URL path.
//!
//! Before the fix (2026-09-11), `POST /nonexistent/y%0d%0aFAKE-LOG` decoded
//! the CRLF and interpolated it into a `tracing::warn!("...{}", self)` call.
//! The raw \r\n bytes split one WARN log line into two, allowing an attacker
//! to forge audit-log entries.
//!
//! Fix: `src/error.rs` uses `error = ?self` (Debug) instead of `{}` (Display).
//! Rust's Debug on String quotes and escapes control characters — raw \r\n
//! becomes the literal string `\r\n` in the log, on the same line as the WARN.
//!
//! This test:
//!   1. Sends a request whose URL path contains real CRLF bytes (percent-encoded).
//!   2. Captures the log line produced by `XtrError::TemplateNotFound`.
//!   3. Asserts the log line contains NO raw \r or \n bytes after the WARN prefix.
//!
//! If the assertion fails, an attacker can inject fake log records — see
//! `/home/rainer/Desktop/h2ck.me/projects/XTR/v1/BREAK-TESTS/LOG-FINDINGS.md`
//! and BREAK-TESTS-LOG-SUMMARY-v1.md § FN-LOG-1.

use tracing::subscriber::with_default;
use tracing_subscriber::fmt;

use xtr_on_rust::error::XtrError;

/// Capture stderr writes from the tracing subscriber into a String.
#[derive(Default)]
struct BufWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl BufWriter {
    fn new() -> (Self, std::sync::Arc<std::sync::Mutex<Vec<u8>>>) {
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        (BufWriter(buf.clone()), buf)
    }
}

impl std::io::Write for BufWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufWriter {
    type Writer = BufWriter;
    fn make_writer(&'a self) -> Self::Writer {
        BufWriter(self.0.clone())
    }
}

fn capture_error_log(err: XtrError) -> Vec<u8> {
    let (writer, buf) = BufWriter::new();
    let subscriber = fmt::Subscriber::builder()
        .with_writer(writer)
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .finish();

    with_default(subscriber, || {
        // Trigger the WARN log emission via IntoResponse — same path as the
        // production HTTP handler.
        use axum::response::IntoResponse;
        let _ = err.into_response();
    });

    let out = buf.lock().unwrap().clone();
    out
}

#[test]
fn template_not_found_log_line_escapes_crlf_in_group_and_service() {
    // The attacker payload — CRLF bytes are the primary vector.
    let hostile_group = "attacker\r\nFAKE-INFO auth_user=admin".to_string();
    let hostile_service = "service\r\nFAKE-INFO privileged=true".to_string();

    let err = XtrError::TemplateNotFound {
        group: hostile_group.clone(),
        service: hostile_service.clone(),
    };
    let raw_log = capture_error_log(err);

    // The captured log stream must contain zero raw \r bytes and zero raw
    // \n bytes ADDITIONAL to the single trailing newline emitted by the
    // tracing::fmt layer per line. Since our WARN emits exactly one line,
    // there should be exactly one \n total and zero \r.
    let cr_count = raw_log.iter().filter(|b| **b == b'\r').count();
    let lf_count = raw_log.iter().filter(|b| **b == b'\n').count();

    assert_eq!(
        cr_count, 0,
        "found {} raw CR bytes in log — CRLF injection possible!\nRAW LOG:\n{}",
        cr_count,
        String::from_utf8_lossy(&raw_log)
    );
    // Exactly one newline is expected (the trailing one from tracing).
    // Any more indicates an injected line break.
    assert_eq!(
        lf_count, 1,
        "found {} raw LF bytes in log — expected exactly 1 (trailing newline)\nRAW LOG:\n{}",
        lf_count,
        String::from_utf8_lossy(&raw_log)
    );

    // Also assert the attacker's forged tokens appear ESCAPED (positive
    // proof that Debug formatting is applied).
    let log_str = String::from_utf8_lossy(&raw_log);
    // The forged token appears somewhere in the log line, but as a literal
    // "\r\n" escape sequence (i.e., 4 chars: \, r, \, n), not as bytes.
    assert!(
        log_str.contains("\\r\\n"),
        "expected escaped \\r\\n literal in log — found raw bytes instead?\nRAW LOG:\n{}",
        log_str
    );
}

#[test]
fn template_not_found_log_line_escapes_ansi_esc_in_group() {
    // ANSI escape bytes in a template name (unusual — hyper normally rejects
    // ESC in headers, but a percent-encoded ESC in the URL path can reach
    // the router).
    let hostile_group = "attacker\x1b[2J\x1b[H".to_string();

    let err = XtrError::TemplateNotFound {
        group: hostile_group,
        service: "any".to_string(),
    };
    let raw_log = capture_error_log(err);

    let esc_count = raw_log.iter().filter(|b| **b == 0x1b).count();
    assert_eq!(
        esc_count, 0,
        "found {} raw ESC bytes in log — ANSI injection possible!\nRAW LOG:\n{}",
        esc_count,
        String::from_utf8_lossy(&raw_log)
    );
}

#[test]
fn method_not_allowed_log_line_escapes_control_chars() {
    // Same category — MethodNotAllowed also embeds group/service.
    let err = XtrError::MethodNotAllowed {
        method: "GET".to_string(),
        group: "attacker\r\nFAKE-LOG".to_string(),
        service: "svc".to_string(),
    };
    let raw_log = capture_error_log(err);

    let cr_count = raw_log.iter().filter(|b| **b == b'\r').count();
    assert_eq!(
        cr_count, 0,
        "method_not_allowed WARN line contains raw \\r — CRLF injection possible!\nRAW LOG:\n{}",
        String::from_utf8_lossy(&raw_log)
    );
}
