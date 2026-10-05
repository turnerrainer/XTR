//! Schema-aware outbound SOAP: `POST /soap-out/<group>/<name>/<operation>`.
//!
//! JSON body = content of the operation's input element (same codec
//! convention as the inbound lane: `@attr`, arrays for repeats). The
//! reply is `{"header": …|null, "response": …}` with the output
//! element decoded schema-aware. Errors use the regular `XtrError`
//! JSON shape — including `upstream_soap_fault`, whose `detail` is
//! gated by `expose_soap_fault_detail` like every other lane.

use super::codec;
use super::contract::{Operation, QName};
use super::dom::{self, Element};
use super::handler::{envelope, LaneState};
use super::{Service, XroadService};
use crate::config::{AppConfig, ClientData, SecurityServer};
use crate::error::XtrError;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use quick_xml::escape::escape;
use serde_json::{json, Value};
use std::path::Path as FsPath;
use std::time::Duration;

const SOAP11_ENV: &str = "http://schemas.xmlsoap.org/soap/envelope/";

#[derive(Debug)]
pub struct Client {
    pub url: String,
    /// True when a client certificate is presented (own keystore or
    /// the Security Server identity).
    pub client_cert: bool,
    http: reqwest::Client,
    xroad: Option<XroadService>,
    instance: String,
    protocol_version: String,
    client_data: ClientData,
}

impl Client {
    pub fn new(
        url: String,
        identity: Option<(SecurityServer, String)>,
        trust_ca: Option<&FsPath>,
        xroad: Option<XroadService>,
        cfg: &AppConfig,
    ) -> Result<Self, String> {
        let client_cert = identity.is_some();
        let http = match identity {
            Some((ss, pw)) => crate::executor::build_mtls_client(&ss, &pw, &cfg.limits)
                .map_err(|e| e.to_string())?,
            None => {
                let mut b = reqwest::Client::builder()
                    .timeout(Duration::from_secs(cfg.limits.request_timeout_secs))
                    .min_tls_version(reqwest::tls::Version::TLS_1_2)
                    .redirect(reqwest::redirect::Policy::none())
                    .no_gzip()
                    .no_brotli()
                    .no_deflate();
                if let Some(ca) = trust_ca {
                    let bytes =
                        std::fs::read(ca).map_err(|e| format!("reading {}: {e}", ca.display()))?;
                    let cert = reqwest::Certificate::from_pem(&bytes)
                        .or_else(|_| reqwest::Certificate::from_der(&bytes))
                        .map_err(|e| format!("parsing {}: {e}", ca.display()))?;
                    b = b.add_root_certificate(cert);
                }
                b.build().map_err(|e| e.to_string())?
            }
        };
        Ok(Self {
            url,
            client_cert,
            http,
            xroad,
            instance: cfg.xroad_instance.clone(),
            protocol_version: cfg.xroad_protocol_version.clone(),
            client_data: cfg.client_data.clone(),
        })
    }

    fn xroad_header(&self, target: &XroadService, op: &Operation, user_id: Option<&str>) -> String {
        let e = |s: &str| escape(s).into_owned();
        let c = &self.client_data;
        let mut h = format!(
            r#"<xrd:client xmlns:xrd="http://x-road.eu/xsd/xroad.xsd" xmlns:id="http://x-road.eu/xsd/identifiers" id:objectType="SUBSYSTEM"><id:xRoadInstance>{}</id:xRoadInstance><id:memberClass>{}</id:memberClass><id:memberCode>{}</id:memberCode><id:subsystemCode>{}</id:subsystemCode></xrd:client>"#,
            e(&self.instance),
            e(&c.member_class),
            e(&c.member_code),
            e(&c.subsystem_code)
        );
        h.push_str(&format!(
            r#"<xrd:service xmlns:xrd="http://x-road.eu/xsd/xroad.xsd" xmlns:id="http://x-road.eu/xsd/identifiers" id:objectType="SERVICE"><id:xRoadInstance>{}</id:xRoadInstance><id:memberClass>{}</id:memberClass><id:memberCode>{}</id:memberCode><id:subsystemCode>{}</id:subsystemCode><id:serviceCode>{}</id:serviceCode>{}</xrd:service>"#,
            e(&self.instance), e(&target.member_class), e(&target.member_code), e(&target.subsystem_code),
            e(&op.name),
            op.xroad_version.as_deref().map(|v| format!("<id:serviceVersion>{}</id:serviceVersion>", e(v))).unwrap_or_default()
        ));
        h.push_str(&format!(
            r#"<xrd:id xmlns:xrd="http://x-road.eu/xsd/xroad.xsd">{}</xrd:id><xrd:protocolVersion xmlns:xrd="http://x-road.eu/xsd/xroad.xsd">{}</xrd:protocolVersion>"#,
            uuid::Uuid::new_v4(),
            e(&self.protocol_version)
        ));
        if let Some(u) = user_id {
            h.push_str(&format!(
                r#"<xrd:userId xmlns:xrd="http://x-road.eu/xsd/xroad.xsd">{}</xrd:userId>"#,
                e(u)
            ));
        }
        h
    }
}

pub async fn post(
    State(st): State<LaneState>,
    Path((group, name, operation)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match call(&st, &group, &name, &operation, &headers, &body).await {
        Ok(resp) => resp,
        Err(e) => e.into_response_with_options(st.cfg.expose_soap_fault_detail),
    }
}

async fn call(
    st: &LaneState,
    group: &str,
    name: &str,
    operation: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Response, XtrError> {
    let not_found = || XtrError::TemplateNotFound {
        group: group.to_string(),
        service: format!("{name}/{operation}"),
    };
    let svc: &Service = st.registry.get(group, name).ok_or_else(not_found)?;
    let client = svc.outbound.as_ref().ok_or_else(not_found)?;
    let op = svc
        .contract
        .operations
        .iter()
        .find(|o| o.name == operation)
        .ok_or_else(not_found)?;
    let limit = st.cfg.limits.max_request_bytes;
    if body.len() > limit {
        return Err(XtrError::RequestTooLarge { limit });
    }
    let value: Value = if body.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice(body) {
            Ok(v @ Value::Object(_)) => v,
            Ok(_) => {
                return Err(XtrError::InvalidJsonBody {
                    reason: "expected a JSON object".into(),
                })
            }
            Err(e) => {
                return Err(XtrError::InvalidJsonBody {
                    reason: format!("parse error: {e}"),
                })
            }
        }
    };
    if st.offline {
        tracing::info!(op = ?op.name, "outbound SOAP blocked by XTR_OFFLINE");
        return Err(XtrError::OfflineMode);
    }
    let schema = &svc.contract.schema;
    let body_xml = codec::json_to_root_element(&op.input, &value, schema);
    let header = client.xroad.as_ref().map(|t| {
        let uid = headers.get("x-road-userid").and_then(|v| v.to_str().ok());
        client.xroad_header(t, op, uid)
    });
    let env = envelope(header.as_deref(), &body_xml);
    let action = op.soap_action.as_deref().unwrap_or("");
    let resp = client
        .http
        .post(&client.url)
        .header("content-type", "text/xml; charset=utf-8")
        .header("SOAPAction", format!("\"{action}\""))
        .body(env)
        .send()
        .await
        .map_err(crate::executor::plain::map_send_error)?;
    let status = resp.status();
    let text = crate::executor::plain::read_bounded(resp, st.cfg.limits.max_response_bytes).await?;
    if text.trim().is_empty() {
        if status.is_success() {
            // One-way operation acknowledged (typically HTTP 202).
            return Ok((
                StatusCode::ACCEPTED,
                Json(json!({"header": null, "response": null})),
            )
                .into_response());
        }
        return Err(XtrError::UpstreamHttpError {
            status: status.as_u16(),
            body: String::new(),
        });
    }
    let root = match dom::parse(&text) {
        Ok(r) => r,
        Err(e) if status.is_success() => return Err(XtrError::XmlParseError(e.0)),
        Err(_) => {
            return Err(XtrError::UpstreamHttpError {
                status: status.as_u16(),
                body: text.chars().take(1024).collect(),
            })
        }
    };
    let in_env = |e: &&Element| e.ns.as_deref() == Some(SOAP11_ENV);
    let body_el = root
        .elements()
        .filter(in_env)
        .find(|e| e.local == "Body")
        .ok_or_else(|| XtrError::XmlParseError("response has no SOAP 1.1 Body".into()))?;
    if let Some(fault) = body_el
        .elements()
        .filter(in_env)
        .find(|e| e.local == "Fault")
    {
        let txt = |n: &str| {
            fault
                .child(n)
                .map(|c| c.text().trim().to_string())
                .unwrap_or_default()
        };
        let detail = fault.child("detail").map(|d| {
            let mut obj = serde_json::Map::new();
            for c in d.elements() {
                let q = QName {
                    ns: c.ns.clone(),
                    local: c.local.clone(),
                };
                let ty = schema.element(&q).and_then(|t| schema.resolve(t));
                obj.insert(c.local.clone(), codec::element_to_json(c, ty, schema));
            }
            Value::Object(obj)
        });
        return Err(XtrError::UpstreamSoapFault {
            code: txt("faultcode"),
            string: txt("faultstring"),
            detail,
        });
    }
    if !status.is_success() {
        return Err(XtrError::UpstreamHttpError {
            status: status.as_u16(),
            body: text.chars().take(1024).collect(),
        });
    }
    let header_json = root
        .elements()
        .filter(in_env)
        .find(|e| e.local == "Header")
        .map(|h| codec::element_to_json(h, None, schema))
        .unwrap_or(Value::Null);
    let response = match body_el.elements().next() {
        Some(el) => {
            let q = QName {
                ns: el.ns.clone(),
                local: el.local.clone(),
            };
            let ty = schema.element(&q).and_then(|t| schema.resolve(t));
            codec::element_to_json(el, ty, schema)
        }
        None => Value::Null,
    };
    Ok(Json(json!({"header": header_json, "response": response})).into_response())
}
