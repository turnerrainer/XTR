//! Audit-v1 C1 — trust-boundary guard on upstream URLs discovered
//! in operator-supplied WSDLs and metadata sidecars.
//!
//! Threat model: an attacker who can drop a file into the
//! `wsdl_watch_dir` mount could otherwise embed
//! `<soap:address location="http://169.254.169.254/…"/>` in a WSDL;
//! the DSL generator would blindly write that as the request-time
//! upstream, and the first REST call would exfiltrate cloud-provider
//! metadata. Guard rejects that shape at *ingest* time so a
//! malicious WSDL never lands in the DSL corpus.
//!
//! Two layers of defense:
//!   1. Literal-IP guard (this module) — rejects private / loopback
//!      / link-local / CGNAT / ULA ranges when the URL host is a
//!      literal IP. Runs at boot; deterministic.
//!   2. Optional operator allowlist — if `upstream_host_allowlist`
//!      is non-empty, the host must be on the list. Belt-and-braces
//!      for high-trust deployments (typically a small set of
//!      well-known producers: ariregxmlv6.rik.ee etc).
//!
//! Hostname → IP resolution at fire time is *not* done here: doing
//! it at boot would require DNS at startup and a stale cache would
//! give false confidence. Operators wanting rebinding-safe deploys
//! should pin the allowlist or run in a network policy that blocks
//! egress to the metadata endpoints.

use crate::config::WsdlIngest;
use crate::error::XtrError;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use url::{Host, Url};

/// Validate an upstream URL string discovered in a WSDL
/// `<soap:address location=…/>` or a metadata sidecar
/// `service_url:` override. Returns the parsed URL on success;
/// on failure the error carries the URL text (safe to log — this
/// data came from operator-controlled files).
pub fn validate_upstream_url(raw: &str, cfg: &WsdlIngest) -> Result<Url, XtrError> {
    let parsed = Url::parse(raw).map_err(|e| {
        XtrError::Internal(format!("upstream URL '{raw}' failed to parse: {e}"))
    })?;

    // Scheme guard — reject anything that isn't http(s), and http
    // only when the operator explicitly opts in. `Url::scheme()`
    // returns the lowercased scheme, so `HTTP://...` also lands
    // here as `"http"`.
    match parsed.scheme() {
        "https" => {}
        "http" if cfg.allow_http_upstream => {}
        "http" => {
            return Err(XtrError::Internal(format!(
                "upstream URL '{raw}' uses http; set wsdl.allow_http_upstream=true to permit"
            )));
        }
        other => {
            return Err(XtrError::Internal(format!(
                "upstream URL '{raw}' uses unsupported scheme '{other}'"
            )));
        }
    }

    // `Url::host()` returns the parsed host as `Ipv4/Ipv6/Domain`.
    // This is the typed form — `host_str()` wraps IPv6 in brackets
    // which then fails `IpAddr::parse` and the range check silently
    // no-ops. Use the enum to avoid that footgun. Userinfo is
    // stripped either way — `http://LEGIT@169.254.169.254/`
    // exposes the real host here, not the userinfo half.
    let host = parsed.host().ok_or_else(|| {
        XtrError::Internal(format!("upstream URL '{raw}' has no host"))
    })?;

    // Literal IP → check ranges. Domains pass through range checks
    // (DNS deferred by design — see module docs).
    let host_ip: Option<IpAddr> = match &host {
        Host::Ipv4(v4) => Some(IpAddr::V4(*v4)),
        Host::Ipv6(v6) => Some(normalise_v4_mapped(IpAddr::V6(*v6))),
        Host::Domain(_) => None,
    };
    if let Some(ip) = host_ip {
        if is_private_or_local(ip) {
            return Err(XtrError::Internal(format!(
                "upstream URL '{raw}' targets blocked IP range ({ip})"
            )));
        }
    }

    // Optional operator allowlist. Compared against the URL host
    // as literal text (case-insensitive per DNS conventions).
    if !cfg.upstream_host_allowlist.is_empty() {
        let host_str = match &host {
            Host::Domain(d) => d.to_string(),
            Host::Ipv4(v4) => v4.to_string(),
            Host::Ipv6(v6) => v6.to_string(),
        };
        let host_lc = host_str.to_ascii_lowercase();
        let allowed = cfg
            .upstream_host_allowlist
            .iter()
            .any(|h| h.to_ascii_lowercase() == host_lc);
        if !allowed {
            return Err(XtrError::Internal(format!(
                "upstream URL '{raw}' host '{host_str}' not in upstream_host_allowlist"
            )));
        }
    }

    Ok(parsed)
}

/// IPv6 addresses of the form `::ffff:a.b.c.d` represent an IPv4
/// address in an IPv6 wrapper. Unwrap so range checks see the real
/// IPv4 value; otherwise `[::ffff:169.254.169.254]` would sneak
/// past a naive check that only examines the v6 range table.
fn normalise_v4_mapped(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        v4 => v4,
    }
}

/// Port of the range table Ruuter uses for the same purpose
/// (see `Ruuter-on-Rust/src/http_client/mod.rs`). Kept in sync so
/// the two projects behave identically at the SSRF boundary.
fn is_private_or_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_v4(v4),
        IpAddr::V6(v6) => is_blocked_v6(v6),
    }
}

fn is_blocked_v4(v4: Ipv4Addr) -> bool {
    if v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_documentation()
        || v4.is_unspecified()
        || v4.is_multicast()
    {
        return true;
    }
    let [a, b, _, _] = v4.octets();
    // CGNAT: 100.64.0.0/10 (RFC 6598)
    if a == 100 && (64..=127).contains(&b) {
        return true;
    }
    // Reserved / benchmarking: 198.18.0.0/15 (RFC 2544)
    if a == 198 && (b == 18 || b == 19) {
        return true;
    }
    // Reserved for future use: 240.0.0.0/4 (excluding 255.255.255.255
    // already caught above).
    if a >= 240 {
        return true;
    }
    false
}

fn is_blocked_v6(v6: Ipv6Addr) -> bool {
    if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
        return true;
    }
    let segments = v6.segments();
    // Link-local: fe80::/10
    if (segments[0] & 0xffc0) == 0xfe80 {
        return true;
    }
    // Unique-local: fc00::/7
    if (segments[0] & 0xfe00) == 0xfc00 {
        return true;
    }
    // Site-local (deprecated but still worth blocking): fec0::/10
    if (segments[0] & 0xffc0) == 0xfec0 {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strict() -> WsdlIngest {
        WsdlIngest::default()
    }

    fn allow_http() -> WsdlIngest {
        WsdlIngest {
            allow_http_upstream: true,
            upstream_host_allowlist: vec![],
        }
    }

    #[test]
    fn accepts_legit_public_https_url() {
        validate_upstream_url("https://ariregxmlv6.rik.ee/", &strict()).unwrap();
    }

    #[test]
    fn accepts_uppercased_scheme_via_url_normalisation() {
        // url::Url normalises scheme to lowercase, so "HTTPS://..."
        // still lands as scheme="https" — no case-based bypass.
        validate_upstream_url("HTTPS://ariregxmlv6.rik.ee/", &strict()).unwrap();
    }

    #[test]
    fn rejects_aws_metadata_ip() {
        let err = validate_upstream_url("http://169.254.169.254/latest/", &allow_http())
            .unwrap_err();
        assert!(
            matches!(&err, XtrError::Internal(m) if m.contains("blocked IP range")),
            "expected blocked-range error, got {err:?}"
        );
    }

    #[test]
    fn rejects_gcp_metadata_ip() {
        // 169.254.169.254 is the shared AWS/GCP metadata IP.
        let err =
            validate_upstream_url("http://169.254.169.254/computeMetadata/v1/", &allow_http())
                .unwrap_err();
        assert!(matches!(err, XtrError::Internal(_)));
    }

    #[test]
    fn rejects_loopback_ipv4() {
        let err =
            validate_upstream_url("http://127.0.0.1:8080/", &allow_http()).unwrap_err();
        assert!(matches!(err, XtrError::Internal(_)));
    }

    #[test]
    fn rejects_private_10() {
        let err = validate_upstream_url("http://10.0.0.5/", &allow_http()).unwrap_err();
        assert!(matches!(err, XtrError::Internal(_)));
    }

    #[test]
    fn rejects_private_192_168() {
        let err = validate_upstream_url("http://192.168.1.1/", &allow_http()).unwrap_err();
        assert!(matches!(err, XtrError::Internal(_)));
    }

    #[test]
    fn rejects_cgnat_100_64() {
        let err =
            validate_upstream_url("http://100.64.0.1/", &allow_http()).unwrap_err();
        assert!(matches!(err, XtrError::Internal(_)));
    }

    #[test]
    fn rejects_ipv6_loopback() {
        let err = validate_upstream_url("http://[::1]/", &allow_http()).unwrap_err();
        assert!(matches!(err, XtrError::Internal(_)));
    }

    #[test]
    fn rejects_ipv6_link_local() {
        let err = validate_upstream_url("http://[fe80::1]/", &allow_http()).unwrap_err();
        assert!(matches!(err, XtrError::Internal(_)));
    }

    #[test]
    fn rejects_ipv6_unique_local() {
        let err = validate_upstream_url("http://[fc00::1]/", &allow_http()).unwrap_err();
        assert!(matches!(err, XtrError::Internal(_)));
    }

    #[test]
    fn rejects_v4_mapped_metadata_bypass() {
        // ::ffff:169.254.169.254 unwraps to the AWS metadata IP.
        // A naive v6-only check would miss this; the guard
        // unwraps mapped addresses first.
        let err =
            validate_upstream_url("http://[::ffff:169.254.169.254]/", &allow_http())
                .unwrap_err();
        assert!(matches!(err, XtrError::Internal(_)));
    }

    #[test]
    fn rejects_userinfo_smuggling() {
        // Naive parsers might treat "LEGIT.example.com" as the
        // host; url::Url correctly places it as userinfo and
        // exposes 169.254.169.254 as host_str().
        let err = validate_upstream_url(
            "http://LEGIT.example.com@169.254.169.254/",
            &allow_http(),
        )
        .unwrap_err();
        assert!(matches!(err, XtrError::Internal(_)));
    }

    #[test]
    fn rejects_http_by_default() {
        let err = validate_upstream_url("http://example.com/", &strict()).unwrap_err();
        assert!(
            matches!(&err, XtrError::Internal(m) if m.contains("http")),
            "expected http-not-allowed error, got {err:?}"
        );
    }

    #[test]
    fn allows_http_when_opted_in() {
        validate_upstream_url("http://example.com/", &allow_http()).unwrap();
    }

    #[test]
    fn rejects_file_scheme() {
        // file:// would let an attacker point at /etc/passwd via
        // an executor that supports it. reqwest doesn't, but the
        // reject-at-the-boundary policy stops it before the
        // question comes up.
        let err = validate_upstream_url("file:///etc/passwd", &allow_http()).unwrap_err();
        assert!(matches!(&err, XtrError::Internal(m) if m.contains("unsupported scheme")));
    }

    #[test]
    fn rejects_gopher_scheme() {
        let err =
            validate_upstream_url("gopher://example.com:1234/", &allow_http()).unwrap_err();
        assert!(matches!(err, XtrError::Internal(_)));
    }

    #[test]
    fn allowlist_accepts_listed_host() {
        let cfg = WsdlIngest {
            allow_http_upstream: false,
            upstream_host_allowlist: vec!["ariregxmlv6.rik.ee".into()],
        };
        validate_upstream_url("https://ariregxmlv6.rik.ee/service", &cfg).unwrap();
    }

    #[test]
    fn allowlist_rejects_unlisted_host_even_when_public() {
        let cfg = WsdlIngest {
            allow_http_upstream: false,
            upstream_host_allowlist: vec!["ariregxmlv6.rik.ee".into()],
        };
        let err =
            validate_upstream_url("https://evil.example.com/", &cfg).unwrap_err();
        assert!(
            matches!(&err, XtrError::Internal(m) if m.contains("upstream_host_allowlist")),
            "expected allowlist error, got {err:?}"
        );
    }

    #[test]
    fn allowlist_is_case_insensitive() {
        let cfg = WsdlIngest {
            allow_http_upstream: false,
            upstream_host_allowlist: vec!["ARIREGXMLV6.rik.ee".into()],
        };
        validate_upstream_url("https://ariregxmlv6.RIK.EE/", &cfg).unwrap();
    }

    #[test]
    fn rejects_malformed_url() {
        let err = validate_upstream_url("not a url", &allow_http()).unwrap_err();
        assert!(matches!(&err, XtrError::Internal(m) if m.contains("failed to parse")));
    }

    #[test]
    fn rejects_multicast_ipv4() {
        let err = validate_upstream_url("http://224.0.0.1/", &allow_http()).unwrap_err();
        assert!(matches!(err, XtrError::Internal(_)));
    }

    #[test]
    fn rejects_unspecified_ipv4() {
        let err = validate_upstream_url("http://0.0.0.0/", &allow_http()).unwrap_err();
        assert!(matches!(err, XtrError::Internal(_)));
    }
}
