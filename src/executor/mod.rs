//! Request executor — routes an expanded SOAP envelope to the
//! upstream and returns the raw XML response body.
//!
//! Two backends selected per-DSL:
//! * `plain` — direct HTTPS for services with `service:` set
//! * `ss` — mTLS via PKCS12 identity to the X-Road Security Server
//!
//! `Executor::new` builds both backends at startup; each request
//! picks one via `dispatch`. Fixes JVM bug #6 — no trust-all TLS;
//! we use the system trust store. Fixes JVM bug #15 — no
//! `Towarsd` typo.

use crate::config::AppConfig;
use crate::dsl::XRoadTemplate;
use crate::error::XtrError;

pub mod plain;
pub mod ss;

/// Assembled request executor. Holds both backend clients so a
/// single `Executor` is enough for any DSL.
#[derive(Clone)]
pub struct Executor {
    plain: plain::PlainExecutor,
    ss: Option<ss::SsExecutor>,
}

impl Executor {
    pub fn new(cfg: &AppConfig) -> Result<Self, XtrError> {
        let plain = plain::PlainExecutor::new(&cfg.limits)?;
        let ss = match &cfg.security_server {
            Some(ss_cfg) => {
                let password = cfg
                    .keystore_password()?
                    .expect("security_server configured but keystore_password returned None");
                Some(ss::SsExecutor::new(ss_cfg, &password, &cfg.limits)?)
            }
            None => None,
        };
        Ok(Self { plain, ss })
    }

    /// Send the expanded envelope to the DSL's target and return
    /// the raw XML response body. Content-Type is set to
    /// `text/xml; charset=utf-8` on the outbound (task 003 tracks
    /// hardening this).
    pub async fn dispatch(
        &self,
        template: &XRoadTemplate,
        envelope: String,
    ) -> Result<String, XtrError> {
        match &template.service {
            Some(uri) => self.plain.execute(uri, &template.method, envelope).await,
            None => {
                let ss = self.ss.as_ref().ok_or_else(|| {
                    XtrError::Internal(
                        "DSL has no `service:` field but no security_server \
                         is configured. Set security_server.url in xtr.yaml \
                         or add a service: URI to the DSL."
                            .into(),
                    )
                })?;
                ss.execute(&template.method, envelope).await
            }
        }
    }
}
