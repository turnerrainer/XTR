//! Full-mTLS integration test for the REST passthrough lane.
//!
//! This is the test that covers what `it_rest_passthrough.rs` cannot:
//! the shipping `RestLaneExecutor::new` code path from PKCS12 file
//! read → `reqwest::Identity::from_pkcs12_der` → mTLS handshake
//! against a real client-cert-verifying server → request delivery.
//!
//! Setup:
//! 1. `rcgen` generates a CA, a server cert (SAN=127.0.0.1) signed
//!    by that CA, and a client cert also signed by that CA.
//! 2. `openssl::pkcs12::Pkcs12Builder` packages the client cert +
//!    key into a PKCS12 file on disk (matching the shape XTR loads
//!    in production).
//! 3. `tokio-rustls` runs an mTLS-required HTTPS server on
//!    127.0.0.1:0. Client cert verification is done against the
//!    generated CA.
//! 4. `SecurityServer { keystore_path, keystore_password_env,
//!    trust_ca_path }` is written to point at the generated
//!    PKCS12 + PEM CA.
//! 5. `RestLaneExecutor::new(...)` — the production constructor —
//!    is called. If PKCS12 loading, identity attachment, or the
//!    reqwest builder are broken, this call fails.
//! 6. A request is driven through the executor. Assertions:
//!    the server received it, saw a client cert, and it was the
//!    one we packaged.
//!
//! If this test passes, the full XTR-side mTLS path works end to
//! end with a real handshake, not a mock.

use axum::http::{HeaderMap, HeaderValue, Method};
use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, KeyUsagePurpose,
    KeyPair, SanType,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use xtr_on_rust::{
    config::{AppConfig, ClientData, Limits, SecurityServer},
    dsl::{RestTarget, RestTemplate},
    executor::rest_lane::RestLaneExecutor,
};

// ---------------- Cert generation & PKCS12 packaging -----------------

struct TestPki {
    /// CA cert PEM — trust anchor for both sides.
    ca_pem: String,
    /// Server cert + key (PEM) — for the tokio-rustls acceptor.
    server_cert_pem: String,
    server_key_pem: String,
    /// Client cert + key packaged as PKCS12 bytes at `pkcs12_password`.
    pkcs12: Vec<u8>,
    pkcs12_password: String,
    /// Subject CN of the client cert — assertion target.
    client_cn: String,
}

fn generate_pki() -> TestPki {
    // 1) CA. Self-signed, marked as CA, KeyCertSign usage.
    let mut ca_params = CertificateParams::new(vec![]).expect("CA params");
    ca_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "xtr-test-ca");
        dn
    };
    ca_params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let ca_key = KeyPair::generate().expect("CA key");
    let ca_cert = ca_params.self_signed(&ca_key).expect("self-sign CA");

    // 2) Server cert signed by CA. SAN=127.0.0.1 (the address the
    //    reqwest client dials).
    let mut server_params = CertificateParams::new(vec![]).expect("server params");
    server_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "xtr-test-server");
        dn
    };
    server_params.subject_alt_names = vec![SanType::IpAddress("127.0.0.1".parse().unwrap())];
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let server_key = KeyPair::generate().expect("server key");
    let server_cert = server_params
        .signed_by(&server_key, &ca_cert, &ca_key)
        .expect("sign server cert");

    // 3) Client cert signed by CA. CN identifies the "consumer
    //    subsystem" in the mTLS handshake — we assert on it below.
    let client_cn = "xtr-consumer-test";
    let mut client_params = CertificateParams::new(vec![]).expect("client params");
    client_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, client_cn);
        dn
    };
    client_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let client_key = KeyPair::generate().expect("client key");
    let client_cert = client_params
        .signed_by(&client_key, &ca_cert, &ca_key)
        .expect("sign client cert");

    // 4) Package client cert+key as PKCS12 using the openssl crate
    //    — matches the shipping format XTR reads via
    //    `Identity::from_pkcs12_der`.
    let pkcs12_password = "test-password".to_string();
    let client_cert_pem = client_cert.pem();
    let client_key_pem = client_key.serialize_pem();
    let ca_pem = ca_cert.pem();
    let pkcs12 = build_pkcs12(
        &client_cert_pem,
        &client_key_pem,
        &ca_pem,
        &pkcs12_password,
    );

    TestPki {
        ca_pem,
        server_cert_pem: server_cert.pem(),
        server_key_pem: client_key_pem_placeholder(&server_key),
        pkcs12,
        pkcs12_password,
        client_cn: client_cn.to_string(),
    }
}

fn client_key_pem_placeholder(k: &KeyPair) -> String {
    // rcgen 0.13's KeyPair::serialize_pem is `&self` — no placeholder
    // needed; kept as a named helper for readability of generate_pki.
    k.serialize_pem()
}

/// Package an X.509 client cert + private key + CA cert into a
/// DER-encoded PKCS12 blob using the openssl crate, so
/// `reqwest::Identity::from_pkcs12_der` (the shipping loader) sees
/// exactly the format an operator would produce with `openssl
/// pkcs12 -export ...`.
fn build_pkcs12(cert_pem: &str, key_pem: &str, ca_pem: &str, password: &str) -> Vec<u8> {
    use openssl::pkcs12::Pkcs12;
    use openssl::pkey::PKey;
    use openssl::stack::Stack;
    use openssl::x509::X509;

    let cert = X509::from_pem(cert_pem.as_bytes()).expect("parse client cert PEM");
    let key = PKey::private_key_from_pem(key_pem.as_bytes()).expect("parse client key PEM");
    let ca = X509::from_pem(ca_pem.as_bytes()).expect("parse CA PEM");

    let mut chain: Stack<X509> = Stack::new().expect("new stack");
    chain.push(ca).expect("push CA");

    let mut builder = Pkcs12::builder();
    builder
        .name("xtr-client")
        .pkey(&key)
        .cert(&cert)
        .ca(chain);
    builder
        .build2(password)
        .expect("PKCS12 build")
        .to_der()
        .expect("PKCS12 → DER")
}

// ---------------- mTLS server (tokio-rustls) -----------------

/// Records what a single mTLS request looked like on the wire —
/// URL path, headers, body, and the client-cert Subject presented
/// during the handshake.
#[derive(Clone, Default)]
struct Observation {
    method: Arc<Mutex<Option<String>>>,
    path: Arc<Mutex<Option<String>>>,
    query: Arc<Mutex<Option<String>>>,
    headers: Arc<Mutex<Vec<(String, String)>>>,
    body: Arc<Mutex<Vec<u8>>>,
    client_cert_subject_cn: Arc<Mutex<Option<String>>>,
}

fn extract_cn(subject: &[u8]) -> Option<String> {
    // Parse the DER Subject Sequence looking for the CN (OID
    // 2.5.4.3). This is a minimal parse — we're extracting one
    // field from a well-formed cert, not doing full X.509
    // validation.
    // Delegate to openssl for parsing — cheap and correct.
    let x509 = openssl::x509::X509::from_der(subject).ok()?;
    let subject_entries = x509.subject_name();
    subject_entries
        .entries_by_nid(openssl::nid::Nid::COMMONNAME)
        .next()?
        .data()
        .as_slice()
        .to_vec()
        .into_iter()
        .map(char::from)
        .collect::<String>()
        .into()
}

async fn spawn_mtls_server(pki: &TestPki, obs: Observation) -> u16 {
    // 1) Build a rustls ServerConfig that REQUIRES a client cert
    //    signed by our test CA.
    let ca_der: CertificateDer<'static> =
        CertificateDer::from_pem_slice(pki.ca_pem.as_bytes()).expect("parse CA PEM");
    let mut roots = RootCertStore::empty();
    roots.add(ca_der).expect("add CA to trust store");
    let client_verifier = WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .expect("client verifier build");

    let server_certs: Vec<CertificateDer<'static>> =
        CertificateDer::pem_slice_iter(pki.server_cert_pem.as_bytes())
            .collect::<Result<_, _>>()
            .expect("parse server cert chain");
    let server_key: PrivateKeyDer<'static> =
        PrivateKeyDer::from_pem_slice(pki.server_key_pem.as_bytes()).expect("parse server key");
    let cfg = ServerConfig::builder()
        .with_client_cert_verifier(client_verifier)
        .with_single_cert(server_certs, server_key)
        .expect("server config");
    let acceptor = TlsAcceptor::from(Arc::new(cfg));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        loop {
            let (socket, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            let acceptor = acceptor.clone();
            let obs = obs.clone();
            tokio::spawn(async move {
                let stream = match acceptor.accept(socket).await {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("mTLS handshake failed: {e}");
                        return;
                    }
                };

                // Capture the client cert subject CN — proof that
                // XTR presented the identity we packaged.
                let (_, conn) = stream.get_ref();
                if let Some(peer_certs) = conn.peer_certificates() {
                    if let Some(first) = peer_certs.first() {
                        if let Some(cn) = extract_cn(first.as_ref()) {
                            *obs.client_cert_subject_cn.lock().unwrap() = Some(cn);
                        }
                    }
                }

                let mut stream = stream;
                let mut reader = BufReader::new(&mut stream);

                // Minimal HTTP/1.1 parser: read request line +
                // headers + body (Content-Length).
                let mut line = String::new();
                if reader.read_line(&mut line).await.is_err() {
                    return;
                }
                let mut it = line.split_whitespace();
                let method = it.next().unwrap_or("").to_string();
                let path_and_query = it.next().unwrap_or("").to_string();
                let (path, query) = match path_and_query.split_once('?') {
                    Some((p, q)) => (p.to_string(), Some(q.to_string())),
                    None => (path_and_query.clone(), None),
                };
                *obs.method.lock().unwrap() = Some(method);
                *obs.path.lock().unwrap() = Some(path);
                *obs.query.lock().unwrap() = query;

                let mut content_length: usize = 0;
                let mut headers = Vec::new();
                loop {
                    let mut hline = String::new();
                    if reader.read_line(&mut hline).await.is_err() {
                        return;
                    }
                    let trimmed = hline.trim_end();
                    if trimmed.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = trimmed.split_once(':') {
                        let k = k.trim().to_ascii_lowercase();
                        let v = v.trim().to_string();
                        if k == "content-length" {
                            content_length = v.parse().unwrap_or(0);
                        }
                        headers.push((k, v));
                    }
                }
                *obs.headers.lock().unwrap() = headers;

                let mut body = vec![0u8; content_length];
                if content_length > 0 {
                    let _ = reader.read_exact(&mut body).await;
                }
                *obs.body.lock().unwrap() = body;

                // Reply with a canned 200 JSON so reqwest's send()
                // resolves cleanly.
                let resp =
                    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 4\r\n\r\n{}\r\n";
                let _ = stream.write_all(resp).await;
                let _ = stream.flush().await;
            });
        }
    });

    port
}

// ---------------- The test itself -----------------

#[tokio::test]
async fn full_mtls_path_from_pkcs12_load_to_server_delivery() {
    // Rustls in this project's dependency tree can be built with
    // either aws-lc-rs or ring backend. Install a default crypto
    // provider once so the ServerConfig builder can start.
    let _ = rustls::crypto::CryptoProvider::install_default(
        rustls::crypto::aws_lc_rs::default_provider(),
    );

    // 1) Generate the PKI + write PKCS12 to disk.
    let pki = generate_pki();
    let tmp = TempDir::new().unwrap();
    let pkcs12_path = tmp.path().join("client.p12");
    std::fs::write(&pkcs12_path, &pki.pkcs12).unwrap();
    let ca_path = tmp.path().join("ca.pem");
    std::fs::write(&ca_path, pki.ca_pem.as_bytes()).unwrap();

    // 2) Spawn the mTLS-required server on 127.0.0.1:0.
    let obs = Observation::default();
    let port = spawn_mtls_server(&pki, obs.clone()).await;
    let ss_url = format!("https://127.0.0.1:{port}");

    // 3) Set the env var reqwest wants — matches production
    //    keystore_password_env resolution.
    let env_name = "XTR_MTLS_TEST_PASSWORD";
    // SAFETY: the env var name is unique per this test file.
    unsafe { std::env::set_var(env_name, &pki.pkcs12_password) };

    // 4) Build AppConfig + SecurityServer as an operator would.
    let cfg = AppConfig {
        xroad_instance: "ee-test".into(),
        client_data: ClientData {
            member_class: "GOV".into(),
            member_code: "70008440".into(),
            subsystem_code: "xtr-test".into(),
        },
        limits: Limits::default(),
        security_server: Some(SecurityServer {
            url: ss_url,
            keystore_path: pkcs12_path,
            keystore_password_env: env_name.into(),
            // XTR's default is the system trust store; here we
            // point at the test CA so the handshake succeeds.
            trust_ca_path: Some(ca_path),
        }),
        ..Default::default()
    };
    let ss = cfg.security_server.as_ref().unwrap().clone();

    // 5) Call THE production constructor. This is what
    //    `Executor::new()` does at server boot for real deployments.
    let executor = RestLaneExecutor::new(&cfg, &ss, &pki.pkcs12_password)
        .expect("RestLaneExecutor::new must succeed with valid PKCS12 + CA");

    // 6) Send a real request through it.
    let template = RestTemplate {
        target: RestTarget {
            member_class: "GOV".into(),
            member_code: "70008440".into(),
            subsystem_code: "rr".into(),
            service_code: "dde".into(),
            path: "/v1/isikud".into(),
        },
        allowed_query_params: None,
        forward_body: true,
    };
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert("x-road-userid", HeaderValue::from_static("EE38001011234"));

    let resp = executor
        .execute(
            &template,
            &Method::POST,
            vec![("personalCode".into(), "38001011234".into())],
            &headers,
            b"{\"personalCode\":\"38001011234\"}".to_vec(),
        )
        .await
        .expect("mTLS request must succeed end to end");

    assert_eq!(resp.status, 200);

    // 7) Assertions on what the server saw.
    // 7a) Client cert Subject CN — proves the PKCS12 identity
    //     rode along in the handshake.
    let cn = obs.client_cert_subject_cn.lock().unwrap().clone();
    assert_eq!(
        cn.as_deref(),
        Some(pki.client_cn.as_str()),
        "server must see our packaged client cert; got: {cn:?}",
    );

    // 7b) URL: /r1/{instance}/{class}/{code}/{sub}/{svc}{path}
    assert_eq!(
        obs.path.lock().unwrap().as_deref(),
        Some("/r1/ee-test/GOV/70008440/rr/dde/v1/isikud"),
    );
    assert_eq!(
        obs.query.lock().unwrap().as_deref(),
        Some("personalCode=38001011234"),
    );

    // 7c) X-Road headers observed on the wire (all lowercased by
    //     the capturing server).
    let hdrs = obs.headers.lock().unwrap().clone();
    let hget = |k: &str| {
        hdrs.iter()
            .find(|(hk, _)| hk == k)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(
        hget("x-road-client").as_deref(),
        Some("ee-test/GOV/70008440/xtr-test"),
    );
    assert!(
        uuid::Uuid::parse_str(hget("x-road-id").as_deref().unwrap_or("")).is_ok(),
        "x-road-id must be a UUID; got: {:?}",
        hget("x-road-id")
    );
    assert_eq!(
        hget("x-road-userid").as_deref(),
        Some("EE38001011234"),
        "user-defined X-Road header must forward",
    );
    assert_eq!(
        hget("content-type").as_deref(),
        Some("application/json"),
        "Content-Type must transport unmodified",
    );

    // 7d) Body pass-through byte-for-byte.
    assert_eq!(
        obs.body.lock().unwrap().as_slice(),
        b"{\"personalCode\":\"38001011234\"}",
    );

    // 7e) Method preserved.
    assert_eq!(obs.method.lock().unwrap().as_deref(), Some("POST"));
    drop(tmp); // hold TempDir until here so the P12 file survives
    unsafe { std::env::remove_var(env_name) };
}

#[tokio::test]
async fn ca_bundle_missing_causes_handshake_failure() {
    // Counter-test: if trust_ca_path is not configured, the system
    // trust store is used, and our rcgen CA isn't in it — the
    // handshake MUST fail. Proves that the trust_ca_path field is
    // load-bearing, not decorative.
    let _ = rustls::crypto::CryptoProvider::install_default(
        rustls::crypto::aws_lc_rs::default_provider(),
    );
    let pki = generate_pki();
    let tmp = TempDir::new().unwrap();
    let pkcs12_path = tmp.path().join("client.p12");
    std::fs::write(&pkcs12_path, &pki.pkcs12).unwrap();

    let obs = Observation::default();
    let port = spawn_mtls_server(&pki, obs).await;
    let ss_url = format!("https://127.0.0.1:{port}");

    let env_name = "XTR_MTLS_TEST_PASSWORD_2";
    unsafe { std::env::set_var(env_name, &pki.pkcs12_password) };

    let cfg = AppConfig {
        xroad_instance: "ee-test".into(),
        client_data: ClientData {
            member_class: "GOV".into(),
            member_code: "70008440".into(),
            subsystem_code: "xtr-test".into(),
        },
        limits: Limits::default(),
        security_server: Some(SecurityServer {
            url: ss_url,
            keystore_path: pkcs12_path,
            keystore_password_env: env_name.into(),
            // Deliberately absent — system trust store won't
            // trust our test CA.
            trust_ca_path: None,
        }),
        ..Default::default()
    };
    let ss = cfg.security_server.as_ref().unwrap().clone();
    let executor = RestLaneExecutor::new(&cfg, &ss, &pki.pkcs12_password).expect("build");

    let template = RestTemplate {
        target: RestTarget {
            member_class: "GOV".into(),
            member_code: "70008440".into(),
            subsystem_code: "rr".into(),
            service_code: "dde".into(),
            path: "/v1/isikud".into(),
        },
        allowed_query_params: None,
        forward_body: false,
    };
    let result = executor
        .execute(
            &template,
            &Method::GET,
            vec![],
            &HeaderMap::new(),
            Vec::new(),
        )
        .await;
    // We can't reliably assert on the error's textual form —
    // native-tls surfaces cert failures differently across
    // platforms and reqwest wraps them into "error sending
    // request for url" with no TLS keyword. The behavioural
    // assertion is enough: the request MUST fail. If a future
    // regression removes trust_ca_path handling in
    // `build_mtls_client`, this test flips green (bad) —
    // that's why the primary test above asserts the SUCCESS
    // case with trust_ca_path SET. Together the two tests
    // pin trust_ca_path as load-bearing.
    result.expect_err(
        "handshake MUST fail when trust_ca_path is unset — \
         the test CA is not in the system trust store",
    );
    unsafe { std::env::remove_var(env_name) };
}
