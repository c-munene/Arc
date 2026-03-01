use arc_common::{ArcError, Result};
use arc_config::{CompiledDownstreamCertificate, CompiledDownstreamTls, CompiledUpstreamTls};

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::client::Resumption;
use rustls::{ClientConfig, RootCertStore, ServerConfig};

use std::collections::HashMap;
use std::sync::Arc;

pub fn build_downstream_server_config(tls: &CompiledDownstreamTls) -> Result<Arc<ServerConfig>> {
    let resolver = Arc::new(StaticCertResolver::from_compiled(
        tls.certificates.as_ref(),
    )?);

    let mut server_cfg = ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(resolver);

    let support_h2 = cfg!(feature = "http2");
    let acme_enabled = tls.acme.as_ref().map(|a| a.enabled).unwrap_or(false);

    server_cfg.alpn_protocols = if tls.enable_h2 && support_h2 {
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

    Ok(Arc::new(server_cfg))
}

pub struct UpstreamTlsRuntime {
    pub config: Arc<ClientConfig>,
    pub server_name: ServerName<'static>,
}

pub fn build_upstream_client_config(
    tls: &CompiledUpstreamTls,
    fallback_sni: &str,
) -> Result<UpstreamTlsRuntime> {
    let server_name_str = tls
        .server_name
        .as_deref()
        .unwrap_or(fallback_sni)
        .trim()
        .to_ascii_lowercase();

    let server_name = ServerName::try_from(server_name_str.as_str()).map_err(|_| {
        ArcError::config(format!(
            "invalid upstream tls server_name: {server_name_str}"
        ))
    })?;

    let mut roots = RootCertStore::empty();
    if !tls.insecure_skip_verify {
        let Some(ca_bytes) = tls.ca_pem.as_ref() else {
            return Err(ArcError::config(
                "upstream tls ca_pem is required when insecure_skip_verify=false".to_string(),
            ));
        };

        let ca_chain: Vec<CertificateDer<'static>> =
            CertificateDer::pem_slice_iter(ca_bytes.as_ref())
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|_| ArcError::config("invalid upstream ca pem".to_string()))?;

        if ca_chain.is_empty() {
            return Err(ArcError::config(
                "upstream ca pem has no certificate".to_string(),
            ));
        }
        for c in ca_chain {
            let _ = roots.add(c);
        }
    }

    let client_chain: Vec<CertificateDer<'static>> =
        CertificateDer::pem_slice_iter(tls.client_cert_pem.as_ref())
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|_| ArcError::config("invalid upstream client cert pem".to_string()))?;
    if client_chain.is_empty() {
        return Err(ArcError::config(
            "upstream client cert pem has no certificate".to_string(),
        ));
    }

    let key = PrivateKeyDer::from_pem_slice(tls.client_key_pem.as_ref())
        .map_err(|_| ArcError::config("invalid upstream client key pem".to_string()))?;

    let mut client_cfg = if tls.insecure_skip_verify {
        ClientConfig::builder()
            .with_root_certificates(RootCertStore::empty())
            .with_client_auth_cert(client_chain, key)
            .map_err(|_| ArcError::config("invalid upstream client auth cert/key".to_string()))?
    } else {
        ClientConfig::builder()
            .with_root_certificates(roots)
            .with_client_auth_cert(client_chain, key)
            .map_err(|_| ArcError::config("invalid upstream client auth cert/key".to_string()))?
    };

    client_cfg.resumption = if tls.enable_resumption {
        Resumption::in_memory_sessions(1024)
    } else {
        Resumption::disabled()
    };

    Ok(UpstreamTlsRuntime {
        config: Arc::new(client_cfg),
        server_name: server_name.to_owned(),
    })
}

#[derive(Debug)]
struct StaticCertResolver {
    default_cert: Arc<CertifiedKey>,
    exact: HashMap<Arc<str>, Arc<CertifiedKey>>,
    wildcard: Vec<(Arc<str>, Arc<CertifiedKey>)>, // suffix, key
}

impl StaticCertResolver {
    fn from_compiled(certs: &[arc_config::CompiledDownstreamCertificate]) -> Result<Self> {
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
            default_cert: default_cert
                .ok_or_else(|| ArcError::config("no downstream tls certificate".to_string()))?,
            exact,
            wildcard,
        })
    }
}

impl ResolvesServerCert for StaticCertResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
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
            if let Some(v) = self.exact.get(host.as_str()) {
                return Some(v.clone());
            }
            for (suffix, ck) in &self.wildcard {
                if host.len() <= suffix.len() {
                    continue;
                }
                if host.ends_with(suffix.as_ref())
                    && host.as_bytes()[host.len().saturating_sub(suffix.len() + 1)] == b'.'
                {
                    return Some(ck.clone());
                }
            }
        }
        Some(self.default_cert.clone())
    }
}

fn parse_certified_key(cert: &CompiledDownstreamCertificate) -> Result<Arc<CertifiedKey>> {
    if (cert.cert_pem.is_empty() || cert.key_pem.is_empty()) && cert.acme_managed {
        return arc_acme::placeholder_certified_key(cert.wildcard, cert.sni.as_ref());
    }

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