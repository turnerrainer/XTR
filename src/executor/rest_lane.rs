//! X-Road REST passthrough executor — issue #5.
//!
//! Implements the client side of the
//! [X-Road Message Protocol for REST v1.0.4](https://github.com/nordic-institute/X-Road/blob/develop/doc/Protocols/pr-rest_x-road_message_protocol_for_rest.md).
//! XTR is the "consumer information system" per §1.1 of the spec —
//! it talks to a local X-Road Security Server over mTLS and lets
//! the Security Server relay the request to the provider Security
//! Server and onwards to the provider service.
//!
//! Wire behaviour (spec-checked, section references throughout):
//!
//! * **URL shape §4.1** — `/r1/{instance}/{class}/{code}/{subsystem}/{service_code}[{path}]`.
//!   Note: versioning ("v1") is part of `[path]` — the spec does
//!   not carve out a serviceVersion segment inside serviceId.
//! * **Percent-encoding §4.2** — every identifier segment is
//!   percent-encoded as UTF-8. The `/` separators between segments
//!   stay literal.
//! * **`X-Road-Client` §4.3** — mandatory; format
//!   `{instance}/{class}/{code}/{subsystem}` with identifier
//!   segments percent-encoded.
//! * **`X-Road-Id` §4.3** — optional. If the caller sets one we
//!   forward it verbatim (allows callers to correlate their own
//!   trace); otherwise we generate a fresh UUID.
//! * **Content-Type / Accept / Cache-Control / user-defined headers
//!   §4.3** — passed unmodified.
//! * **Filtered headers §4.3** — hop-by-hop headers (Connection,
//!   Keep-Alive, TE, Trailer, Transfer-Encoding, Upgrade,
//!   Proxy-*), Host, and any inbound X-Road-Client are stripped.
//! * **Redirects §4.4** — `Policy::none()` on the reqwest client.
//! * **Query params §4.5** — passed unmodified by default; DSL
//!   MAY narrow via `allowed_query_params`.
//! * **Response headers §4.3** — provider Security Server sets
//!   `X-Road-Service`, `X-Road-Request-Hash`, `X-Road-Error` etc.
//!   The executor returns the full HeaderMap so the router can
//!   forward them to the consumer.

use crate::config::{AppConfig, SecurityServer};
use crate::dsl::{RestTarget, RestTemplate};
use crate::error::XtrError;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use reqwest::{redirect::Policy, Client, Identity};
use std::time::Duration;
use uuid::Uuid;

use super::plain::map_send_error;

/// Percent-encoding set for X-Road identifier segments per REST
/// §4.2. The spec's `[RFC3986]` reference means every non-safe
/// character MUST be percent-encoded.
///
/// Design decision: encode everything except RFC 3986 "unreserved"
/// characters (`A-Za-z0-9-._~`). This is stricter than §4.8
/// implies — §4.8 permits `'()+,-.=?` inside identifier *values*,
/// but several of those (`+ ? =`) have URL-level special meaning
/// (space encoding, query-string boundary, key/value separator).
/// Encoding them defensively means the identifier survives every
/// URL parser identically.
///
/// The `/` separator between segments is applied by the URL
/// builder, not this encoder — so a service_code literally
/// containing `/` (spec §4.2 example: "BAR/SERVICE") gets encoded
/// to `BAR%2FSERVICE` as required.
const IDENTIFIER_ALLOWED: AsciiSet = NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Headers we NEVER copy from the inbound REST request to the
/// outbound X-Road request. Names are compared case-insensitively
/// (via HeaderName equality). See spec §4.3 "Filtered headers".
const DENY_LIST_FORWARD: &[&str] = &[
    // Hop-by-hop headers per RFC 7230 §6.1. reqwest re-emits its
    // own for these; passing the client's would confuse the
    // connection manager.
    "connection",
    "keep-alive",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "proxy-authenticate",
    "proxy-authorization",
    // Host is set by reqwest from the URL. Forwarding the caller's
    // Host to the upstream would leak the XTR public hostname to
    // the Security Server and could break TLS SNI. Spec §4.3
    // lists Host among "headers that can leak the name or address
    // of the origin host".
    "host",
    // Content-Length is managed by reqwest based on the actual
    // body bytes; passing the inbound value would poison the
    // outbound wire shape.
    "content-length",
    // XTR overrides these — inbound values would confuse the SS.
    "x-road-client",
];

/// Response headers we don't forward from upstream to consumer.
/// Same hop-by-hop set. Content-Length is left to axum's body
/// pipeline.
const DENY_LIST_RESPONSE: &[&str] = &[
    "connection",
    "keep-alive",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "proxy-authenticate",
    "proxy-authorization",
    "content-length",
];

/// One upstream response, ready to be turned into an axum
/// `Response` by the router with content-type, all response
/// headers, and status preserved.
#[derive(Debug)]
pub struct RestUpstreamResponse {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

#[derive(Clone)]
pub struct RestLaneExecutor {
    client: Client,
    base_url: String,
    xroad_instance: String,
    client_header: String,
    max_response_bytes: usize,
}

impl RestLaneExecutor {
    /// Build the production REST executor: reads the operator's
    /// PKCS12 keystore, constructs the mTLS client, formats the
    /// `X-Road-Client` header value from the config's client_data.
    pub fn new(cfg: &AppConfig, ss: &SecurityServer, password: &str) -> Result<Self, XtrError> {
        let client = super::build_mtls_client(ss, password, &cfg.limits)?;
        let client_header = format_client_header(&cfg.xroad_instance, cfg);
        tracing::info!(
            keystore = %ss.keystore_path.display(),
            ss_url = %ss.url,
            xroad_client = %client_header,
            "REST-lane executor initialised"
        );
        Ok(Self {
            client,
            base_url: ss.url.trim_end_matches('/').to_string(),
            xroad_instance: cfg.xroad_instance.clone(),
            client_header,
            max_response_bytes: cfg.limits.max_response_bytes,
        })
    }

    /// Test-only constructor: assemble the executor from an
    /// already-built reqwest client, letting integration tests
    /// point at a plain-HTTP mock without provisioning PKCS12.
    #[doc(hidden)]
    pub fn __from_parts_for_tests(
        client: Client,
        base_url: String,
        xroad_instance: String,
        client_header: String,
        max_response_bytes: usize,
    ) -> Self {
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            xroad_instance,
            client_header,
            max_response_bytes,
        }
    }

    /// Forward an inbound REST request to the Security Server.
    ///
    /// * `method` — the HTTP method (already validated against the
    ///   DSL's declared method by the router).
    /// * `template.target` — the target service identity.
    /// * `template.allowed_query_params` — `None` forwards every
    ///   inbound query key (spec default); `Some(vec)` filters.
    /// * `query_pairs` — parsed from the inbound URL by axum's
    ///   `Query<Vec<(String, String)>>` extractor.
    /// * `inbound_headers` — the axum request headers. Forwarded
    ///   after applying `DENY_LIST_FORWARD` and injecting the
    ///   mandatory X-Road-* headers.
    /// * `body` — inbound request body bytes.
    pub async fn execute(
        &self,
        template: &RestTemplate,
        method: &Method,
        query_pairs: Vec<(String, String)>,
        inbound_headers: &HeaderMap,
        body: Vec<u8>,
    ) -> Result<RestUpstreamResponse, XtrError> {
        let url = build_url(
            &self.base_url,
            &self.xroad_instance,
            &template.target,
            &filter_query(template.allowed_query_params.as_deref(), query_pairs),
        );

        let mut req = self.client.request(method.clone(), &url);

        // 1) Forward inbound headers with deny-list applied. This
        //    covers Content-Type, Accept, Cache-Control,
        //    user-defined headers, X-Road-UserId, X-Road-Issue,
        //    X-Road-Represented-Party, etc. per spec §4.3.
        for (name, value) in inbound_headers.iter() {
            let n = name.as_str().to_ascii_lowercase();
            if DENY_LIST_FORWARD.iter().any(|d| *d == n) {
                continue;
            }
            req = req.header(name.clone(), value.clone());
        }

        // 2) Overwrite / inject the X-Road-* headers XTR owns.
        //    X-Road-Client is XTR's identity — always ours.
        req = req.header("X-Road-Client", &self.client_header);

        // 3) X-Road-Id: forward inbound if the caller set one
        //    (spec §4.3: optional; if not provided, the consumer
        //    Security Server SHALL generate). We generate here as
        //    the consumer information system so operators see the
        //    same id in XTR logs as in SS logs.
        if inbound_headers.get("X-Road-Id").is_none() {
            req = req.header("X-Road-Id", Uuid::new_v4().to_string());
        }

        // 4) Body. Forward verbatim when the DSL enables it, even
        //    if empty and the inbound Content-Type is set — that
        //    Content-Type came in via header pass-through above.
        if template.forward_body {
            req = req.body(body);
        }

        tracing::debug!(
            method = %method,
            url = %url,
            "REST-lane outbound request"
        );
        let resp = req.send().await.map_err(map_send_error)?;

        let status = resp.status().as_u16();
        let mut headers = HeaderMap::new();
        for (name, value) in resp.headers().iter() {
            let n = name.as_str().to_ascii_lowercase();
            if DENY_LIST_RESPONSE.iter().any(|d| *d == n) {
                continue;
            }
            if let (Ok(hn), Ok(hv)) = (
                HeaderName::from_bytes(name.as_str().as_bytes()),
                HeaderValue::from_bytes(value.as_bytes()),
            ) {
                headers.insert(hn, hv);
            }
        }

        let body = read_bounded_bytes(resp, self.max_response_bytes).await?;
        Ok(RestUpstreamResponse {
            status,
            headers,
            body,
        })
    }
}

/// Build the mTLS client shared by the SOAP Security Server lane
/// (`SecurityServerExecutor`) and the REST lane. Exposed on the
/// `executor` module — see `super::build_mtls_client`.
///
/// Placed here for documentation locality; the actual definition
/// lives in `executor/mod.rs` alongside the `Executor` struct.
///
/// This module-level doc-only reference is a legacy convenience —
/// see `super::build_mtls_client` for the implementation.
///
/// Configuration:
/// * `redirect(Policy::none())` per spec §4.4 — X-Road never
///   follows redirects; XTR passes them to the caller unmodified.
/// * `min_tls_version(TLS_1_2)` — audit-v1 H4.
/// * `no_gzip/brotli/deflate` — audit-v1 M2 keeps response cap
///   accounting honest.
/// * `add_root_certificate` when `security_server.trust_ca_path`
///   is set — real X-Road SS certs are behind an operator's
///   private CA that isn't in the system trust store.
#[allow(dead_code)]
fn _mtls_client_docs() {}

/// Format the X-Road-Client header per spec §4.3:
/// `{instance}/{class}/{code}/{subsystem}` with each segment
/// percent-encoded.
///
/// Spec allows omitting the subsystem, but XTR always emits one
/// because `client_data.subsystem_code` is required by the SOAP
/// lane already — degrading it to optional here would be
/// asymmetric.
fn format_client_header(instance: &str, cfg: &AppConfig) -> String {
    format!(
        "{}/{}/{}/{}",
        pct(instance),
        pct(&cfg.client_data.member_class),
        pct(&cfg.client_data.member_code),
        pct(&cfg.client_data.subsystem_code),
    )
}

/// Build the X-Road REST URL per spec §4.1:
///
/// ```text
/// <ss>/r1/<instance>/<class>/<code>/<subsystem>/<service_code>[<path>][?query]
/// ```
///
/// The path segment is joined without an extra `/` when it already
/// starts with one; if the DSL omitted the leading `/`, we insert
/// it. Identifier segments are percent-encoded per §4.2.
fn build_url(
    base_url: &str,
    instance: &str,
    target: &RestTarget,
    query_pairs: &[(String, String)],
) -> String {
    let mut url = format!(
        "{}/r1/{}/{}/{}/{}/{}",
        base_url,
        pct(instance),
        pct(&target.member_class),
        pct(&target.member_code),
        pct(&target.subsystem_code),
        pct(&target.service_code),
    );
    if !target.path.is_empty() {
        if !target.path.starts_with('/') {
            url.push('/');
        }
        url.push_str(&target.path);
    }
    if !query_pairs.is_empty() {
        let mut ser = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in query_pairs {
            ser.append_pair(k, v);
        }
        let qs = ser.finish();
        if !qs.is_empty() {
            url.push('?');
            url.push_str(&qs);
        }
    }
    url
}

/// Percent-encode a single identifier segment for the URL or the
/// X-Road-Client header. Empty input → empty output (validation
/// happens elsewhere).
fn pct(s: &str) -> String {
    utf8_percent_encode(s, &IDENTIFIER_ALLOWED).collect()
}

/// Apply the DSL's `allowed_query_params` filter to the inbound
/// query pairs.
///
/// * `None` → forward everything (spec §4.5 default).
/// * `Some(vec![])` → drop everything (paranoid opt-in).
/// * `Some(vec!["k"])` → allow-list.
fn filter_query(
    allowed: Option<&[String]>,
    pairs: Vec<(String, String)>,
) -> Vec<(String, String)> {
    match allowed {
        None => pairs,
        Some([]) => Vec::new(),
        Some(list) => {
            let set: std::collections::HashSet<&String> = list.iter().collect();
            pairs.into_iter().filter(|(k, _)| set.contains(k)).collect()
        }
    }
}

async fn read_bounded_bytes(
    mut resp: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, XtrError> {
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| XtrError::Internal(format!("reading upstream body chunk: {e}")))?
    {
        if buf.len() + chunk.len() > limit {
            return Err(XtrError::UpstreamBodyTooLarge { limit });
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

// Kept to silence unused-import lints on Duration/Identity/Policy
// when the module is used from doc examples; not called at runtime.
#[allow(dead_code)]
fn _keepalive(_: Duration, _: Option<Identity>, _: Policy) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::RestTarget;

    fn target(path: &str) -> RestTarget {
        RestTarget {
            member_class: "GOV".into(),
            member_code: "70008440".into(),
            subsystem_code: "rr".into(),
            service_code: "dde".into(),
            path: path.into(),
        }
    }

    #[test]
    fn url_construction_matches_spec_example_4_1() {
        // Spec §4.1 example:
        //   GET /r1/INSTANCE/CLASS2/MEMBER2/SUBSYSTEM2/BARSERVICE/v1/bar/zyggy?quu=1
        // The /v1/bar/zyggy tail is [path], not a version segment.
        let t = RestTarget {
            member_class: "CLASS2".into(),
            member_code: "MEMBER2".into(),
            subsystem_code: "SUBSYSTEM2".into(),
            service_code: "BARSERVICE".into(),
            path: "/v1/bar/zyggy".into(),
        };
        let url = build_url(
            "https://ss.test",
            "INSTANCE",
            &t,
            &[("quu".into(), "1".into())],
        );
        assert_eq!(
            url,
            "https://ss.test/r1/INSTANCE/CLASS2/MEMBER2/SUBSYSTEM2/BARSERVICE/v1/bar/zyggy?quu=1"
        );
    }

    #[test]
    fn url_construction_tolerates_missing_leading_slash_on_path() {
        let url = build_url("https://ss.test", "ee-test", &target("isikud"), &[]);
        assert_eq!(url, "https://ss.test/r1/ee-test/GOV/70008440/rr/dde/isikud");
    }

    #[test]
    fn url_construction_strips_trailing_slash_from_base_and_stores_it() {
        // RestLaneExecutor::new does .trim_end_matches('/') on
        // ss.url; build_url itself doesn't. This test asserts that
        // downstream callers must trim, and that a trimmed base
        // produces the correct shape.
        let url = build_url(
            "https://ss.test/".trim_end_matches('/'),
            "ee-test",
            &target("/svc"),
            &[],
        );
        assert_eq!(url, "https://ss.test/r1/ee-test/GOV/70008440/rr/dde/svc");
    }

    #[test]
    fn url_construction_percent_encodes_identifier_segments() {
        // Spec §4.2 example: service code "BAR/SERVICE" must be
        // encoded to "BAR%2FSERVICE".
        let t = RestTarget {
            member_class: "GOV".into(),
            member_code: "70008440".into(),
            subsystem_code: "rr".into(),
            service_code: "BAR/SERVICE".into(),
            path: "/foo".into(),
        };
        let url = build_url("https://ss.test", "ee-test", &t, &[]);
        assert!(
            url.contains("/BAR%2FSERVICE/"),
            "service_code must be percent-encoded: {url}"
        );
    }

    #[test]
    fn url_construction_leaves_estonian_identifiers_alone() {
        // GOV / 70008440 / rr / dde — nothing in the spec-allowed
        // set needs encoding. Regression: don't over-encode.
        let url = build_url("https://ss.test", "ee-test", &target("/isikud"), &[]);
        assert_eq!(url, "https://ss.test/r1/ee-test/GOV/70008440/rr/dde/isikud");
        // No stray percent signs.
        assert!(!url.contains('%'));
    }

    #[test]
    fn url_construction_encodes_special_chars_from_spec_allowed_set() {
        // Spec §4.8 allows '()+,-.=? in identifier values —
        // parseable, but ' ( ) + , = ? need percent-encoding in a
        // URL context per RFC 3986 sub-delim rules. The exact set
        // percent-encoded is defined by IDENTIFIER_ALLOWED.
        // Verify a stable subset: '+' becomes %2B.
        let t = RestTarget {
            member_class: "A+B".into(),
            member_code: "1".into(),
            subsystem_code: "s".into(),
            service_code: "c".into(),
            path: "".into(),
        };
        let url = build_url("https://ss.test", "ee-test", &t, &[]);
        assert!(url.contains("A%2BB"), "got: {url}");
    }

    #[test]
    fn filter_query_none_forwards_everything() {
        // Spec §4.5 default — pass through unmodified.
        let out = filter_query(None, vec![("a".into(), "1".into()), ("b".into(), "2".into())]);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn filter_query_empty_vec_drops_everything() {
        let out = filter_query(Some(&[]), vec![("k".into(), "v".into())]);
        assert!(out.is_empty());
    }

    #[test]
    fn filter_query_allow_list_keeps_only_named() {
        let allow = ["a".to_string()];
        let out = filter_query(
            Some(&allow),
            vec![("a".into(), "1".into()), ("b".into(), "2".into())],
        );
        assert_eq!(out, vec![("a".into(), "1".into())]);
    }

    #[test]
    fn client_header_shape_matches_spec_4_3() {
        // Spec §4.3 example: X-Road-Client: INSTANCE/CLASS/MEMBER/SUBSYSTEM
        use crate::config::ClientData;
        let cfg = AppConfig {
            xroad_instance: "INSTANCE".into(),
            client_data: ClientData {
                member_class: "CLASS".into(),
                member_code: "MEMBER".into(),
                subsystem_code: "SUBSYSTEM".into(),
            },
            ..Default::default()
        };
        assert_eq!(
            format_client_header(&cfg.xroad_instance, &cfg),
            "INSTANCE/CLASS/MEMBER/SUBSYSTEM"
        );
    }

    #[test]
    fn client_header_percent_encodes_segments() {
        // If subsystem code contains a `/` (spec §4.2 corner case),
        // it MUST be encoded so the wire form isn't ambiguous.
        use crate::config::ClientData;
        let cfg = AppConfig {
            xroad_instance: "ee-test".into(),
            client_data: ClientData {
                member_class: "GOV".into(),
                member_code: "70008440".into(),
                subsystem_code: "weird/sub".into(),
            },
            ..Default::default()
        };
        let h = format_client_header(&cfg.xroad_instance, &cfg);
        assert!(h.ends_with("weird%2Fsub"), "got: {h}");
        // The separators between segments stay literal.
        assert_eq!(h.matches('/').count(), 3);
    }
}
