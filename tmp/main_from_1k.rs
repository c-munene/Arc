#[cfg(not(target_os = "linux"))]
compile_error!("arc-gateway currently targets Linux only (io_uring + thread-per-core).");

mod downstream_tls;
mod h2;
mod cluster_circuit;
mod control;
mod tls; // NEW: unified TLS in worker
mod worker;

use arc_common::{ArcError, Result};
use arc_config::{ConfigManager, GlobalRateLimitBackend};
use arc_global_rate_limit::{
    redis_backend::RedisLuaBackend, GlobalRateLimiter, GlobalRateLimiterConfig, InMemoryBackend,
    RateLimiterBackend, WorkerLimiter as GlobalWorkerLimiter,
};
use arc_net::cpu;
use arc_observability::{start_admin_server, MetricsRegistry};
use cluster_circuit::{ClusterCircuit, ClusterCircuitConfig};

use std::path::PathBuf;
use std::process;
use std::sync::Arc;
use std::time::Duration;

// NEW: ACME runtime
use std::time::SystemTime;
use arc_swap::ArcSwap;

fn main() {
    if let Err(e) = real_main() {
        eprintln!("{e}");
        process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let args = Args::from_env()?;
    let cfg = ConfigManager::load_from_path(&args.config_path)?;

    let detected = cpu::cpu_count().unwrap_or(1).max(1);
    let workers = if cfg.workers == 0 {
        detected
    } else {
        cfg.workers
    };
    if workers == 0 {
        return Err(ArcError::config("workers must be > 0".to_string()));
    }

    let mgr = ConfigManager::new(cfg);
    // hot reload interval: 500ms (non-hot path)
    mgr.spawn_hot_reload(args.config_path.clone(), 500);

    let swap = mgr.swap();

    // ✅ start ACME runtime if configured (control-plane thread)
    let _acme_rt = start_acme_runtime_if_configured(swap.clone())?;

    // 分布式控制面（可选）：用于跨节点配置下发与 follower 拉取同步。
    let bootstrap_cfg = swap.load();
    let cp_cfg = bootstrap_cfg.control_plane.clone();
    let cc_cfg = bootstrap_cfg.cluster_circuit.clone();
    let mut circuit_cfg = ClusterCircuitConfig::default();
    circuit_cfg.enabled = true;
    circuit_cfg.fail_streak_threshold = cc_cfg.failure_threshold.max(1);
    circuit_cfg.open_ms = cc_cfg.circuit_open_ms.max(1);
    circuit_cfg.half_open_probe_interval_ms = cc_cfg.half_open_probe_interval_ms.max(1);
    circuit_cfg.peer_sync_interval_ms = cp_cfg.pull_interval_ms.max(200);
    circuit_cfg.peer_ttl_ms = cp_cfg.peer_timeout_ms.saturating_mul(4).max(1_000);
    circuit_cfg.peer_open_quorum = cc_cfg.quorum.max(1);
    let cluster_circuit = Arc::new(ClusterCircuit::new(cp_cfg.node_id.clone(), circuit_cfg));

    control::start_control_plane(mgr.clone(), cluster_circuit.clone())?;

    // metrics registry + admin server
    let reg = MetricsRegistry::new(workers);
    start_admin_server(swap.load().admin_listen, reg.clone())?;

    // global rate limiter runtime (process-wide).
    let grl_cfg = swap.load().global_rate_limit.clone();
    let mut grl_runtime_cfg = GlobalRateLimiterConfig::default();
    let backend: Arc<dyn RateLimiterBackend> = match grl_cfg.backend {
        GlobalRateLimitBackend::InMemory => Arc::new(InMemoryBackend::new(workers.max(1) * 4)),
        GlobalRateLimitBackend::Redis => {
            let redis = grl_cfg.redis.as_ref().ok_or_else(|| {
                ArcError::config(
                    "global_rate_limit.backend=redis but global_rate_limit.redis is missing"
                        .to_string(),
                )
            })?;

            grl_runtime_cfg.redis_budget = Duration::from_millis(redis.budget_ms.max(1));
            grl_runtime_cfg.circuit_open = Duration::from_millis(redis.circuit_open_ms.max(1));
            grl_runtime_cfg.prefetch = redis.prefetch.max(1);
            grl_runtime_cfg.low_watermark = redis.low_watermark.min(
                grl_runtime_cfg
                    .prefetch
                    .saturating_sub(1)
                    .max(1),
            );
            grl_runtime_cfg.refill_backoff =
                Duration::from_millis(redis.refill_backoff_ms.max(1));

            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .enable_time()
                .build()
                .map_err(|e| {
                    ArcError::config(format!(
                        "build global_rate_limit redis runtime failed: {e}"
                    ))
                })?;
            let redis_backend = rt
                .block_on(RedisLuaBackend::connect(redis.url.as_str()))
                .map_err(|e| {
                    ArcError::config(format!(
                        "connect global_rate_limit redis failed ({}): {e}",
                        redis.url
                    ))
                })?;
            Arc::new(redis_backend)
        }
    };

    let (global_rl, handles) = GlobalRateLimiter::spawn(backend, workers, grl_runtime_cfg);
    let mut global_worker_limiters: Vec<Option<GlobalWorkerLimiter>> =
        handles.into_iter().map(Some).collect();
    let _global_rl = global_rl;

    // ✅ always spawn io_uring workers (unified dataplane)
    let mut handles = Vec::with_capacity(workers);
    for wid in 0..workers {
        let swap = swap.clone();
        let metrics = reg.worker(wid);
        let global_limiter = global_worker_limiters
            .get_mut(wid)
            .and_then(|slot| slot.take());
        let cluster_circuit = cluster_circuit.clone();

        let builder = std::thread::Builder::new().name(format!("arc-gw-{wid}"));
        let h = builder
            .spawn(move || {
                if let Err(e) = worker::Worker::run(
                    wid,
                    workers,
                    swap,
                    metrics,
                    global_limiter,
                    cluster_circuit,
                ) {
                    eprintln!("worker[{wid}] fatal: {e}");
                    process::exit(1);
                }
            })
            .map_err(|e| ArcError::io("spawn worker", e))?;
        handles.push(h);
    }

    for h in handles {
        if let Err(_) = h.join() {
            return Err(ArcError::internal("worker thread panicked"));
        }
    }

    Ok(())
}

// ---- ACME runtime bootstrap (NEW) ----

fn start_acme_runtime_if_configured(
    swap: Arc<ArcSwap<arc_config::SharedConfig>>,
) -> Result<Option<()>> {
    let cfg = swap.load();
    let Some(dtls) = cfg.downstream_tls.as_ref() else {
        return Ok(None);
    };
    let Some(acme) = dtls.acme.as_ref() else {
        return Ok(None);
    };
    if !acme.enabled {
        return Ok(None);
    }

    // init global challenge store
    let store = Arc::new(arc_acme::ChallengeStore::new());
    arc_acme::set_global_challenge_store(store.clone());

    // optional HTTP-01 responder
    if let Some(http01) = acme.http01.as_ref() {
        arc_acme::spawn_http01_server(http01.listen)?;
    }

    // members for rendezvous hashing
    let node_id = cfg.control_plane.node_id.clone();
    let members: Vec<String> = if acme.members.is_empty() {
        vec![node_id.clone()]
    } else {
        acme.members.iter().map(|s| s.to_string()).collect()
    };

    let passphrase = match &acme.account_key.passphrase {
        arc_config::CompiledAcmePassphraseSource::Env { name } => {
            arc_acme::PassphraseSource::Env { name: name.to_string() }
        }
        arc_config::CompiledAcmePassphraseSource::File { path } => {
            arc_acme::PassphraseSource::File { path: path.clone() }
        }
    };

    let alg = match acme.account_key.algorithm {
        arc_config::AcmeAccountKeyAlgorithm::Ed25519 => arc_acme::AccountKeyAlgorithm::Ed25519,
        arc_config::AcmeAccountKeyAlgorithm::Rsa2048 => arc_acme::AccountKeyAlgorithm::Rsa2048,
    };

    let mut priority: Vec<arc_acme::ChallengeType> = Vec::new();
    for c in acme.challenge_priority.iter() {
        match c {
            arc_config::AcmeChallengeType::Http01 => priority.push(arc_acme::ChallengeType::Http01),
            arc_config::AcmeChallengeType::TlsAlpn01 => priority.push(arc_acme::ChallengeType::TlsAlpn01),
            arc_config::AcmeChallengeType::Dns01 => priority.push(arc_acme::ChallengeType::Dns01),
        }
    }

    let dns_hook = acme.dns_hook.as_ref().map(|h| arc_acme::DnsHook {
        command: h.command.to_string(),
        args: h.args.iter().map(|s| s.to_string()).collect(),
        env: h.env.iter().map(|(k,v)| (k.to_string(), v.to_string())).collect(),
        propagation_timeout: Duration::from_secs(h.propagation_timeout_secs.max(1)),
        poll_interval: Duration::from_secs(h.poll_interval_secs.max(1)),
    });

    let http01_cfg = acme.http01.as_ref().map(|h| arc_acme::Http01Config { listen: h.listen });

    let provider_cfg = arc_acme::RealAcmeConfig {
        directory_url: acme.directory_url.to_string(),
        directory_ca_pem: acme.directory_ca_pem.as_ref().map(|b| b.as_ref().to_vec()),
        email: acme.email.as_ref().map(|s| s.to_string()),
        challenge_priority: priority,
        dns_hook,
        http01: http01_cfg,
        account_key: arc_acme::AccountKeyConfig {
            algorithm: alg,
            encrypted_key_path: acme.account_key.encrypted_key_path.clone(),
            passphrase,
        },
        poll_timeout: Duration::from_secs(300),
    };

    let provider = Arc::new(arc_acme::RealAcmeProvider::new(provider_cfg, store.clone())?);
    let lease: Arc<dyn arc_acme::LeaseStore> = Arc::new(arc_acme::InMemoryLeaseStore::new());

    let mut manager = arc_acme::AcmeManager::new(
        node_id.clone(),
        Arc::new(members),
        lease,
        provider,
    );

    // register managed certs
    for c in acme.certificates.iter() {
        let not_after = arc_acme::read_cert_not_after_unix(c.cert_path.as_path()).unwrap_or(0);
        manager.register_domain(
            c.domain.to_string(),
            c.cert_path.clone(),
            c.key_path.clone(),
            not_after,
        );
    }

    let tick_every = Duration::from_secs(acme.poll_interval_secs.max(1));
    std::thread::Builder::new()
        .name("arc-acme".to_string())
        .spawn(move || {
            let mut last_gen: u64 = 0;
            loop {
                let cur = swap.load();
                if cur.generation != last_gen {
                    // best-effort: refresh members + register new certs if config changed
                    if let Some(dtls) = cur.downstream_tls.as_ref() {
                        if let Some(acme) = dtls.acme.as_ref() {
                            let members: Vec<String> = if acme.members.is_empty() {
                                vec![cur.control_plane.node_id.clone()]
                            } else {
                                acme.members.iter().map(|s| s.to_string()).collect()
                            };
                            manager.set_members(Arc::new(members));
                            for c in acme.certificates.iter() {
                                let not_after = arc_acme::read_cert_not_after_unix(c.cert_path.as_path()).unwrap_or(0);
                                manager.register_domain(
                                    c.domain.to_string(),
                                    c.cert_path.clone(),
                                    c.key_path.clone(),
                                    not_after,
                                );
                            }
                        }
                    }
                    last_gen = cur.generation;
                }

                let now = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_else(|_| Duration::from_secs(0))
                    .as_secs();

                manager.tick(now);
                std::thread::sleep(tick_every);
            }
        })
        .map_err(|e| ArcError::io("spawn arc-acme thread", e))?;

    Ok(Some(()))
}

struct Args {
    config_path: PathBuf,
}

impl Args {
    fn from_env() -> Result<Self> {
        let mut it = std::env::args();
        let _program = it.next();

        let mut config_path: Option<PathBuf> = None;

        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--config" => {
                    let v = it
                        .next()
                        .ok_or_else(|| ArcError::config("--config requires <path>".to_string()))?;
                    config_path = Some(PathBuf::from(v));
                }
                "--help" | "-h" => {
                    print_help();
                    process::exit(0);
                }
                _ => {
                    return Err(ArcError::config(format!(
                        "unknown argument: {arg} (use --help)"
                    )));
                }
            }
        }

        let config_path = config_path.unwrap_or_else(|| PathBuf::from("arc.example.json"));
        Ok(Self { config_path })
    }
}

fn print_help() {
    println!(
        "\
arc-gateway — io_uring thread-per-core HTTP/1.1 reverse proxy (unified TLS dataplane)

USAGE:
  arc-gateway --config <path>

OPTIONS:
  --config <path>    JSON config file (default: arc.example.json)
  -h, --help         Print this help
"
    );
}