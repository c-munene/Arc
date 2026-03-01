use arc_common::{ArcError, Result};
use arc_config::{CompiledDownstreamCertificate, SharedConfig, TlsMinVersionConfig};

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::ServerConfig;

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::acme;

#[derive(Clone)]
pub struct DownstreamTls {
    pub server_cfg: Arc<ServerConfig>,
}

impl DownstreamTls {
    pub fn build(cfg: &SharedConfig) -> Result<Option<Self>> {
        let Some(tls) = cfg.downstream_tls.as_ref() else {
            return Ok(None);
        };

        let static_resolver = Arc::new(StaticCertResolver::from_compiled(&tls.certificates)?);
        let resolver: Arc<dyn ResolvesServerCert> = match acme::load() {
            Some(acme_svc) => Arc::new(CompositeCertResolver {
                static_resolver: static_resolver.clone(),
                acme: acme_svc,
            }),
            None => static_resolver.clone(),
        };

        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut server_cfg = match tls.min_version {
            TlsMinVersionConfig::V1_2 => ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|e| {
                    ArcError::config(format!("invalid downstream tls min_version: {e}"))
                })?,
            TlsMinVersionConfig::V1_3 => ServerConfig::builder_with_provider(provider)
                .with_protocol_versions(&[&rustls::version::TLS13])
                .map_err(|e| {
                    ArcError::config(format!("invalid downstream tls min_version: {e}"))
                })?,
        }
        .with_no_client_auth()
        .with_cert_resolver(resolver);

        server_cfg.alpn_protocols = if tls.enable_h2 {
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        } else {
            vec![b"http/1.1".to_vec()]
        };
        if acme::is_enabled() {
            server_cfg.alpn_protocols.push(b"acme-tls/1".to_vec());
        }

        Ok(Some(Self {
            server_cfg: Arc::new(server_cfg),
        }))
    }
}

#[derive(Debug)]
struct StaticCertResolver {
    default_cert: Arc<CertifiedKey>,
    exact: BTreeMap<String, Arc<CertifiedKey>>,
    wildcard: Vec<(String, Arc<CertifiedKey>)>,
}

impl StaticCertResolver {
    fn from_compiled(certs: &[CompiledDownstreamCertificate]) -> Result<Self> {
        if certs.is_empty() {
            return Err(ArcError::config(
                "downstream_tls.certificates must not be empty".to_string(),
            ));
        }

        let mut default_cert: Option<Arc<CertifiedKey>> = None;
        let mut exact = BTreeMap::new();
        let mut wildcard = Vec::new();

        for c in certs {
            let ck = parse_certified_key(c)?;
            if default_cert.is_none() {
                default_cert = Some(ck.clone());
            }
            if c.wildcard {
                wildcard.push((c.sni.to_ascii_lowercase(), ck));
            } else {
                exact.insert(c.sni.to_ascii_lowercase(), ck);
            }
        }

        wildcard.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

        Ok(Self {
            default_cert: default_cert
                .ok_or_else(|| ArcError::config("no downstream tls certificate".to_string()))?,
            exact,
            wildcard,
        })
    }

    fn lookup_for_sni(&self, sni: &str) -> Option<Arc<CertifiedKey>> {
        if let Some(v) = self.exact.get(sni) {
            return Some(v.clone());
        }
        for (suffix, ck) in &self.wildcard {
            if sni.len() <= suffix.len() {
                continue;
            }
            if sni.ends_with(suffix.as_str())
                && sni.as_bytes()[sni.len().saturating_sub(suffix.len() + 1)] == b'.'
            {
                return Some(ck.clone());
            }
        }
        None
    }
}

impl ResolvesServerCert for StaticCertResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let host = client_hello
            .server_name()
            .and_then(normalize_host)
            .map(|v| v.to_ascii_lowercase());

        if let Some(host) = host {
            if let Some(v) = self.lookup_for_sni(host.as_str()) {
                return Some(v);
            }
        }
        Some(self.default_cert.clone())
    }
}

struct CompositeCertResolver {
    static_resolver: Arc<StaticCertResolver>,
    acme: Arc<arc_acme::AcmeService>,
}

impl fmt::Debug for CompositeCertResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompositeCertResolver")
            .finish_non_exhaustive()
    }
}

impl ResolvesServerCert for CompositeCertResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let host = client_hello
            .server_name()
            .and_then(normalize_host)
            .map(|v| v.to_ascii_lowercase());

        if let Some(ref host) = host {
            if let Some(acme_resolver) = self.acme.resolver_for_domain(host) {
                if let Some(cert) = acme_resolver.resolve(client_hello) {
                    return Some(cert);
                }
                if let Some(cert) = self.static_resolver.lookup_for_sni(host.as_str()) {
                    return Some(cert);
                }
                return None;
            }
        }

        self.static_resolver.resolve(client_hello)
    }
}

fn parse_certified_key(cert: &CompiledDownstreamCertificate) -> Result<Arc<CertifiedKey>> {
    let cert_chain: Vec<CertificateDer<'static>> =
        CertificateDer::pem_slice_iter(cert.cert_pem.as_ref())
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|_| ArcError::config("invalid downstream tls cert pem".to_string()))?;

    if cert_chain.is_empty() {
        return Err(ArcError::config(
            "downstream tls cert pem has no certificate".to_string(),
        ));
    }

    let key = PrivateKeyDer::from_pem_slice(cert.key_pem.as_ref())
        .map_err(|_| ArcError::config("invalid downstream tls key pem".to_string()))?;
    let signing_key = rustls::crypto::ring::sign::any_supported_type(&key)
        .map_err(|_| ArcError::config("unsupported downstream tls private key".to_string()))?;

    Ok(Arc::new(CertifiedKey::new(cert_chain, signing_key)))
}

#[inline]
fn normalize_host(host: &str) -> Option<&str> {
    let host = host.trim().trim_end_matches('.');
    if host.is_empty() {
        return None;
    }
    if host
        .as_bytes()
        .iter()
        .any(|b| b.is_ascii_whitespace() || *b == b'/')
    {
        return None;
    }
    Some(host)
}
