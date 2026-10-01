//! Schema-aware SOAP lanes driven by one WSDL, in either direction.
//!
//! The legacy outbound lane (`wsdl::generator`) flattens a WSDL into
//! Handlebars DSLs: fine for flat requests, but it cannot express
//! attributes, repeated elements or `xs:choice`, and it has no client
//! certificate for non-X-Road peers. This module adds two lanes that
//! share one contract model and one XML ⇄ JSON codec:
//!
//! * **inbound** — XTR as a SOAP *provider*. A SOAP 1.1 client (X-Road
//!   Security Server, any WSDL 1.1 consumer) calls XTR;
//!   XTR decodes the request to JSON, POSTs it to a JSON backend
//!   (Ruuter, …) and encodes the answer into the WSDL output element.
//! * **outbound** — XTR as a schema-aware SOAP *client*. A caller POSTs
//!   JSON; XTR encodes it into the WSDL input element, calls the peer
//!   (optionally with its own client certificate, or via the X-Road
//!   Security Server with an X-Road header) and decodes the reply.
//!
//! Direction is chosen per WSDL by a sidecar next to it:
//!
//! ```text
//! wsdl/<group>/<name>.wsdl        # the contract
//! wsdl/<group>/<name>.meta.yaml   # optional, legacy DSL X-Road wrapping (unchanged)
//! wsdl/<group>/<name>.soap.yaml   # optional, enables `inbound:` and/or `outbound:`
//! ```
//!
//! Endpoints — inbound and outbound live under different prefixes so an
//! ingress can expose one without the other (`inbound.port` puts the
//! inbound routes on a listener of their own):
//! * `POST /soap-in/<group>/<name>`              — inbound SOAP 1.1 endpoint
//! * `GET  /soap-in/<group>/<name>?wsdl`         — the WSDL, `soap:address` rewritten
//! * `GET  /soap-in/<group>/<file>.xsd`          — local XSDs the WSDL includes/imports
//! * `POST /soap-out/<group>/<name>/<operation>` — outbound, JSON in / JSON out

pub mod codec;
pub mod contract;
pub mod dom;
pub mod handler;
pub mod outbound;

use crate::config::{AppConfig, SecurityServer};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// `<name>.soap.yaml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sidecar {
    /// Keep generating legacy Handlebars DSLs from this WSDL.
    #[serde(default = "default_true")]
    pub dsl: bool,
    #[serde(default)]
    pub inbound: Option<InboundCfg>,
    #[serde(default)]
    pub outbound: Option<OutboundCfg>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboundCfg {
    /// Base URL of the JSON backend. Operation `X` is POSTed to
    /// `<backend>/<X>` unless `operations.X.backend` overrides it.
    #[serde(default)]
    pub backend: Option<String>,
    #[serde(default)]
    pub operations: BTreeMap<String, OperationOverride>,
    /// Echo the inbound `<SOAP-ENV:Header>` children in the response.
    /// Required by the X-Road SOAP profile; harmless elsewhere.
    #[serde(default = "default_true")]
    pub echo_soap_header: bool,
    /// Shape of the JSON POSTed to the backend.
    #[serde(default)]
    pub payload: PayloadMode,
    /// Optional JSON Pointer into the backend's response selecting the
    /// output-element content (e.g. `/response` for Ruuter flows that
    /// wrap their result). A string at that location is parsed as JSON.
    #[serde(default)]
    pub response_pointer: Option<String>,
    /// Optional JSON Pointer into the decoded request element: only that
    /// part is sent to the backend (e.g. `/request` for X-Road v4 style
    /// WSDLs whose input element wraps the data in `<request>`).
    #[serde(default)]
    pub request_pointer: Option<String>,
    /// Optional: build the output element from parts instead of using
    /// the backend reply as-is. Keys are output child elements; values
    /// name the source — `request` (the request after `request_pointer`)
    /// or `backend` (the reply after `response_pointer`). X-Road v4
    /// style: `{request: request, response: backend}`.
    #[serde(default)]
    pub response_wrap: Option<BTreeMap<String, WrapSource>>,
    /// Pass the X-Road SOAP header to the backend as the X-Road REST
    /// HTTP headers (`X-Road-Client`, `X-Road-Service`, `X-Road-Id`,
    /// `X-Road-UserId`, `X-Road-Issue`) — what backends written for the
    /// REST protocol (Ruuter `.guard`s) expect. Default true.
    #[serde(default = "default_true")]
    pub forward_xroad_headers: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WrapSource {
    Request,
    Backend,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationOverride {
    pub backend: String,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PayloadMode {
    /// `{service, operation, soapAction, header, request}`.
    #[default]
    Wrapped,
    /// Only the decoded request element content.
    Request,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundCfg {
    /// Peer URL. Default: the WSDL's `soap:address`, or — when that
    /// is an X-Road `TURVASERVER` placeholder / absent — the
    /// `security_server` from `xtr.yaml` (URL + its mTLS identity).
    #[serde(default)]
    pub url: Option<String>,
    /// Own client certificate for non-X-Road peers that require
    /// 2-way TLS. PKCS#12, password from env var.
    #[serde(default)]
    pub keystore_path: Option<PathBuf>,
    #[serde(default = "default_keystore_env")]
    pub keystore_password_env: String,
    /// PEM/DER CA bundle to trust for the peer's server certificate.
    #[serde(default)]
    pub trust_ca_path: Option<PathBuf>,
    /// Add the X-Road SOAP header (client from `client_data`, this
    /// service identity, serviceCode = operation name, serviceVersion
    /// from the WSDL's `<xrd:version>` when present).
    #[serde(default)]
    pub xroad_service: Option<XroadService>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XroadService {
    pub member_class: String,
    pub member_code: String,
    pub subsystem_code: String,
}

fn default_true() -> bool {
    true
}

fn default_keystore_env() -> String {
    "XTR_OUTBOUND_KEYSTORE_PASSWORD".into()
}

impl InboundCfg {
    pub fn backend_for(&self, op: &str) -> Option<String> {
        if let Some(o) = self.operations.get(op) {
            return Some(o.backend.clone());
        }
        self.backend
            .as_deref()
            .map(|b| format!("{}/{}", b.trim_end_matches('/'), op))
    }
}

#[derive(Debug)]
pub struct Service {
    pub group: String,
    pub name: String,
    pub wsdl_path: PathBuf,
    pub wsdl_text: String,
    pub contract: contract::Contract,
    pub sidecar: Sidecar,
    /// Built at boot when `outbound:` is configured and valid.
    pub outbound: Option<outbound::Client>,
}

#[derive(Debug, Default)]
pub struct Registry {
    services: BTreeMap<(String, String), Arc<Service>>,
}

impl Registry {
    pub fn get(&self, group: &str, name: &str) -> Option<&Arc<Service>> {
        self.services.get(&(group.to_string(), name.to_string()))
    }

    pub fn is_empty(&self) -> bool {
        self.services.is_empty()
    }

    pub fn len(&self) -> usize {
        self.services.len()
    }

    /// A local XSD that some inbound WSDL of `group` includes.
    pub fn schema_file(&self, group: &str, file: &str) -> Option<&Path> {
        self.services
            .values()
            .filter(|s| s.group == group && s.sidecar.inbound.is_some())
            .find_map(|s| s.contract.schema_files.get(file).map(|p| p.as_path()))
    }
}

pub fn sidecar_path(wsdl: &Path) -> Option<PathBuf> {
    ["soap.yaml", "soap.yml"]
        .iter()
        .map(|ext| wsdl.with_extension(ext))
        .find(|p| p.is_file())
}

/// Used by the legacy DSL pipeline: `dsl: false` in the sidecar
/// suppresses DSL generation for this WSDL. Unreadable sidecars
/// don't suppress anything (the loader reports them).
pub fn dsl_disabled(wsdl: &Path) -> bool {
    sidecar_path(wsdl)
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_yaml_ng::from_str::<Sidecar>(&s).ok())
        .map(|s| !s.dsl)
        .unwrap_or(false)
}

/// How many sidecars ask for each lane.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LaneSummary {
    pub inbound: usize,
    pub outbound: usize,
}

impl Registry {
    pub fn summary(&self) -> LaneSummary {
        LaneSummary {
            inbound: self
                .services
                .values()
                .filter(|s| s.sidecar.inbound.is_some())
                .count(),
            outbound: self
                .services
                .values()
                .filter(|s| s.outbound.is_some())
                .count(),
        }
    }
}

/// An inbound endpoint must be reachable by the SOAP peer network; every
/// endpoint that makes XTR call out with its own identity must not —
/// the `/soap-out/…` lanes AND the DSL endpoints `/:group/:service`
/// (hand-written or generated from WSDLs). Sharing one listener is only
/// safe when those are token-gated.
pub fn exposure_error(
    sum: LaneSummary,
    dsl_endpoints: usize,
    separate_port: bool,
    token_set: bool,
) -> Option<String> {
    let outbound_like = sum.outbound + dsl_endpoints;
    (sum.inbound > 0 && outbound_like > 0 && !separate_port && !token_set).then(|| {
        format!(
            "the inbound SOAP lane (/soap-in/) would share one listener with {} \
             outbound endpoint(s) ({} /soap-out/ lane(s), {} DSL endpoint(s) on \
             /:group/:service) while XTR_INTER_SERVICE_TOKEN is unset: whoever can \
             reach /soap-in/ (the SOAP peer network) could make XTR call out with its \
             own identity. Set `inbound.port` (separate listener for /soap-in/) or \
             XTR_INTER_SERVICE_TOKEN.",
            outbound_like, sum.outbound, dsl_endpoints
        )
    })
}

/// A configuration problem found without building any client or
/// reading any keystore — shared by boot (`load_all`) and the doctor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub kind: ProblemKind,
    /// The `.soap.yaml` the problem belongs to.
    pub file: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemKind {
    /// Sidecar unreadable/invalid, or its WSDL unusable → boot refused.
    SidecarInvalid,
    /// `outbound:` cannot work (target, keystore, password) → boot refused.
    OutboundInvalid,
    /// An inbound operation has no backend → calls get a SOAP fault.
    InboundOpWithoutBackend,
}

impl ProblemKind {
    pub fn is_fatal(self) -> bool {
        !matches!(self, Self::InboundOpWithoutBackend)
    }
}

#[derive(Debug, Default)]
pub struct Check {
    pub summary: LaneSummary,
    pub problems: Vec<Problem>,
}

fn sidecar_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for p in rd.flatten().map(|e| e.path()) {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if p.is_dir() {
                stack.push(p);
            } else if name.ends_with(".soap.yaml") || name.ends_with(".soap.yml") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// `wsdl/g/x.soap.yaml` → `wsdl/g/x.wsdl` (or `.WSDL`).
fn wsdl_for_sidecar(sidecar: &Path) -> Option<PathBuf> {
    let name = sidecar.file_name()?.to_str()?;
    let stem = name
        .strip_suffix(".soap.yaml")
        .or_else(|| name.strip_suffix(".soap.yml"))?;
    ["wsdl", "WSDL"]
        .iter()
        .map(|ext| sidecar.with_file_name(format!("{stem}.{ext}")))
        .find(|p| p.is_file())
}

/// Validate every `.soap.yaml` under `dir` statically.
pub fn check_all(dir: Option<&Path>, cfg: &AppConfig) -> Check {
    let mut check = Check::default();
    let Some(root) = dir.filter(|d| d.is_dir()) else {
        return check;
    };
    // (group, name) → first sidecar claiming it: `g/a/x.wsdl` and
    // `g/a-x.wsdl` both map to `/soap-in/g/a-x`.
    let mut endpoints: BTreeMap<(String, String), PathBuf> = BTreeMap::new();
    // (group, xsd file name) → canonical path: `/soap-in/<group>/<file>.xsd`
    // is one URL per group, so two different files must not share it.
    let mut xsds: BTreeMap<(String, String), PathBuf> = BTreeMap::new();
    for sc in sidecar_files(root) {
        let problem = |kind, message: String| Problem {
            kind,
            file: sc.clone(),
            message,
        };
        let Some(wsdl) = wsdl_for_sidecar(&sc) else {
            check.problems.push(problem(
                ProblemKind::SidecarInvalid,
                "no WSDL with the same name next to this sidecar".into(),
            ));
            continue;
        };
        let prepared = match prepare(root, &wsdl, &sc) {
            Ok(p) => p,
            Err(e) => {
                check.problems.push(problem(ProblemKind::SidecarInvalid, e));
                continue;
            }
        };
        let key = (prepared.group.clone(), prepared.name.clone());
        if let Some(first) = endpoints.get(&key) {
            check.problems.push(problem(
                ProblemKind::SidecarInvalid,
                format!(
                    "endpoint name /soap-in/{}/{} is already taken by {} — rename one WSDL",
                    key.0,
                    key.1,
                    first.display()
                ),
            ));
            continue;
        }
        endpoints.insert(key, sc.clone());
        if prepared.sidecar.inbound.is_some() {
            for (file, path) in &prepared.contract.schema_files {
                let canon = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
                let k = (prepared.group.clone(), file.clone());
                match xsds.get(&k) {
                    Some(prev) if prev != &canon => check.problems.push(problem(
                        ProblemKind::SidecarInvalid,
                        format!(
                            "schema {file} would be published as /soap-in/{}/{file} but another \
                             WSDL of this group publishes a different {file} ({}) — rename \
                             one of them or move the WSDL to another group",
                            prepared.group,
                            prev.display()
                        ),
                    )),
                    Some(_) => {}
                    None => {
                        xsds.insert(k, canon);
                    }
                }
            }
        }
        if let Some(ib) = &prepared.sidecar.inbound {
            check.summary.inbound += 1;
            for op in &prepared.contract.operations {
                if ib.backend_for(&op.name).is_none() {
                    check.problems.push(problem(
                        ProblemKind::InboundOpWithoutBackend,
                        format!(
                            "operation {} has no backend — calls get a SOAP fault",
                            op.name
                        ),
                    ));
                }
            }
        }
        if let Some(ob) = &prepared.sidecar.outbound {
            check.summary.outbound += 1;
            if let Err(e) = outbound_target(ob, &prepared.contract, cfg) {
                check
                    .problems
                    .push(problem(ProblemKind::OutboundInvalid, e));
            }
        }
    }
    check
}

/// Load every WSDL with a `.soap.yaml` sidecar. Any fatal problem
/// (invalid sidecar or WSDL, unusable outbound configuration) refuses
/// boot with the full list — a sidecar is explicit opt-in config, so a
/// typo must not silently drop a published SOAP service.
pub fn load_all(dir: Option<&Path>, cfg: &AppConfig) -> Result<Registry, String> {
    let mut reg = Registry::default();
    let check = check_all(dir, cfg);
    let fatal: Vec<String> = check
        .problems
        .iter()
        .filter(|p| p.kind.is_fatal())
        .map(|p| format!("{}: {}", p.file.display(), p.message))
        .collect();
    if !fatal.is_empty() {
        return Err(format!(
            "invalid SOAP lane configuration (run `xtr-on-rust doctor` for details):\n  {}",
            fatal.join("\n  ")
        ));
    }
    for p in check.problems.iter().filter(|p| !p.kind.is_fatal()) {
        tracing::warn!(sidecar = %p.file.display(), "{}", p.message);
    }
    let Some(root) = dir.filter(|d| d.is_dir()) else {
        return Ok(reg);
    };
    for sc in sidecar_files(root) {
        let wsdl = wsdl_for_sidecar(&sc).ok_or("sidecar without WSDL")?;
        let prepared = prepare(root, &wsdl, &sc)?;
        let outbound = match &prepared.sidecar.outbound {
            None => None,
            Some(ob) => Some(
                build_outbound(ob, &prepared.contract, cfg)
                    .map_err(|e| format!("{}: outbound: {e}", sc.display()))?,
            ),
        };
        let svc = Service {
            group: prepared.group,
            name: prepared.name,
            wsdl_path: wsdl,
            wsdl_text: prepared.wsdl_text,
            contract: prepared.contract,
            sidecar: prepared.sidecar,
            outbound,
        };
        let ops: Vec<&str> = svc
            .contract
            .operations
            .iter()
            .map(|o| o.name.as_str())
            .collect();
        if svc.sidecar.inbound.is_some() {
            tracing::info!(endpoint = %format!("/soap-in/{}/{}", svc.group, svc.name), operations = ?ops,
                "inbound SOAP endpoint registered");
        }
        if let Some(ob) = &svc.outbound {
            tracing::info!(endpoint = %format!("/soap-out/{}/{}/<operation>", svc.group, svc.name), operations = ?ops,
                target = %ob.url, client_cert = ob.client_cert, "schema-aware outbound registered");
        }
        reg.services
            .insert((svc.group.clone(), svc.name.clone()), Arc::new(svc));
    }
    Ok(reg)
}

struct Prepared {
    group: String,
    name: String,
    wsdl_text: String,
    contract: contract::Contract,
    sidecar: Sidecar,
}

fn prepare(root: &Path, wsdl: &Path, sidecar: &Path) -> Result<Prepared, String> {
    let sc_text = fs::read_to_string(sidecar).map_err(|e| format!("reading sidecar: {e}"))?;
    let sidecar_cfg: Sidecar =
        serde_yaml_ng::from_str(&sc_text).map_err(|e| format!("invalid sidecar: {e}"))?;
    if let Some(ib) = &sidecar_cfg.inbound {
        for url in ib
            .backend
            .iter()
            .chain(ib.operations.values().map(|o| &o.backend))
        {
            let u = url::Url::parse(url).map_err(|e| format!("backend URL {url:?}: {e}"))?;
            if !matches!(u.scheme(), "http" | "https") {
                return Err(format!("backend URL {url:?}: only http/https allowed"));
            }
        }
        for (field, p) in [
            ("response_pointer", &ib.response_pointer),
            ("request_pointer", &ib.request_pointer),
        ] {
            if let Some(p) = p {
                if !p.is_empty() && !p.starts_with('/') {
                    return Err(format!(
                        "{field} {p:?} must be a JSON Pointer starting with '/'"
                    ));
                }
            }
        }
    }
    let rel: Vec<String> = wsdl
        .parent()
        .and_then(|p| p.strip_prefix(root).ok())
        .map(|p| {
            p.components()
                .filter_map(|c| c.as_os_str().to_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let stem = wsdl
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("WSDL file name is not UTF-8")?;
    let (group, name) = match rel.as_slice() {
        [] => return Err("WSDLs with a .soap.yaml must live in a <group>/ subdirectory".into()),
        [g] => (g.clone(), stem.to_string()),
        [g, rest @ ..] => (g.clone(), format!("{}-{stem}", rest.join("-"))),
    };
    let wsdl_text = fs::read_to_string(wsdl).map_err(|e| format!("reading WSDL: {e}"))?;
    let dir = wsdl.parent().unwrap_or(root);
    let loader = contract::fs_loader(dir);
    let contract = contract::load(&wsdl_text, &loader).map_err(|e| format!("WSDL: {e}"))?;
    if !contract.soap11 {
        return Err("WSDL has no SOAP 1.1 binding (these lanes speak SOAP 1.1 only)".into());
    }
    if contract.operations.is_empty() {
        return Err("WSDL has no operations with a resolvable input element".into());
    }
    Ok(Prepared {
        group,
        name,
        wsdl_text,
        contract,
        sidecar: sidecar_cfg,
    })
}

fn is_placeholder(url: &str) -> bool {
    let l = url.to_ascii_lowercase();
    l.contains("turvaserver") || l.contains("security-server") || l.contains("your-security-server")
}

/// Where an outbound lane sends and with which identity — resolved
/// statically (file existence and env presence only; the keystore is
/// not opened here).
struct Target {
    url: String,
    /// `(identity config, password env var)` when a client cert is used.
    identity: Option<(SecurityServer, String)>,
}

fn outbound_target(
    ob: &OutboundCfg,
    contract: &contract::Contract,
    cfg: &AppConfig,
) -> Result<Target, String> {
    let direct = ob
        .url
        .clone()
        .or_else(|| contract.address.clone().filter(|a| !is_placeholder(a)));
    match direct {
        Some(url) => {
            // Same SSRF guard as WSDL-declared URLs on the DSL lane.
            crate::wsdl::url_guard::validate_upstream_url(&url, &cfg.wsdl)
                .map_err(|e| e.to_string())?;
            let identity = match &ob.keystore_path {
                Some(ks) => {
                    if !ks.is_file() {
                        return Err(format!("keystore_path {} does not exist", ks.display()));
                    }
                    if std::env::var(&ob.keystore_password_env).is_err() {
                        return Err(format!(
                            "env var {} (keystore password) is unset",
                            ob.keystore_password_env
                        ));
                    }
                    Some((
                        SecurityServer {
                            url: url.clone(),
                            keystore_path: ks.clone(),
                            keystore_password_env: ob.keystore_password_env.clone(),
                            trust_ca_path: ob.trust_ca_path.clone(),
                        },
                        ob.keystore_password_env.clone(),
                    ))
                }
                None => None,
            };
            if let Some(ca) = &ob.trust_ca_path {
                if !ca.is_file() {
                    return Err(format!("trust_ca_path {} does not exist", ca.display()));
                }
            }
            Ok(Target { url, identity })
        }
        None => {
            let ss = cfg.security_server.as_ref().ok_or(
                "WSDL address is an X-Road placeholder (or absent) and no security_server is configured",
            )?;
            if ob.keystore_path.is_some() || ob.trust_ca_path.is_some() {
                // The Security Server route always uses the identity and CA
                // from `security_server` in xtr.yaml — say so instead of
                // silently ignoring the sidecar's values.
                return Err(
                    "`outbound.keystore_path` / `outbound.trust_ca_path` are not used \
                            when the target is the Security Server (WSDL address is a \
                            placeholder) — set `security_server.keystore_path` / \
                            `security_server.trust_ca_path` in xtr.yaml instead"
                        .into(),
                );
            }
            if ob.xroad_service.is_none() {
                // A Security Server rejects SOAP without the X-Road header.
                return Err(
                    "target is the X-Road Security Server (WSDL address is a placeholder) \
                     but `outbound.xroad_service` is not set — the X-Road header \
                     (client/service/id/protocolVersion) would be missing"
                        .into(),
                );
            }
            Ok(Target {
                url: ss.url.clone(),
                identity: Some((ss.clone(), ss.keystore_password_env.clone())),
            })
        }
    }
}

fn build_outbound(
    ob: &OutboundCfg,
    contract: &contract::Contract,
    cfg: &AppConfig,
) -> Result<outbound::Client, String> {
    let target = outbound_target(ob, contract, cfg)?;
    let identity = match target.identity {
        Some((ss, env)) => {
            let pw = std::env::var(&env)
                .map_err(|_| format!("env var {env} (keystore password) is unset"))?;
            Some((ss, pw))
        }
        None => None,
    };
    outbound::Client::new(
        target.url,
        identity,
        ob.trust_ca_path.as_deref(),
        ob.xroad_service.clone(),
        cfg,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposure_error_only_for_shared_listener_without_token() {
        let both = LaneSummary {
            inbound: 1,
            outbound: 1,
        };
        assert!(exposure_error(both, 0, false, false).is_some());
        assert!(
            exposure_error(both, 0, true, false).is_none(),
            "separate inbound port"
        );
        assert!(
            exposure_error(both, 0, false, true).is_none(),
            "token-gated"
        );
        let only_in = LaneSummary {
            inbound: 1,
            outbound: 0,
        };
        assert!(exposure_error(only_in, 0, false, false).is_none());
        // Legacy DSL endpoints call out with XTR's identity too.
        let e = exposure_error(only_in, 194, false, false).expect("DSL endpoints count");
        assert!(e.contains("194 DSL endpoint(s)"), "{e}");
        assert!(exposure_error(only_in, 194, true, false).is_none());
        let only_out = LaneSummary {
            inbound: 0,
            outbound: 1,
        };
        assert!(exposure_error(only_out, 194, false, false).is_none());
    }

    #[test]
    fn sidecar_rejects_unknown_fields_and_parses_new_options() {
        let sc: Sidecar = serde_yaml_ng::from_str(
            "inbound:\n  backend: http://b\n  request_pointer: /request\n  response_wrap: {request: request, response: backend}\n",
        )
        .unwrap();
        let ib = sc.inbound.unwrap();
        assert!(ib.forward_xroad_headers);
        assert_eq!(ib.response_wrap.unwrap()["response"], WrapSource::Backend);
        assert!(serde_yaml_ng::from_str::<Sidecar>("inbound:\n  backend: x\n  typo: 1\n").is_err());
    }
}
