//! Raw-XML lanes: XTR in front of a peer that exchanges bare XML
//! documents over plain HTTP(S), no SOAP envelope (e.g. the EU ERRU
//! MOVEHUB `BTSHTTPReceive` endpoints).
//!
//! The SOAP lanes (`crate::inbound`) need a WSDL and always wrap the
//! payload in a SOAP 1.1 envelope. This module reuses their XSD-guided
//! XML ⇄ JSON codec but speaks raw XML on the wire. It stays stateless:
//! no queue, no retry, no outbox — the caller owns those.
//!
//! One sidecar per contract, next to the XSD files:
//!
//! ```text
//! wsdl/<group>/<name>.xml.yaml    # enables `inbound:` and/or `outbound:`
//! wsdl/<group>/*.xsd              # the contract (entries listed in `schemas:`)
//! ```
//!
//! Endpoints (inbound and outbound live under different prefixes so an
//! ingress can expose one without the other):
//! * `POST /xml-in/<group>/<name>`                — raw XML in → JSON to a backend
//! * `POST <inbound.path>`                        — optional alias of the above
//! * `POST /xml-out/<group>/<name>/<message>`     — JSON in → raw XML POST to the peer
//!
//! The XSD is used as a *hint* for the codec (element order, repetition,
//! namespaces); it is not a validating schema model.

pub mod handler;

use crate::config::{AppConfig, SecurityServer};
use crate::inbound::contract::{self, QName, Schema};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// `<name>.xml.yaml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XmlSidecar {
    /// Entry XSD files, relative to the sidecar's directory. Local
    /// `xs:include` / `xs:import` targets are resolved from the same
    /// directory; remote `schemaLocation`s are never fetched.
    pub schemas: Vec<String>,
    /// Message types this lane carries: global element names in the XSD.
    /// Anything else is refused. `Name: {}` is enough; a value may
    /// override the backend / peer URL / timeout for that message.
    pub messages: BTreeMap<String, Option<MessageCfg>>,
    #[serde(default)]
    pub inbound: Option<XmlInbound>,
    #[serde(default)]
    pub outbound: Option<XmlOutbound>,
}

/// Which way a message type travels on this lane.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// Peer → XTR only (`inbound:`).
    In,
    /// XTR → peer only (`outbound:`).
    Out,
    #[default]
    Both,
}

impl Direction {
    pub fn inbound(self) -> bool {
        matches!(self, Self::In | Self::Both)
    }
    pub fn outbound(self) -> bool {
        matches!(self, Self::Out | Self::Both)
    }
}

impl XmlSidecar {
    /// Backend URL of an inbound message: the message override, else
    /// `<inbound.backend>/<Message>`.
    pub fn backend_for(&self, message: &str) -> Option<String> {
        let own = self
            .messages
            .get(message)
            .and_then(Option::as_ref)
            .and_then(|c| c.backend.clone());
        own.or_else(|| {
            let base = self.inbound.as_ref()?.backend.as_deref()?;
            Some(format!("{}/{}", base.trim_end_matches('/'), message))
        })
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageCfg {
    /// `in`, `out` or `both` (default). A message that is only sent
    /// (or only received) should say so: the other direction then
    /// refuses it, and settings that cannot apply are rejected at boot.
    #[serde(default)]
    pub direction: Direction,
    /// Inbound: backend URL for this message (default `<inbound.backend>/<Message>`).
    #[serde(default)]
    pub backend: Option<String>,
    /// Inbound: instead of decoding to JSON, POST the received bytes
    /// unchanged to this URL and relay its status to the peer. For
    /// staged migrations: some message types are handled here, the
    /// rest keep going to the system being replaced.
    #[serde(default)]
    pub forward: Option<String>,
    /// Outbound: peer URL for this message (default `outbound.url`).
    #[serde(default)]
    pub url: Option<String>,
    /// Outbound: timeout in seconds for this message (default `outbound.timeout_secs`).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XmlInbound {
    /// Base URL of the JSON backend. Message `X` is POSTed to
    /// `<backend>/<X>` unless `messages.X.backend` overrides it.
    #[serde(default)]
    pub backend: Option<String>,
    /// HTTP status returned to the peer when the backend answers 2xx.
    #[serde(default = "default_accept_status")]
    pub success_status: u16,
    /// Also send the received bytes (`rawXml`) and their SHA-256 hex
    /// digest (`sha256`) to the backend. The digest is always sent.
    #[serde(default = "default_true")]
    pub include_raw: bool,
    /// Backend 4xx codes handed to the peer as they are (the backend
    /// decides: 400 malformed, 409 duplicate, 422 rejected …). Any
    /// other 4xx (a wrong backend path → 404, a gateway guard → 401/403)
    /// is an infrastructure problem, not a verdict on the message: the
    /// peer gets 502 so it retries instead of dropping the message.
    #[serde(default = "default_relay_statuses")]
    pub relay_statuses: Vec<u16>,
    /// Extra route serving this lane, e.g. `/erru/xml`, so an existing
    /// ingress path keeps working unchanged. Must not collide with the
    /// reserved routes.
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XmlOutbound {
    /// Peer URL (per message overridable).
    #[serde(default)]
    pub url: Option<String>,
    /// Seconds. Must not exceed `limits.request_timeout_secs`.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Own client certificate for peers that require 2-way TLS (PKCS#12).
    #[serde(default)]
    pub keystore_path: Option<PathBuf>,
    #[serde(default = "default_keystore_env")]
    pub keystore_password_env: String,
    /// PEM/DER CA to trust for the peer's server certificate.
    #[serde(default)]
    pub trust_ca_path: Option<PathBuf>,
    #[serde(default = "default_content_type")]
    pub content_type: String,
}

fn default_true() -> bool {
    true
}

fn default_relay_statuses() -> Vec<u16> {
    vec![400, 409, 422]
}

fn default_accept_status() -> u16 {
    202
}

fn default_keystore_env() -> String {
    "XTR_OUTBOUND_KEYSTORE_PASSWORD".into()
}

fn default_content_type() -> String {
    "application/xml; charset=utf-8".into()
}

#[derive(Debug)]
pub struct Message {
    pub qname: QName,
    pub cfg: MessageCfg,
}

#[derive(Debug)]
pub struct XmlService {
    pub group: String,
    pub name: String,
    pub schema: Schema,
    pub messages: BTreeMap<String, Message>,
    pub sidecar: XmlSidecar,
    /// Built at boot when `outbound:` is configured.
    pub http: Option<reqwest::Client>,
}

impl XmlService {
    pub fn backend_for(&self, message: &str) -> Option<String> {
        self.sidecar.backend_for(message)
    }

    pub fn url_for(&self, message: &str) -> Option<String> {
        let ob = self.sidecar.outbound.as_ref()?;
        self.messages
            .get(message)
            .and_then(|m| m.cfg.url.clone())
            .or_else(|| ob.url.clone())
    }

    pub fn timeout_for(&self, message: &str, default: Duration) -> Duration {
        let secs = self
            .messages
            .get(message)
            .and_then(|m| m.cfg.timeout_secs)
            .or_else(|| self.sidecar.outbound.as_ref().and_then(|o| o.timeout_secs));
        secs.map(Duration::from_secs).unwrap_or(default)
    }
}

#[derive(Debug, Default)]
pub struct XmlRegistry {
    services: BTreeMap<(String, String), Arc<XmlService>>,
}

impl XmlRegistry {
    pub fn get(&self, group: &str, name: &str) -> Option<&Arc<XmlService>> {
        self.services.get(&(group.to_string(), name.to_string()))
    }

    pub fn is_empty(&self) -> bool {
        self.services.is_empty()
    }

    pub fn len(&self) -> usize {
        self.services.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<XmlService>> {
        self.services.values()
    }

    /// `inbound.path` aliases that shadow a DSL endpoint `/:group/:service`
    /// (the static alias would win and silently take its POSTs). With a
    /// separate `inbound.port` the aliases live on their own listener: no clash.
    pub fn alias_conflicts(
        &self,
        is_dsl: impl Fn(&str, &str) -> bool,
        separate_port: bool,
    ) -> Vec<String> {
        let mut out = Vec::new();
        if separate_port {
            // The alias is on its own listener. It cannot shadow a DSL endpoint.
            return out;
        }
        for svc in self.iter() {
            let Some(p) = svc.sidecar.inbound.as_ref().and_then(|i| i.path.as_deref()) else {
                continue;
            };
            let mut parts = p.trim_matches('/').split('/');
            if let (Some(g), Some(n), None) = (parts.next(), parts.next(), parts.next()) {
                if is_dsl(g, n) {
                    out.push(format!(
                        "inbound.path {p} of /xml-in/{}/{} shadows the DSL endpoint {g}/{n}",
                        svc.group, svc.name
                    ));
                }
            }
        }
        out
    }

    /// How many sidecars ask for each lane (same meaning as the SOAP
    /// lanes' summary, so the shared-listener exposure rule applies).
    pub fn summary(&self) -> crate::inbound::LaneSummary {
        crate::inbound::LaneSummary {
            inbound: self
                .services
                .values()
                .filter(|s| s.sidecar.inbound.is_some())
                .count(),
            outbound: self
                .services
                .values()
                .filter(|s| s.sidecar.outbound.is_some())
                .count(),
        }
    }
}

const RESERVED_PATHS: [&str; 6] = [
    "/health",
    "/api",
    "/soap-in",
    "/soap-out",
    "/xml-in",
    "/xml-out",
];

fn sidecar_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    Ok(crate::fswalk::files(root)?
        .into_iter()
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name.ends_with(".xml.yaml") || name.ends_with(".xml.yml")
        })
        .collect())
}

/// How many raw-XML sidecars under `dir` enable each lane — reads and
/// parses the YAML only (no XSDs, keystores or clients), for
/// `xtr doctor`. Unparseable sidecars are skipped here; boot refuses them.
pub fn count_lanes(dir: Option<&Path>) -> crate::inbound::LaneSummary {
    let mut sum = crate::inbound::LaneSummary {
        inbound: 0,
        outbound: 0,
    };
    let Some(root) = dir.filter(|d| d.is_dir()) else {
        return sum;
    };
    let Ok(files) = sidecar_files(root) else {
        return sum; // boot refuses this directory
    };
    for sc in files {
        let Ok(text) = fs::read_to_string(&sc) else {
            continue;
        };
        if let Ok(x) = serde_yaml_ng::from_str::<XmlSidecar>(&text) {
            sum.inbound += usize::from(x.inbound.is_some());
            sum.outbound += usize::from(x.outbound.is_some());
        }
    }
    sum
}

/// Load every `*.xml.yaml` under `dir`. Any problem refuses boot with
/// the full list — a sidecar is explicit opt-in config, so a typo must
/// not silently drop a published lane.
pub fn load_all(dir: Option<&Path>, cfg: &AppConfig) -> Result<XmlRegistry, String> {
    let mut reg = XmlRegistry::default();
    let Some(root) = dir.filter(|d| d.is_dir()) else {
        return Ok(reg);
    };
    let mut problems = Vec::new();
    let mut paths: BTreeMap<String, PathBuf> = BTreeMap::new();
    for sc in sidecar_files(root)? {
        match prepare(root, &sc, cfg) {
            Ok(svc) => {
                let key = (svc.group.clone(), svc.name.clone());
                if reg.services.contains_key(&key) {
                    problems.push(format!(
                        "{}: endpoint name /xml-in/{}/{} is already taken — rename one sidecar",
                        sc.display(),
                        key.0,
                        key.1
                    ));
                    continue;
                }
                if let Some(p) = svc.sidecar.inbound.as_ref().and_then(|i| i.path.clone()) {
                    if let Some(first) = paths.get(&p) {
                        problems.push(format!(
                            "{}: inbound.path {p} is already used by {}",
                            sc.display(),
                            first.display()
                        ));
                        continue;
                    }
                    paths.insert(p, sc.clone());
                }
                reg.services.insert(key, Arc::new(svc));
            }
            Err(e) => problems.push(format!("{}: {e}", sc.display())),
        }
    }
    if !problems.is_empty() {
        return Err(format!(
            "invalid raw-XML lane configuration:\n  {}",
            problems.join("\n  ")
        ));
    }
    for svc in reg.iter() {
        let msgs: Vec<&str> = svc.messages.keys().map(String::as_str).collect();
        if svc.sidecar.inbound.is_some() {
            tracing::info!(endpoint = %format!("/xml-in/{}/{}", svc.group, svc.name), messages = ?msgs,
                alias = ?svc.sidecar.inbound.as_ref().and_then(|i| i.path.as_deref()),
                "inbound raw-XML endpoint registered");
        }
        if svc.sidecar.outbound.is_some() {
            tracing::info!(endpoint = %format!("/xml-out/{}/{}/<message>", svc.group, svc.name), messages = ?msgs,
                "raw-XML outbound registered");
        }
    }
    Ok(reg)
}

fn http_url(what: &str, url: &str) -> Result<(), String> {
    let u = url::Url::parse(url).map_err(|e| format!("{what} {url:?}: {e}"))?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err(format!("{what} {url:?}: only http/https allowed"));
    }
    Ok(())
}

fn prepare(root: &Path, sidecar: &Path, cfg: &AppConfig) -> Result<XmlService, String> {
    let sc = load_sidecar(sidecar)?;
    let (group, name) = lane_id(root, sidecar)?;

    let dir = sidecar.parent().unwrap_or(root);
    let loader = contract::fs_loader(dir);
    let entries = schema_entries(&sc, &loader)?;
    let (schema, _files) = contract::load_xsd(&entries, &loader)?;

    let messages = resolve_messages(&sc, &schema)?;
    check_directions(&sc, &messages)?;
    check_inbound(&sc, &messages)?;
    let http = build_outbound(&sc, &messages, cfg)?;

    Ok(XmlService {
        group,
        name,
        schema,
        messages,
        sidecar: sc,
        http,
    })
}

/// Reads and parses a sidecar and checks its top-level shape.
fn load_sidecar(sidecar: &Path) -> Result<XmlSidecar, String> {
    let sc_text = fs::read_to_string(sidecar).map_err(|e| format!("reading sidecar: {e}"))?;
    let sc: XmlSidecar =
        serde_yaml_ng::from_str(&sc_text).map_err(|e| format!("invalid sidecar: {e}"))?;
    if sc.inbound.is_none() && sc.outbound.is_none() {
        return Err("sidecar enables neither `inbound:` nor `outbound:`".into());
    }
    if sc.schemas.is_empty() {
        return Err("`schemas:` must list at least one entry XSD".into());
    }
    if sc.messages.is_empty() {
        return Err("`messages:` must list at least one message".into());
    }
    Ok(sc)
}

/// `(group, name)` of a lane from the path of its sidecar, like the SOAP
/// lanes: the first directory under `root` is the group, deeper
/// directories join the name with `-`.
fn lane_id(root: &Path, sidecar: &Path) -> Result<(String, String), String> {
    let rel: Vec<String> = sidecar
        .parent()
        .and_then(|p| p.strip_prefix(root).ok())
        .map(|p| {
            p.components()
                .filter_map(|c| c.as_os_str().to_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let file = sidecar
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or("sidecar file name is not UTF-8")?;
    let stem = file
        .strip_suffix(".xml.yaml")
        .or_else(|| file.strip_suffix(".xml.yml"))
        .ok_or("sidecar must be named <name>.xml.yaml")?;
    match rel.as_slice() {
        [] => Err("a .xml.yaml sidecar must live in a <group>/ subdirectory".into()),
        [g] => Ok((g.clone(), stem.to_string())),
        [g, rest @ ..] => Ok((g.clone(), format!("{}-{stem}", rest.join("-")))),
    }
}

/// Loads the entry XSD files that the sidecar lists, as `(file name, text)`.
fn schema_entries(
    sc: &XmlSidecar,
    loader: &contract::SchemaLoader,
) -> Result<Vec<(String, String)>, String> {
    let mut entries = Vec::new();
    for s in &sc.schemas {
        if s.contains(['/', '\\']) {
            return Err(format!(
                "schema {s:?}: list plain file names (the file must sit next to the sidecar)"
            ));
        }
        let (fname, _path, xml) = loader(s).ok_or_else(|| {
            format!("schema {s:?} not found next to the sidecar (or not a plain file name)")
        })?;
        entries.push((fname, xml));
    }
    Ok(entries)
}

/// Maps each message of the sidecar to a global element of the schemas.
fn resolve_messages(sc: &XmlSidecar, schema: &Schema) -> Result<BTreeMap<String, Message>, String> {
    let mut messages = BTreeMap::new();
    for (m, mc) in &sc.messages {
        let same_local = schema.elements.keys().filter(|q| &q.local == m).count();
        if same_local > 1 {
            return Err(format!(
                "message {m:?} is a global element in {same_local} namespaces — the lane cannot tell them apart; keep one"
            ));
        }
        let (q, _) = schema
            .global_by_local(m)
            .ok_or_else(|| format!("message {m:?} is not a global element of the schemas"))?;
        messages.insert(
            m.clone(),
            Message {
                qname: q.clone(),
                cfg: mc.clone().unwrap_or_default(),
            },
        );
    }
    Ok(messages)
}

/// The direction of each message must match the sections the sidecar has.
fn check_directions(sc: &XmlSidecar, messages: &BTreeMap<String, Message>) -> Result<(), String> {
    let any_in = messages.values().any(|m| m.cfg.direction.inbound());
    let any_out = messages.values().any(|m| m.cfg.direction.outbound());
    for (m, msg) in messages {
        let d = msg.cfg.direction;
        if d.inbound() && !d.outbound() && sc.inbound.is_none() {
            return Err(format!(
                "message {m} is `direction: in` but the sidecar has no `inbound:`"
            ));
        }
        if d.outbound() && !d.inbound() && sc.outbound.is_none() {
            return Err(format!(
                "message {m} is `direction: out` but the sidecar has no `outbound:`"
            ));
        }
    }
    if sc.inbound.is_some() && !any_in {
        return Err("`inbound:` is set but every message is `direction: out`".into());
    }
    if sc.outbound.is_some() && !any_out {
        return Err("`outbound:` is set but every message is `direction: in`".into());
    }
    Ok(())
}

/// Checks the `inbound:` section and the backend / forward settings.
fn check_inbound(sc: &XmlSidecar, messages: &BTreeMap<String, Message>) -> Result<(), String> {
    let Some(ib) = &sc.inbound else {
        for (m, msg) in messages {
            if msg.cfg.backend.is_some() || msg.cfg.forward.is_some() {
                return Err(format!(
                    "message {m}: `backend`/`forward` need an `inbound:` section"
                ));
            }
        }
        return Ok(());
    };
    if !(200..300).contains(&ib.success_status) {
        return Err(format!(
            "inbound.success_status {} must be a 2xx code",
            ib.success_status
        ));
    }
    if let Some(p) = &ib.path {
        check_inbound_path(p)?;
    }
    if ib.relay_statuses.iter().any(|c| !(400..500).contains(c)) {
        return Err("inbound.relay_statuses must only contain 4xx codes".into());
    }
    for (m, msg) in messages {
        check_message_target(sc, m, msg)?;
    }
    Ok(())
}

/// `inbound.path` must be a literal absolute path outside the reserved prefixes.
fn check_inbound_path(p: &str) -> Result<(), String> {
    let bad = !p.starts_with('/')
        || p.len() < 2
        || p.contains(['{', '}', ':', '*', '?', '#', ' '])
        || RESERVED_PATHS
            .iter()
            .any(|r| p.trim_end_matches('/') == *r || p.starts_with(&format!("{r}/")));
    if bad {
        return Err(format!(
            "inbound.path {p:?} must be a literal absolute path that does not use a reserved prefix"
        ));
    }
    Ok(())
}

/// The backend or forward target of one inbound message.
fn check_message_target(sc: &XmlSidecar, m: &str, msg: &Message) -> Result<(), String> {
    if !msg.cfg.direction.inbound() {
        if msg.cfg.backend.is_some() || msg.cfg.forward.is_some() {
            return Err(format!(
                "message {m}: `backend`/`forward` cannot apply to a `direction: out` message"
            ));
        }
        return Ok(());
    }
    if let Some(f) = &msg.cfg.forward {
        http_url("forward URL", f)?;
        if msg.cfg.backend.is_some() {
            return Err(format!(
                "message {m}: set either `forward` or `backend`, not both"
            ));
        }
        return Ok(());
    }
    let url = sc.backend_for(m).ok_or_else(|| {
        format!("message {m} has no backend (set inbound.backend or messages.{m}.backend)")
    })?;
    http_url("backend URL", &url)
}

/// Checks the `outbound:` section and builds the HTTP client for it.
fn build_outbound(
    sc: &XmlSidecar,
    messages: &BTreeMap<String, Message>,
    cfg: &AppConfig,
) -> Result<Option<reqwest::Client>, String> {
    let Some(ob) = &sc.outbound else {
        for (m, msg) in messages {
            if msg.cfg.url.is_some() || msg.cfg.timeout_secs.is_some() {
                return Err(format!(
                    "message {m}: `url`/`timeout_secs` need an `outbound:` section"
                ));
            }
        }
        return Ok(None);
    };
    let mut seen_url = None;
    for (m, mc) in &sc.messages {
        if !messages[m].cfg.direction.outbound() {
            if mc
                .as_ref()
                .is_some_and(|c| c.url.is_some() || c.timeout_secs.is_some())
            {
                return Err(format!(
                    "message {m}: `url`/`timeout_secs` cannot apply to a `direction: in` message"
                ));
            }
            continue;
        }
        let url = check_peer_target(m, mc.as_ref(), ob, cfg)?;
        seen_url.get_or_insert(url);
    }
    if ob.content_type.trim().is_empty()
        || !ob.content_type.is_ascii()
        || reqwest::header::HeaderValue::from_str(&ob.content_type).is_err()
    {
        return Err("outbound.content_type is empty or not a valid header value".into());
    }
    Ok(Some(build_http(ob, seen_url.unwrap_or_default(), cfg)?))
}

/// Peer URL (SSRF guard) and timeout of one outbound message. Returns the URL.
fn check_peer_target(
    m: &str,
    mc: Option<&MessageCfg>,
    ob: &XmlOutbound,
    cfg: &AppConfig,
) -> Result<String, String> {
    let url = mc
        .and_then(|c| c.url.clone())
        .or_else(|| ob.url.clone())
        .ok_or_else(|| {
            format!("message {m} has no peer URL (set outbound.url or messages.{m}.url)")
        })?;
    // Same SSRF guard as WSDL-declared URLs.
    crate::wsdl::url_guard::validate_upstream_url(&url, &cfg.wsdl)
        .map_err(|e| format!("message {m}: {e}"))?;
    let limit = cfg.limits.request_timeout_secs;
    if let Some(t) = mc.and_then(|c| c.timeout_secs).or(ob.timeout_secs) {
        if t == 0 || t > limit {
            return Err(format!(
                "message {m}: timeout_secs {t} must be 1..={limit} (limits.request_timeout_secs)"
            ));
        }
    }
    Ok(url)
}

fn build_http(
    ob: &XmlOutbound,
    any_url: String,
    cfg: &AppConfig,
) -> Result<reqwest::Client, String> {
    if let Some(ca) = &ob.trust_ca_path {
        if !ca.is_file() {
            return Err(format!("trust_ca_path {} does not exist", ca.display()));
        }
    }
    match &ob.keystore_path {
        Some(ks) => {
            if !ks.is_file() {
                return Err(format!("keystore_path {} does not exist", ks.display()));
            }
            let pw = std::env::var(&ob.keystore_password_env).map_err(|_| {
                format!(
                    "env var {} (keystore password) is unset",
                    ob.keystore_password_env
                )
            })?;
            let ss = SecurityServer {
                url: any_url,
                keystore_path: ks.clone(),
                keystore_password_env: ob.keystore_password_env.clone(),
                trust_ca_path: ob.trust_ca_path.clone(),
            };
            crate::executor::build_mtls_client(&ss, &pw, &cfg.limits).map_err(|e| e.to_string())
        }
        None => {
            let mut b = reqwest::Client::builder()
                .timeout(Duration::from_secs(cfg.limits.request_timeout_secs))
                .min_tls_version(reqwest::tls::Version::TLS_1_2)
                .redirect(reqwest::redirect::Policy::none())
                .no_gzip()
                .no_brotli()
                .no_deflate();
            if let Some(ca) = &ob.trust_ca_path {
                let bytes = fs::read(ca).map_err(|e| format!("reading {}: {e}", ca.display()))?;
                let cert = reqwest::Certificate::from_pem(&bytes)
                    .or_else(|_| reqwest::Certificate::from_der(&bytes))
                    .map_err(|e| format!("parsing {}: {e}", ca.display()))?;
                b = b.add_root_certificate(cert);
            }
            b.build().map_err(|e| e.to_string())
        }
    }
}
