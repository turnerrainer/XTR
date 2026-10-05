//! Schema-aware outbound SOAP over real mutual TLS.
//!
//! * own client certificate (`outbound.keystore_path`) to a direct peer
//!   — a non-X-Road peer that requires 2-way TLS;
//! * via the X-Road Security Server (WSDL address `TURVASERVER`) with
//!   the `security_server` identity from `xtr.yaml` + X-Road header;
//! * `TURVASERVER` without `xroad_service` → boot refused.
//!
//! The peer is a tokio-rustls server that REQUIRES a client certificate
//! signed by the test CA and records the presented CN. PKI generation
//! mirrors `it_rest_mtls.rs`; the server cert also carries
//! `DNS:localhost` because direct URLs pass the URL guard, which rejects
//! literal loopback IPs.

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose, SanType,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;
use xtr_on_rust::config::{AppConfig, ClientData, SecurityServer, WsdlIngest};
use xtr_on_rust::inbound;

const WSDL: &str = r#"<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/"
    xmlns:soap="http://schemas.xmlsoap.org/wsdl/soap/" xmlns:xs="http://www.w3.org/2001/XMLSchema"
    xmlns:xrd="http://x-road.eu/xsd/xroad.xsd" xmlns:tns="urn:demo" targetNamespace="urn:demo">
  <wsdl:types><xs:schema targetNamespace="urn:demo" elementFormDefault="qualified">
    <xs:element name="Q"><xs:complexType><xs:sequence><xs:element name="code" type="xs:string"/></xs:sequence>
      <xs:attribute name="id" type="xs:string"/></xs:complexType></xs:element>
    <xs:element name="A"><xs:complexType><xs:sequence><xs:element name="hit" type="xs:string" maxOccurs="unbounded"/></xs:sequence></xs:complexType></xs:element>
  </xs:schema></wsdl:types>
  <wsdl:message name="In"><wsdl:part name="p" element="tns:Q"/></wsdl:message>
  <wsdl:message name="Out"><wsdl:part name="p" element="tns:A"/></wsdl:message>
  <wsdl:portType name="P"><wsdl:operation name="Lookup"><wsdl:input message="tns:In"/><wsdl:output message="tns:Out"/></wsdl:operation></wsdl:portType>
  <wsdl:binding name="B" type="tns:P"><soap:binding style="document" transport="http://schemas.xmlsoap.org/soap/http"/>
    <wsdl:operation name="Lookup"><soap:operation soapAction="urn:demo/Lookup"/><xrd:version>v2</xrd:version></wsdl:operation></wsdl:binding>
  <wsdl:service name="S"><wsdl:port name="p" binding="tns:B"><soap:address location="{ADDRESS}"/></wsdl:port></wsdl:service>
</wsdl:definitions>"#;

const REPLY: &str = r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><A xmlns="urn:demo"><hit>one</hit></A></s:Body></s:Envelope>"#;

struct Pki {
    ca_pem: String,
    server_cert_pem: String,
    server_key_pem: String,
    pkcs12: Vec<u8>,
}

fn pki() -> Pki {
    let mut ca = CertificateParams::new(vec![]).unwrap();
    ca.distinguished_name = dn("xtr-soap-test-ca");
    ca.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let ca_key = KeyPair::generate().unwrap();
    let ca_cert = ca.self_signed(&ca_key).unwrap();

    let mut srv = CertificateParams::new(vec![]).unwrap();
    srv.distinguished_name = dn("peer");
    srv.subject_alt_names = vec![
        SanType::IpAddress("127.0.0.1".parse().unwrap()),
        SanType::DnsName("localhost".try_into().unwrap()),
    ];
    srv.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let srv_key = KeyPair::generate().unwrap();
    let srv_cert = srv.signed_by(&srv_key, &ca_cert, &ca_key).unwrap();

    let mut cli = CertificateParams::new(vec![]).unwrap();
    cli.distinguished_name = dn("client.example.org");
    cli.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let cli_key = KeyPair::generate().unwrap();
    let cli_cert = cli.signed_by(&cli_key, &ca_cert, &ca_key).unwrap();

    use openssl::{pkcs12::Pkcs12, pkey::PKey, stack::Stack, x509::X509};
    let mut chain = Stack::new().unwrap();
    chain
        .push(X509::from_pem(ca_cert.pem().as_bytes()).unwrap())
        .unwrap();
    let mut b = Pkcs12::builder();
    b.name("client")
        .pkey(&PKey::private_key_from_pem(cli_key.serialize_pem().as_bytes()).unwrap())
        .cert(&X509::from_pem(cli_cert.pem().as_bytes()).unwrap())
        .ca(chain);
    Pki {
        ca_pem: ca_cert.pem(),
        server_cert_pem: srv_cert.pem(),
        server_key_pem: srv_key.serialize_pem(),
        pkcs12: b.build2("pw").unwrap().to_der().unwrap(),
    }
}

fn dn(cn: &str) -> DistinguishedName {
    let mut d = DistinguishedName::new();
    d.push(DnType::CommonName, cn);
    d
}

#[derive(Clone, Default)]
struct Seen {
    cn: Arc<Mutex<Option<String>>>,
    headers: Arc<Mutex<Vec<(String, String)>>>,
    body: Arc<Mutex<String>>,
}

/// mTLS peer: requires a client cert from the test CA, answers `REPLY`.
async fn spawn_peer(p: &Pki, seen: Seen) -> u16 {
    let _ = rustls::crypto::CryptoProvider::install_default(
        rustls::crypto::aws_lc_rs::default_provider(),
    );
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(p.ca_pem.as_bytes()).unwrap())
        .unwrap();
    let cfg = ServerConfig::builder()
        .with_client_cert_verifier(
            WebPkiClientVerifier::builder(Arc::new(roots))
                .build()
                .unwrap(),
        )
        .with_single_cert(
            CertificateDer::pem_slice_iter(p.server_cert_pem.as_bytes())
                .collect::<Result<_, _>>()
                .unwrap(),
            PrivateKeyDer::from_pem_slice(p.server_key_pem.as_bytes()).unwrap(),
        )
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(cfg));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            let (acceptor, seen) = (acceptor.clone(), seen.clone());
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(sock).await else {
                    return;
                };
                if let Some(cert) = tls.get_ref().1.peer_certificates().and_then(|c| c.first()) {
                    let x = openssl::x509::X509::from_der(cert.as_ref()).unwrap();
                    let cn = x
                        .subject_name()
                        .entries_by_nid(openssl::nid::Nid::COMMONNAME)
                        .next()
                        .unwrap();
                    *seen.cn.lock().unwrap() =
                        Some(String::from_utf8_lossy(cn.data().as_slice()).into_owned());
                }
                let mut r = BufReader::new(&mut tls);
                let mut line = String::new();
                r.read_line(&mut line).await.unwrap();
                let mut len = 0usize;
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).await.unwrap();
                    let h = h.trim_end();
                    if h.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':') {
                        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_string());
                        if k == "content-length" {
                            len = v.parse().unwrap();
                        }
                        seen.headers.lock().unwrap().push((k, v));
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).await.unwrap();
                *seen.body.lock().unwrap() = String::from_utf8(body).unwrap();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/xml\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{REPLY}",
                    REPLY.len()
                );
                tls.write_all(resp.as_bytes()).await.unwrap();
                tls.flush().await.unwrap();
            });
        }
    });
    port
}

struct Lab {
    app: Router,
    _dir: TempDir,
}

fn lab(
    p: &Pki,
    address: &str,
    sidecar: &str,
    security_server: Option<SecurityServer>,
) -> Result<Lab, String> {
    let dir = TempDir::new().unwrap();
    let g = dir.path().join("demo");
    std::fs::create_dir_all(&g).unwrap();
    std::fs::write(dir.path().join("client.p12"), &p.pkcs12).unwrap();
    std::fs::write(dir.path().join("ca.pem"), &p.ca_pem).unwrap();
    std::fs::write(g.join("Svc.wsdl"), WSDL.replace("{ADDRESS}", address)).unwrap();
    std::fs::write(
        g.join("Svc.soap.yaml"),
        sidecar.replace("{DIR}", dir.path().to_str().unwrap()),
    )
    .unwrap();
    let cfg = Arc::new(AppConfig {
        client_data: ClientData {
            member_class: "GOV".into(),
            member_code: "70000000".into(),
            subsystem_code: "demo".into(),
        },
        xroad_instance: "ee-dev".into(),
        security_server: security_server.map(|mut s| {
            s.keystore_path = dir.path().join("client.p12");
            s.trust_ca_path = Some(dir.path().join("ca.pem"));
            s
        }),
        wsdl: WsdlIngest::default(),
        ..Default::default()
    });
    let registry = inbound::load_all(Some(dir.path()), &cfg)?;
    let lane = inbound::handler::LaneState::new(registry, cfg, false, None).unwrap();
    Ok(Lab {
        app: inbound::handler::outbound_router(lane),
        _dir: dir,
    })
}

async fn post(app: &Router) -> (StatusCode, Value) {
    let req = Request::post("/soap-out/demo/Svc/Lookup")
        .header("content-type", "application/json")
        .header("x-road-userid", "EE10000000001")
        .body(Body::from(
            json!({"@id": "q-1", "code": "0012"}).to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let s = resp.status();
    let b = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
}

#[tokio::test]
async fn own_client_certificate_to_direct_peer() {
    let p = pki();
    let seen = Seen::default();
    let port = spawn_peer(&p, seen.clone()).await;
    std::env::set_var("SOAP_TEST_KS_DIRECT", "pw");
    let l = lab(
        &p,
        &format!("https://localhost:{port}/peer/soap"),
        "dsl: false\noutbound:\n  keystore_path: {DIR}/client.p12\n  keystore_password_env: SOAP_TEST_KS_DIRECT\n  trust_ca_path: {DIR}/ca.pem\n",
        None,
    ).unwrap();
    let (s, v) = post(&l.app).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["response"], json!({"hit": ["one"]}));
    assert_eq!(
        seen.cn.lock().unwrap().as_deref(),
        Some("client.example.org"),
        "client cert presented"
    );
    let body = seen.body.lock().unwrap().clone();
    assert!(
        body.contains(r#"<Q xmlns="urn:demo" id="q-1"><code>0012</code></Q>"#),
        "{body}"
    );
    assert!(
        !body.contains("xrd:client"),
        "no X-Road header for a non-X-Road peer"
    );
}

#[tokio::test]
async fn via_security_server_with_xroad_header() {
    let p = pki();
    let seen = Seen::default();
    let port = spawn_peer(&p, seen.clone()).await;
    std::env::set_var("SOAP_TEST_KS_SS", "pw");
    let l = lab(
        &p,
        "http://TURVASERVER/cgi-bin/consumer_proxy",
        "dsl: false\noutbound:\n  xroad_service: {member_class: GOV, member_code: \"70000310\", subsystem_code: arireg}\n",
        Some(SecurityServer {
            url: format!("https://127.0.0.1:{port}/"),
            keystore_path: Default::default(),
            keystore_password_env: "SOAP_TEST_KS_SS".into(),
            trust_ca_path: None,
        }),
    ).unwrap();
    let (s, v) = post(&l.app).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(
        seen.cn.lock().unwrap().as_deref(),
        Some("client.example.org"),
        "Security Server identity presented"
    );
    let body = seen.body.lock().unwrap().clone();
    for needle in [
        "<id:memberCode>70000000</id:memberCode><id:subsystemCode>demo</id:subsystemCode></xrd:client>",
        "<id:memberCode>70000310</id:memberCode><id:subsystemCode>arireg</id:subsystemCode><id:serviceCode>Lookup</id:serviceCode><id:serviceVersion>v2</id:serviceVersion>",
        ">EE10000000001</xrd:userId>",
    ] {
        assert!(body.contains(needle), "missing {needle} in {body}");
    }
    let headers = seen.headers.lock().unwrap().clone();
    assert!(
        headers.contains(&("soapaction".into(), "\"urn:demo/Lookup\"".into())),
        "{headers:?}"
    );
}

#[tokio::test]
async fn security_server_target_without_xroad_service_is_refused() {
    let p = pki();
    std::env::set_var("SOAP_TEST_KS_SS2", "pw");
    let err = lab(
        &p,
        "http://TURVASERVER/cgi-bin/consumer_proxy",
        "dsl: false\noutbound: {}\n",
        Some(SecurityServer {
            url: "https://127.0.0.1:9/".into(),
            keystore_path: Default::default(),
            keystore_password_env: "SOAP_TEST_KS_SS2".into(),
            trust_ca_path: None,
        }),
    )
    .err()
    .expect("boot must be refused");
    assert!(err.contains("`outbound.xroad_service` is not set"), "{err}");
}
