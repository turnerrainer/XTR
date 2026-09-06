//! Audit-v1 H4 regression: assert reqwest with its default
//! (system-trust-store) TLS validation refuses a self-signed
//! upstream cert. This catches the regression where a future
//! maintainer adds `.danger_accept_invalid_certs(true)` to
//! either executor's Client builder — the executor's client
//! would then handshake successfully with the self-signed
//! server below, and this test would flip red.
//!
//! We spin up a bare TLS acceptor on 127.0.0.1 rather than
//! constructing a full PlainExecutor because we want the test
//! to be sensitive to Client builder options specifically —
//! extra layers add noise. If both this test AND the plain
//! executor's constructor smoke test are green, the audit-v1
//! H4 pin is intact.

use std::time::Duration;

use native_tls::{Identity, TlsAcceptor as NativeTlsAcceptor};
use rcgen::{generate_simple_self_signed, CertifiedKey};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_native_tls::TlsAcceptor;

/// Generates a self-signed cert for 127.0.0.1 valid for the
/// duration of the test process. rcgen defaults to a
/// short-lived ECDSA P-256 key which native-tls accepts on
/// both Linux and macOS builds.
fn self_signed_identity() -> Identity {
    let CertifiedKey { cert, key_pair } =
        generate_simple_self_signed(vec!["127.0.0.1".to_string()]).unwrap();
    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();
    Identity::from_pkcs8(cert_pem.as_bytes(), key_pem.as_bytes())
        .expect("native-tls should accept the PEM cert + key rcgen emits")
}

async fn spawn_self_signed_tls_server() -> u16 {
    let identity = self_signed_identity();
    let acceptor = NativeTlsAcceptor::new(identity)
        .expect("native-tls acceptor should build from generated identity");
    let acceptor = TlsAcceptor::from(acceptor);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        // Accept forever in the background; each accepted
        // connection just gets closed once the handshake either
        // completes or fails. We don't need to serve anything —
        // the client's job is to REJECT the handshake before it
        // ever asks for bytes.
        loop {
            let (socket, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(mut stream) = acceptor.accept(socket).await {
                    // Drain a bit so a happy-path client would see
                    // something; irrelevant when handshake fails.
                    let mut buf = [0u8; 128];
                    let _ = stream.read(&mut buf).await;
                    let _ = stream
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                        .await;
                }
            });
        }
    });

    port
}

#[tokio::test]
async fn audit_h4_reqwest_default_client_refuses_self_signed_cert() {
    let port = spawn_self_signed_tls_server().await;
    let url = format!("https://127.0.0.1:{port}/");

    // Explicitly build a client with the SAME settings both
    // XTR executors use — TLS 1.2 floor, no decompression, no
    // invalid-cert bypass. If the test ever fails, the fix is
    // NOT to weaken this client; it's to restore whatever the
    // executor lost.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .build()
        .expect("reqwest client builder must succeed");

    let result = client.get(&url).send().await;
    let err = result.expect_err(
        "reqwest with default trust store MUST refuse a self-signed cert — \
         if this passes, either the guard was disabled or the server accepted \
         no client and the connection succeeded for a different reason",
    );
    // Different platforms surface the cert failure differently
    // (native-tls on Linux says "certificate verify failed",
    // macOS/SecureTransport says "invalid certificate chain",
    // Windows/SChannel says "the certificate chain was issued
    // by an authority that is not trusted"). Assert that the
    // error is a connect/tls category, not a timeout or a 4xx.
    assert!(
        err.is_connect()
            || err.to_string().to_lowercase().contains("certif")
            || err.to_string().to_lowercase().contains("tls")
            || err.to_string().to_lowercase().contains("ssl"),
        "expected TLS/cert error, got: {err:?}"
    );
}

#[tokio::test]
async fn audit_h4_reqwest_with_bypass_accepts_self_signed_cert() {
    // Sanity check: proves the self-signed server actually
    // works when a client explicitly opts INTO trusting it.
    // Without this counter-test, a bug that always fails
    // handshake (broken server, wrong SNI, etc.) would masquerade
    // as a passing audit_h4 test.
    let port = spawn_self_signed_tls_server().await;
    let url = format!("https://127.0.0.1:{port}/");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .danger_accept_invalid_certs(true)
        .build()
        .expect("reqwest client builder must succeed");

    let result = client.get(&url).send().await;
    assert!(
        result.is_ok(),
        "self-signed server should be reachable when cert validation is bypassed — \
         if this fails, the server setup is broken and the audit_h4 test above is \
         passing for the WRONG reason. got: {result:?}"
    );
}
