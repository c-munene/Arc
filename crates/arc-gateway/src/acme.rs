use std::path::Path;
use std::sync::{Arc, OnceLock};

use arc_acme::{AcmeRuntimeConfig, AcmeService, ChallengeType};
use arc_common::{ArcError, Result};
use arc_swap::ArcSwapOption;
use serde::Deserialize;

static ACME_SERVICE: OnceLock<ArcSwapOption<AcmeService>> = OnceLock::new();

fn global() -> &'static ArcSwapOption<AcmeService> {
    ACME_SERVICE.get_or_init(|| ArcSwapOption::from(None))
}

pub fn load() -> Option<Arc<AcmeService>> {
    global().load_full()
}

pub fn is_enabled() -> bool {
    load().is_some()
}

/// Minimal typed view of config JSON to extract `downstream_tls.acme`.
#[derive(Debug, Deserialize)]
struct ConfigAcmeWrapper {
    #[serde(default)]
    downstream_tls: Option<DownstreamTlsAcmeWrapper>,
}

#[derive(Debug, Deserialize)]
struct DownstreamTlsAcmeWrapper {
    #[serde(default)]
    acme: Option<AcmeJsonConfig>,
}

#[derive(Debug, Clone, Deserialize)]
struct AcmeJsonConfig {
    #[serde(default)]
    enabled: bool,

    /// Contact email (e.g. "admin@example.com")
    email: Option<String>,

    /// Domains to manage.
    #[serde(default)]
    domains: Vec<String>,

    /// Use Let's Encrypt staging when true.
    #[serde(default)]
    staging: bool,

    /// Override directory URL (custom ACME CA).
    #[serde(default)]
    directory_url: Option<String>,

    /// Optional PEM file path for ACME directory TLS trust roots (e.g. Pebble local CA).
    #[serde(default)]
    directory_ca_pem: Option<String>,

    /// Encrypted cache dir for account/cert blobs.
    #[serde(default = "default_cache_dir")]
    cache_dir: String,

    /// Env var name to read master key from (e.g. "ARC_MASTER_KEY").
    #[serde(default = "default_master_key_env")]
    master_key_env: String,

    /// "tls-alpn-01" (default) or "http-01"
    #[serde(default = "default_challenge")]
    challenge: String,

    /// Dedicated HTTP-01 listener, e.g. "0.0.0.0:80"
    #[serde(default)]
    http01_listen: Option<String>,

    /// Startup jitter seconds
    #[serde(default)]
    startup_jitter_secs: Option<u64>,

    /// Tokio runtime worker threads for ACME
    #[serde(default)]
    runtime_threads: Option<usize>,
}

fn default_cache_dir() -> String {
    "/var/lib/arc/acme".to_string()
}

fn default_master_key_env() -> String {
    "ARC_MASTER_KEY".to_string()
}

fn default_challenge() -> String {
    "tls-alpn-01".to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigSourceFormat {
    Json,
    Toml,
    Yaml,
}

impl ConfigSourceFormat {
    fn from_path(path: &str) -> Result<Self> {
        let ext = Path::new(path)
            .extension()
            .and_then(|v| v.to_str())
            .map(|v| v.to_ascii_lowercase())
            .ok_or_else(|| {
                ArcError::config(format!(
                    "unsupported config file extension for '{}': expected .json/.toml/.yaml/.yml",
                    path
                ))
            })?;

        match ext.as_str() {
            "json" => Ok(Self::Json),
            "toml" => Ok(Self::Toml),
            "yaml" | "yml" => Ok(Self::Yaml),
            _ => Err(ArcError::config(format!(
                "unsupported config file extension '.{ext}' for '{}': expected .json/.toml/.yaml/.yml",
                path
            ))),
        }
    }
}

fn parse_config_acme_wrapper(raw: &[u8], config_path: &str) -> Result<ConfigAcmeWrapper> {
    let format = ConfigSourceFormat::from_path(config_path)?;
    let text = std::str::from_utf8(raw).map_err(|_| {
        ArcError::config(format!("config file is not valid utf-8: {}", config_path))
    })?;

    match format {
        ConfigSourceFormat::Json => serde_json::from_slice(raw)
            .map_err(|e| ArcError::config(format!("parse config for acme (json): {e}"))),
        ConfigSourceFormat::Toml => toml::from_str(text)
            .map_err(|e| ArcError::config(format!("parse config for acme (toml): {e}"))),
        ConfigSourceFormat::Yaml => serde_yaml::from_str(text)
            .map_err(|e| ArcError::config(format!("parse config for acme (yaml): {e}"))),
    }
}

/// Start ACME service from config file path.
/// Safe to call multiple times; subsequent calls are no-ops if ACME already started.
pub fn init_from_config_path(config_path: &str) -> Result<()> {
    if is_enabled() {
        return Ok(());
    }

    let raw = std::fs::read(config_path).map_err(|e| ArcError::io("read config for acme", e))?;

    let parsed = parse_config_acme_wrapper(&raw, config_path)?;

    let Some(acme_cfg) = parsed
        .downstream_tls
        .and_then(|d| d.acme)
        .filter(|a| a.enabled)
    else {
        return Ok(());
    };

    let email = acme_cfg.email.unwrap_or_default().trim().to_string();
    if email.is_empty() {
        return Err(ArcError::config(
            "downstream_tls.acme.enabled=true but email is empty".into(),
        ));
    }
    if acme_cfg.domains.is_empty() {
        return Err(ArcError::config(
            "downstream_tls.acme.enabled=true but domains is empty".into(),
        ));
    }

    let challenge = match acme_cfg.challenge.as_str() {
        "tls-alpn-01" => ChallengeType::TlsAlpn01,
        "http-01" => ChallengeType::Http01,
        other => {
            return Err(ArcError::config(format!(
                "downstream_tls.acme.challenge must be 'tls-alpn-01' or 'http-01', got '{other}'"
            )));
        }
    };

    let http01_listen = match (challenge, acme_cfg.http01_listen.as_deref()) {
        (ChallengeType::Http01, Some(s)) => {
            Some(s.parse::<std::net::SocketAddr>().map_err(|e| {
                ArcError::config(format!("invalid downstream_tls.acme.http01_listen: {e}"))
            })?)
        }
        (ChallengeType::Http01, None) => {
            return Err(ArcError::config(
                "downstream_tls.acme.challenge=http-01 requires downstream_tls.acme.http01_listen"
                    .into(),
            ));
        }
        (_, Some(s)) => Some(s.parse::<std::net::SocketAddr>().map_err(|e| {
            ArcError::config(format!("invalid downstream_tls.acme.http01_listen: {e}"))
        })?),
        (_, None) => None,
    };

    let master_key = std::env::var(&acme_cfg.master_key_env).map_err(|_| {
        ArcError::config(format!(
            "ACME master key env '{}' is not set",
            acme_cfg.master_key_env
        ))
    })?;

    let startup_jitter_max =
        std::time::Duration::from_secs(acme_cfg.startup_jitter_secs.unwrap_or(30 * 60));
    let runtime_threads = acme_cfg.runtime_threads.unwrap_or(2);

    let directory_ca_pem = if let Some(path) = acme_cfg.directory_ca_pem.as_ref() {
        let data = std::fs::read(path)
            .map_err(|e| ArcError::io("read downstream_tls.acme.directory_ca_pem", e))?;
        if data.is_empty() {
            return Err(ArcError::config(format!(
                "downstream_tls.acme.directory_ca_pem is empty: {path}"
            )));
        }
        Some(data)
    } else {
        None
    };

    let runtime_cfg = AcmeRuntimeConfig {
        email,
        domains: acme_cfg.domains,
        cache_dir: std::path::PathBuf::from(acme_cfg.cache_dir),
        master_key,
        staging: acme_cfg.staging,
        directory_url: acme_cfg.directory_url,
        directory_ca_pem,
        challenge,
        http01_listen,
        startup_jitter_max,
        runtime_threads,
    };

    let svc = AcmeService::start_threaded(runtime_cfg)
        .map_err(|e| ArcError::internal(&format!("start ACME: {e}")))?;
    global().store(Some(svc));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_acme_wrapper_supports_json_toml_yaml() {
        let json = br#"{"downstream_tls":{"acme":{"enabled":false}}}"#;
        let toml = br#"[downstream_tls.acme]
enabled = false
"#;
        let yaml = br#"downstream_tls:
  acme:
    enabled: false
"#;

        assert!(parse_config_acme_wrapper(json, "arc.json").is_ok());
        assert!(parse_config_acme_wrapper(toml, "arc.toml").is_ok());
        assert!(parse_config_acme_wrapper(yaml, "arc.yaml").is_ok());
    }
}
