//! downstream_tls — rustls ServerConfig builder + lock-free SNI cert resolver.

use arc_common::{ArcError, Result};
use arc_config::{CompiledDownstreamCertificate, SharedConfig};

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::ServerConfig;

use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct DownstreamTls {
    pub generation: u64,
    pub enable_h2: bool,
    pub server_cfg: Arc<ServerConfig>,
}

impl DownstreamTls {
    pub fn build(cfg: &SharedConfig) -> Result<Option<Self>> {
        let Some(tls) = cfg.downstream_tls.as_ref() else {
            return Ok(None);
        };

        let resolver = Arc::new(StaticCertResolver::from_compiled(&tls.certificates)?);

        let mut scfg = ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(resolver);

        let acme_enabled = tls.acme.as_ref().map(|a| a.enabled).unwrap_or(false);

        // ✅ ALPN：支持热重载（因为 worker 会在 generation 变化时重建 scfg）
        // ✅ 如果启用 ACME：加上 acme-tls/1 以支持 TLS-ALPN-01
        scfg.alpn_protocols = if tls.enable_h2 {
            if acme_enabled {
                vec![b"acme-tls/1".to_vec(), b"h2".to_vec(), b"http/1.1".to_vec()]
            } else {
                vec![b"h2".to_vec(), b"http/1.1".to_vec()]
            }
        } else if acme_enabled {
            vec![b"acme-tls/1".to_vec(), b"http/1.1".to_vec()]
        } else {
            vec![b"http/1.1".to_vec()]
        };

        Ok(Some(Self {
            generation: cfg.generation,
            enable_h2: tls.enable_h2,
            server_cfg: Arc::new(scfg),
        }))
    }
}

#[derive(Debug)]
struct StaticCertResolver {
    default_cert: Arc<CertifiedKey>,
    exact: HashMap<Arc<str>, Arc<CertifiedKey>>,
    wildcard: Vec<(Arc<str>, Arc<CertifiedKey>)>, // suffix -> cert (sorted desc)
}

impl StaticCertResolver {
    fn from_compiled(certs: &[CompiledDownstreamCertificate]) -> Result<Self> {
        if certs.is_empty() {
            return Err(ArcError::config(
                "downstream_tls.certificates must not be empty".to_string(),
            ));
        }

        let mut default_cert: Option<Arc<CertifiedKey>> = None;
        let mut exact = HashMap::new();
        let mut wildcard: Vec<(Arc<str>, Arc<CertifiedKey>)> = Vec::new();

        for c in certs {
            let ck = parse_certified_key(c)?;
            if default_cert.is_none() {
                default_cert = Some(ck.clone());
            }
            if c.wildcard {
                wildcard.push((c.sni.clone(), ck));
            } else {
                exact.insert(c.sni.clone(), ck);
            }
        }
        wildcard.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

        Ok(Self {
            default_cert: default_cert.unwrap(),
            exact,
            wildcard,
        })
    }
}

impl ResolvesServerCert for StaticCertResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        // ✅ TLS-ALPN-01 fast path: ALPN == acme-tls/1
        if client_hello.alpn() == Some(b"acme-tls/1".as_ref()) {
            if let Some(store) = arc_acme::global_challenge_store() {
                if let Some(host) = client_hello.server_name().and_then(normalize_host) {
                    if let Some(ck) = store.get_tls_alpn01(host) {
                        return Some(ck);
                    }
                }
            }
        }

        let host = client_hello
            .server_name()
            .and_then(normalize_host)
            .map(|v| v.to_ascii_lowercase());

        if let Some(host) = host {
            if let Some(c) = self.exact.get(host.as_str()) {
                return Some(c.clone());
            }
            for (suffix, cert) in &self.wildcard {
                if host.len() <= suffix.len() {
                    continue;
                }
                if host.ends_with(suffix.as_ref())
                    && host.as_bytes()[host.len().saturating_sub(suffix.len() + 1)] == b'.'
                {
                    return Some(cert.clone());
                }
            }
        }
        Some(self.default_cert.clone())
    }
}

fn parse_certified_key(cert: &CompiledDownstreamCertificate) -> Result<Arc<CertifiedKey>> {
    // cold start: acme-managed certs may be missing => placeholder
    if (cert.cert_pem.is_empty() || cert.key_pem.is_empty()) && cert.acme_managed {
        return arc_acme::placeholder_certified_key(cert.wildcard, cert.sni.as_ref());
    }

    let chain: Vec<CertificateDer<'static>> =
        CertificateDer::pem_slice_iter(cert.cert_pem.as_ref())
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|_| ArcError::config("invalid downstream tls cert pem".to_string()))?;

    if chain.is_empty() {
        return Err(ArcError::config(
            "downstream tls cert pem has no certificate".to_string(),
        ));
    }

    let key = PrivateKeyDer::from_pem_slice(cert.key_pem.as_ref())
        .map_err(|_| ArcError::config("invalid downstream tls key pem".to_string()))?;
    let signing_key = rustls::crypto::ring::sign::any_supported_type(&key)
        .map_err(|_| ArcError::config("unsupported downstream tls private key".to_string()))?;

    Ok(Arc::new(CertifiedKey::new(chain, signing_key)))
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