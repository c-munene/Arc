use arc_common::{ArcError, Result};
use arc_config::{CompiledUpstreamTls, TlsMinVersionConfig};

use rustls::client::Resumption;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, RootCertStore};

use std::sync::Arc;

pub struct UpstreamTlsRuntime {
    pub config: Arc<ClientConfig>,
    pub server_name: ServerName<'static>,
}

#[derive(Debug)]
struct SkipServerVerification(Arc<rustls::crypto::CryptoProvider>);

impl SkipServerVerification {
    fn new(provider: Arc<rustls::crypto::CryptoProvider>) -> Arc<Self> {
        Arc::new(Self(provider))
    }
}

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
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
    if tls.verify_server {
        let Some(ca_bytes) = tls.ca_pem.as_ref() else {
            return Err(ArcError::config(
                "upstream tls ca_pem is required when tls.verify_server=true".to_string(),
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

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = match tls.min_version {
        TlsMinVersionConfig::V1_2 => ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|e| ArcError::config(format!("invalid upstream tls min_version: {e}")))?,
        TlsMinVersionConfig::V1_3 => ClientConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|e| ArcError::config(format!("invalid upstream tls min_version: {e}")))?,
    };
    let mut client_cfg = builder
        .with_root_certificates(if tls.verify_server {
            roots
        } else {
            RootCertStore::empty()
        })
        .with_client_auth_cert(client_chain, key)
        .map_err(|_| ArcError::config("invalid upstream client auth cert/key".to_string()))?;
    if !tls.verify_server {
        client_cfg
            .dangerous()
            .set_certificate_verifier(SkipServerVerification::new(provider.clone()));
    }

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
