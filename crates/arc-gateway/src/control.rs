use crate::cluster_circuit::{ClusterCircuit, NodeCircuitSnapshot};
use crate::gossip::{GossipBus, GossipMemberView, GossipStatsView};
use arc_common::{ArcError, Result};
use arc_config::{restart_required_changes, ConfigManager, ControlPlaneConfig, ControlRole};
use arc_logging::{LogLevel, LogStr, LogValue};
use arc_xdp_common::{BlockReason, IpKey};
use arc_xdp_userspace::manager::global_xdp_manager;

use bytes::{Bytes, BytesMut};
use http_body_util::{BodyExt, Full};
use hyper::header::{
    HeaderValue, AUTHORIZATION, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, ETAG, IF_NONE_MATCH,
};
use hyper::service::service_fn;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as AutoBuilder;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::collections::BTreeSet;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::time::{self, Instant};

const DEFAULT_MAX_BODY_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_LONGPOLL_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_PEER_CONCURRENCY: usize = 16;

type FixedBody = Full<Bytes>;
type ReqBody = Incoming;
type RespBody = FixedBody;

#[derive(Clone)]
struct ControlState {
    mgr: ConfigManager,
    boot_cp: ControlPlaneConfig,
    cluster_circuit: Arc<ClusterCircuit>,
    gossip: Option<Arc<GossipBus>>,
    http: Client<HttpConnector, FixedBody>,
}

pub fn start_control_plane(mgr: ConfigManager, cluster_circuit: Arc<ClusterCircuit>) -> Result<()> {
    let cfg = mgr.current();
    let cp = cfg.control_plane.clone();
    if !cp.enabled {
        return Ok(());
    }

    let bind: SocketAddr = cp
        .bind
        .parse()
        .map_err(|_| ArcError::config(format!("invalid control_plane.bind: {}", cp.bind)))?;

    let gossip = GossipBus::from_bootstrap(mgr.clone(), &cp, cluster_circuit.clone());

    let state = Arc::new(ControlState {
        mgr: mgr.clone(),
        boot_cp: cp.clone(),
        cluster_circuit,
        gossip,
        http: Client::builder(TokioExecutor::new()).build(HttpConnector::new()),
    });

    let thread_state = state.clone();
    let builder = thread::Builder::new().name("arc-control".to_string());
    builder
        .spawn(move || {
            // Dedicated control-plane runtime: cannot execute on io_uring workers.
            let runtime_threads = cp.runtime_threads.max(1);
            let max_blocking_threads = cp.compile_threads.max(1);

            // Keep async tasks on a dedicated runtime; heavy CPU work is offloaded via spawn_blocking.
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(runtime_threads)
                .max_blocking_threads(max_blocking_threads)
                .enable_io()
                .enable_time()
                .thread_name("arc-control-rt")
                .build();

            let rt = match rt {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("control plane runtime build failed: {e}");
                    return;
                }
            };

            rt.block_on(async move {
                if let Err(e) = run_control(bind, thread_state).await {
                    eprintln!("control plane fatal: {e}");
                }
            });
        })
        .map_err(|e| ArcError::io("spawn control plane thread", e))?;

    Ok(())
}

async fn run_control(bind: SocketAddr, state: Arc<ControlState>) -> Result<()> {
    eprintln!("control plane listening on {bind}");

    // Start gossip bus first (if enabled) so fallbacks can be decided based on actual runtime state.
    if let Some(g) = state.gossip.clone() {
        if let Err(e) = g.start().await {
            eprintln!("gossip start failed: {e}");
        }
    }

    let gossip_running = state
        .gossip
        .as_ref()
        .map(|g| g.is_running())
        .unwrap_or(false);
    let http_push_fallback = state
        .gossip
        .as_ref()
        .map(|g| g.http_push_fallback())
        .unwrap_or(true);

    // follower: long-poll leader for config changes (zero-waste) — kept only as fallback when gossip is not usable.
    if state.boot_cp.role == ControlRole::Follower
        && state.boot_cp.pull_from.is_some()
        && (!gossip_running || http_push_fallback)
    {
        let follower_state = state.clone();
        tokio::spawn(async move {
            run_follower_longpoll(follower_state).await;
        });
    }

    // Legacy circuit sync (HTTP polling) — disabled when gossip is running, since circuit state is propagated via gossip.
    if !state.boot_cp.peers.is_empty() && state.cluster_circuit.enabled() && !gossip_running {
        let sync_state = state.clone();
        tokio::spawn(async move {
            run_cluster_circuit_sync(sync_state).await;
        });
    }

    let listener = TcpListener::bind(bind)
        .await
        .map_err(|e| ArcError::io("control plane bind", e))?;
    loop {
        let (stream, peer_addr) = listener
            .accept()
            .await
            .map_err(|e| ArcError::io("control plane accept", e))?;
        let io = TokioIo::new(stream);
        let st = state.clone();
        tokio::spawn(async move {
            let service = service_fn(move |mut req| {
                req.extensions_mut().insert(peer_addr);
                handle(req, st.clone())
            });
            if let Err(e) = AutoBuilder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(io, service)
                .await
            {
                eprintln!("control plane connection error: {e}");
            }
        });
    }
}

async fn handle(
    req: Request<ReqBody>,
    state: Arc<ControlState>,
) -> std::result::Result<Response<RespBody>, Infallible> {
    if !authorized(&req, state.boot_cp.auth_token.as_deref()) {
        return Ok(json_error(StatusCode::UNAUTHORIZED, "unauthorized"));
    }

    let method = req.method().clone();
    let path = req
        .uri()
        .path()
        .split('?')
        .next()
        .unwrap_or(req.uri().path());

    let resp = match (method, path) {
        (Method::GET, "/v1/status") => handle_status(state).await,
        (Method::GET, "/v1/config") => handle_get_config(req, state).await,
        (Method::GET, "/v1/config/longpoll") => handle_longpoll(req, state).await,
        (Method::GET, "/v1/cluster/circuit/local") => {
            handle_cluster_circuit_local(req, state).await
        }
        (Method::POST, "/v1/config/validate") => handle_validate(req, state).await,
        (Method::POST, "/v1/config") => handle_post_config(req, state).await,
        (Method::POST, "/v1/cluster/config") => handle_cluster_config(req, state).await,
        (Method::GET, "/v1/xdp/status") => handle_xdp_status(req, state).await,
        (Method::GET, "/v1/xdp/blacklist") => handle_xdp_blacklist_list(req, state).await,
        (Method::POST, "/v1/xdp/blacklist") => handle_xdp_blacklist_add(req, state).await,
        (Method::DELETE, "/v1/xdp/blacklist") => handle_xdp_blacklist_remove(req, state).await,
        (Method::GET, "/v1/xdp/whitelist") => handle_xdp_whitelist_list(req, state).await,
        (Method::POST, "/v1/xdp/whitelist") => handle_xdp_whitelist_add(req, state).await,
        (Method::DELETE, "/v1/xdp/whitelist") => handle_xdp_whitelist_remove(req, state).await,

        // Gossip control plane export (spec paths + /v1 compatibility)
        (Method::GET, "/cluster/members") => handle_cluster_members(req, state).await,
        (Method::GET, "/v1/cluster/members") => handle_cluster_members(req, state).await,

        (Method::GET, "/cluster/gossip/stats") => handle_gossip_stats(req, state).await,
        (Method::GET, "/v1/cluster/gossip/stats") => handle_gossip_stats(req, state).await,

        (Method::POST, "/cluster/gossip/join") => handle_gossip_join(req, state).await,
        (Method::POST, "/v1/cluster/gossip/join") => handle_gossip_join(req, state).await,

        (Method::POST, "/cluster/gossip/leave") => handle_gossip_leave(req, state).await,
        (Method::POST, "/v1/cluster/gossip/leave") => handle_gossip_leave(req, state).await,

        _ => Ok(json_error(StatusCode::NOT_FOUND, "not found")),
    };

    Ok(resp.unwrap_or_else(|e| json_error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}"))))
}

async fn handle_status(state: Arc<ControlState>) -> Result<Response<RespBody>> {
    let gen = state.mgr.current_generation();
    let role = role_str(state.boot_cp.role);

    let body = format!(
        "{{\"generation\":{},\"node_id\":\"{}\",\"role\":\"{}\"}}\n",
        gen,
        escape_json(&state.boot_cp.node_id),
        role
    );
    Ok(json_ok(body))
}

async fn handle_get_config(
    req: Request<ReqBody>,
    state: Arc<ControlState>,
) -> Result<Response<RespBody>> {
    if !authorized(&req, state.boot_cp.auth_token.as_deref()) {
        return Ok(json_error(StatusCode::UNAUTHORIZED, "unauthorized"));
    }

    let cfg = state.mgr.current();
    let gen = cfg.generation;

    // If-None-Match (ETag) short-circuit: zero-waste pull for non-longpoll clients.
    if etag_matches(&req, gen) {
        return Ok(Response::builder()
            .status(StatusCode::NOT_MODIFIED)
            .header(CACHE_CONTROL, "no-store")
            .body(empty_body())
            .unwrap_or_else(|_| {
                json_error(StatusCode::INTERNAL_SERVER_ERROR, "response build failed")
            }));
    }

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/json; charset=utf-8")
        .header(CACHE_CONTROL, "no-store")
        .header(ETAG, etag_value(gen))
        .body(body_from_string(cfg.raw_json.as_ref().to_owned()))
        .unwrap_or_else(|_| json_error(StatusCode::INTERNAL_SERVER_ERROR, "response build failed")))
}

async fn handle_longpoll(
    req: Request<ReqBody>,
    state: Arc<ControlState>,
) -> Result<Response<RespBody>> {
    if !authorized(&req, state.boot_cp.auth_token.as_deref()) {
        return Ok(json_error(StatusCode::UNAUTHORIZED, "unauthorized"));
    }

    let query = req.uri().query().unwrap_or("");
    let since = parse_query_u64(query, "since").unwrap_or_else(|| {
        // Also accept If-None-Match as "since".
        parse_etag_u64(req.headers().get(IF_NONE_MATCH)).unwrap_or(state.mgr.current_generation())
    });

    let timeout_ms = parse_query_u64(query, "timeout_ms")
        .unwrap_or(state.boot_cp.longpoll_timeout_ms.max(1).min(120_000))
        .max(1);

    let cur = state.mgr.current_generation();
    if cur != since {
        // changed => return immediately
        let cfg = state.mgr.current();
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "application/json; charset=utf-8")
            .header(CACHE_CONTROL, "no-store")
            .header(ETAG, etag_value(cfg.generation))
            .body(body_from_string(cfg.raw_json.as_ref().to_owned()))
            .unwrap_or_else(|_| {
                json_error(StatusCode::INTERNAL_SERVER_ERROR, "response build failed")
            }));
    }

    // Wait for change (Condvar in arc-config) without blocking async threads.
    let mgr = state.mgr.clone();
    let wait = tokio::task::spawn_blocking(move || {
        mgr.wait_for_generation_change(since, Duration::from_millis(timeout_ms))
    });
    let changed = match wait.await {
        Ok(v) => v,
        Err(_) => false,
    };

    if !changed {
        // timeout or spurious wake: return 304 and let follower re-issue
        return Ok(Response::builder()
            .status(StatusCode::NOT_MODIFIED)
            .header(CACHE_CONTROL, "no-store")
            .body(empty_body())
            .unwrap_or_else(|_| {
                json_error(StatusCode::INTERNAL_SERVER_ERROR, "response build failed")
            }));
    }

    let cfg = state.mgr.current();
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/json; charset=utf-8")
        .header(CACHE_CONTROL, "no-store")
        .header(ETAG, etag_value(cfg.generation))
        .body(body_from_string(cfg.raw_json.as_ref().to_owned()))
        .unwrap_or_else(|_| json_error(StatusCode::INTERNAL_SERVER_ERROR, "response build failed")))
}

async fn handle_cluster_circuit_local(
    req: Request<ReqBody>,
    state: Arc<ControlState>,
) -> Result<Response<RespBody>> {
    if !authorized(&req, state.boot_cp.auth_token.as_deref()) {
        return Ok(json_error(StatusCode::UNAUTHORIZED, "unauthorized"));
    }

    let snap = state.cluster_circuit.local_snapshot();
    let body = serde_json::to_string(&snap)
        .map_err(|_| ArcError::internal("serialize circuit snapshot failed"))?;
    Ok(json_ok(format!("{body}\n")))
}

async fn handle_validate(
    req: Request<ReqBody>,
    state: Arc<ControlState>,
) -> Result<Response<RespBody>> {
    if !authorized(&req, state.boot_cp.auth_token.as_deref()) {
        return Ok(json_error(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let peer = request_peer_addr(&req);

    let max_body = state
        .boot_cp
        .max_body_bytes
        .unwrap_or(DEFAULT_MAX_BODY_BYTES)
        .max(1024);

    let raw = read_body_utf8(req, max_body).await?;
    let started = Instant::now();

    let raw_clone = raw.clone();
    let compiled = tokio::task::spawn_blocking(move || ConfigManager::compile_raw_json(&raw_clone))
        .await
        .map_err(|_| ArcError::internal("compile task panicked"))??;

    let cur = state.mgr.current();
    let restart_required = restart_required_changes(cur.as_ref(), &compiled);
    let diff = summarize_config_diff(cur.raw_json.as_ref(), compiled.raw_json.as_ref(), 32);
    if !restart_required.is_empty() {
        let reject_msg = restart_required.join(", ");
        emit_config_audit(
            "api_validate",
            peer,
            "rejected_restart_required",
            cur.generation,
            compiled.generation,
            &diff,
            Some(reject_msg.as_str()),
        );
        return Ok(json_restart_required(
            compiled.generation,
            &restart_required,
        ));
    }

    let gen = compiled.generation;
    let _elapsed = started.elapsed();
    emit_config_audit(
        "api_validate",
        peer,
        "validated",
        cur.generation,
        gen,
        &diff,
        None,
    );

    let body = format!("{{\"ok\":true,\"generation\":{}}}\n", gen);
    Ok(json_ok(body))
}

async fn handle_post_config(
    req: Request<ReqBody>,
    state: Arc<ControlState>,
) -> Result<Response<RespBody>> {
    if !authorized(&req, state.boot_cp.auth_token.as_deref()) {
        return Ok(json_error(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let peer = request_peer_addr(&req);

    let max_body = state
        .boot_cp
        .max_body_bytes
        .unwrap_or(DEFAULT_MAX_BODY_BYTES)
        .max(1024);

    let raw = read_body_utf8(req, max_body).await?;
    let started = Instant::now();

    // Compile off-thread; then apply by swapping (RCU) immediately.
    let raw_clone = raw.clone();
    let compiled = tokio::task::spawn_blocking(move || ConfigManager::compile_raw_json(&raw_clone))
        .await
        .map_err(|_| ArcError::internal("compile task panicked"))??;

    let cur = state.mgr.current();
    let restart_required = restart_required_changes(cur.as_ref(), &compiled);
    let diff = summarize_config_diff(cur.raw_json.as_ref(), compiled.raw_json.as_ref(), 32);
    if !restart_required.is_empty() {
        let reject_msg = restart_required.join(", ");
        emit_config_audit(
            "api",
            peer,
            "rejected_restart_required",
            cur.generation,
            compiled.generation,
            &diff,
            Some(reject_msg.as_str()),
        );
        eprintln!(
            "control plane config apply rejected (restart required): {}",
            restart_required.join(", ")
        );
        return Ok(json_restart_required(
            compiled.generation,
            &restart_required,
        ));
    }

    let gen = state.mgr.apply_compiled(compiled);
    let _elapsed = started.elapsed();
    emit_config_audit("api", peer, "applied", cur.generation, gen, &diff, None);

    // Gossip: config changes trigger dissemination (best-effort).
    if let Some(g) = state.gossip.as_ref() {
        g.notify_local_config_applied(gen);
    }

    let body = format!("{{\"generation\":{},\"scope\":\"local\"}}\n", gen);
    Ok(json_ok(body))
}

async fn handle_cluster_config(
    req: Request<ReqBody>,
    state: Arc<ControlState>,
) -> Result<Response<RespBody>> {
    let peer = request_peer_addr(&req);
    let cp = state.boot_cp.clone();

    if !authorized(&req, cp.auth_token.as_deref()) {
        return Ok(json_error(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    if cp.role != ControlRole::Leader {
        return Ok(json_error(
            StatusCode::FORBIDDEN,
            "cluster config push is allowed only on leader",
        ));
    }

    let gossip_running = state
        .gossip
        .as_ref()
        .map(|g| g.is_running())
        .unwrap_or(false);
    let http_push_fallback = state
        .gossip
        .as_ref()
        .map(|g| g.http_push_fallback())
        .unwrap_or(true);

    let gossip_only = gossip_running && !http_push_fallback;

    let max_body = cp
        .max_body_bytes
        .unwrap_or(DEFAULT_MAX_BODY_BYTES)
        .max(1024);
    let raw = read_body_utf8(req, max_body).await?;

    // Local compile first (hard gate): never propagate bad config.
    let raw_for_compile = raw.clone();
    let compiled = match tokio::task::spawn_blocking(move || {
        ConfigManager::compile_raw_json(&raw_for_compile)
    })
    .await
    {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            let cur_gen = state.mgr.current_generation();
            let err_text = e.to_string();
            emit_config_audit(
                "cluster_api",
                peer,
                "rejected_invalid_config",
                cur_gen,
                cur_gen,
                &[],
                Some(err_text.as_str()),
            );
            return Ok(json_error(StatusCode::BAD_REQUEST, &format!("{e}")));
        }
        Err(_) => {
            return Ok(json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "compile task panicked",
            ))
        }
    };
    let generation = compiled.generation;

    let cur = state.mgr.current();
    let restart_required = restart_required_changes(cur.as_ref(), &compiled);
    let diff = summarize_config_diff(cur.raw_json.as_ref(), compiled.raw_json.as_ref(), 32);
    if !restart_required.is_empty() {
        let reject_msg = restart_required.join(", ");
        emit_config_audit(
            "cluster_api",
            peer,
            "rejected_restart_required",
            cur.generation,
            generation,
            &diff,
            Some(reject_msg.as_str()),
        );
        eprintln!(
            "cluster config apply rejected (restart required): {}",
            restart_required.join(", ")
        );
        return Ok(json_restart_required(generation, &restart_required));
    }

    if gossip_only {
        // Gossip-first mode: leader applies locally and relies on gossip for dissemination.
        let local_gen = state.mgr.apply_compiled(compiled);
        debug_assert_eq!(local_gen, generation);

        if let Some(g) = state.gossip.as_ref() {
            g.notify_local_config_applied(generation);
        }

        let body = format!(
            "{{\"ok\":true,\"generation\":{},\"scope\":\"gossip\"}}\n",
            generation
        );
        return Ok(json_ok(body));
    }

    // Legacy HTTP push remains as fallback / compatibility.
    let timeout_ms = cp.peer_timeout_ms.max(100);
    let peers = cp.peers.clone();
    let total_nodes = peers.len().saturating_add(1);
    let quorum = if cp.quorum == 0 {
        total_nodes / 2 + 1
    } else {
        cp.quorum.clamp(1, total_nodes)
    };

    let concurrency = if cp.peer_concurrency == 0 {
        DEFAULT_PEER_CONCURRENCY
    } else {
        cp.peer_concurrency
    }
    .clamp(1, 256);
    let token = cp.auth_token.clone();
    let client = state.http.clone();

    // Phase-1: validate peers in parallel
    let mut validated = 1usize; // self
    let mut ready_peers: Vec<String> = Vec::new();

    let mut join = tokio::task::JoinSet::new();
    let mut it = peers.into_iter();

    for _ in 0..concurrency {
        if let Some(peer) = it.next() {
            let body = raw.clone();
            let token = token.clone();
            let client = client.clone();
            join.spawn(async move {
                let ok = post_json_status(
                    &client,
                    &peer,
                    "/v1/config/validate",
                    &body,
                    token.as_deref(),
                    timeout_ms,
                )
                .await;
                (peer, ok)
            });
        }
    }

    while let Some(res) = join.join_next().await {
        if let Ok((peer, ok)) = res {
            if ok {
                validated += 1;
                ready_peers.push(peer);
            }
        }
        if let Some(peer) = it.next() {
            let body = raw.clone();
            let token = token.clone();
            let client = client.clone();
            join.spawn(async move {
                let ok = post_json_status(
                    &client,
                    &peer,
                    "/v1/config/validate",
                    &body,
                    token.as_deref(),
                    timeout_ms,
                )
                .await;
                (peer, ok)
            });
        }
    }

    if validated < quorum {
        let reject_msg = format!("validated={validated}, quorum={quorum}");
        emit_config_audit(
            "cluster_api",
            peer,
            "rejected_quorum",
            cur.generation,
            generation,
            &diff,
            Some(reject_msg.as_str()),
        );
        let body = format!(
            "{{\"ok\":false,\"reason\":\"quorum not reached in validate phase\",\"generation\":{},\"validated\":{},\"quorum\":{}}}\n",
            generation, validated, quorum
        );
        return Ok(Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header(CONTENT_TYPE, "application/json; charset=utf-8")
            .body(body_from_string(body))
            .unwrap_or_else(|_| {
                json_error(StatusCode::INTERNAL_SERVER_ERROR, "response build failed")
            }));
    }

    // Phase-2: local apply (RCU swap) with the already-compiled config (no double compile).
    let local_gen = state.mgr.apply_compiled(compiled);
    debug_assert_eq!(local_gen, generation);
    emit_config_audit(
        "cluster_api",
        peer,
        "applied",
        cur.generation,
        local_gen,
        &diff,
        None,
    );

    // Gossip: even in HTTP fallback mode, we still disseminate via gossip (best-effort).
    if let Some(g) = state.gossip.as_ref() {
        g.notify_local_config_applied(generation);
    }

    // Best-effort peer commit in parallel
    let mut committed = 1usize; // self
    let mut join2 = tokio::task::JoinSet::new();
    let mut it2 = ready_peers.into_iter();

    for _ in 0..concurrency {
        if let Some(peer) = it2.next() {
            let body = raw.clone();
            let token = token.clone();
            let client = client.clone();
            join2.spawn(async move {
                let ok = post_json_status(
                    &client,
                    &peer,
                    "/v1/config",
                    &body,
                    token.as_deref(),
                    timeout_ms,
                )
                .await;
                (peer, ok)
            });
        }
    }

    while let Some(res) = join2.join_next().await {
        if let Ok((_peer, ok)) = res {
            if ok {
                committed += 1;
            }
        }
        if let Some(peer) = it2.next() {
            let body = raw.clone();
            let token = token.clone();
            let client = client.clone();
            join2.spawn(async move {
                let ok = post_json_status(
                    &client,
                    &peer,
                    "/v1/config",
                    &body,
                    token.as_deref(),
                    timeout_ms,
                )
                .await;
                (peer, ok)
            });
        }
    }

    let ok = committed >= quorum;
    let status = if ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    let body = format!(
        "{{\"ok\":{},\"generation\":{},\"validated\":{},\"committed\":{},\"quorum\":{},\"scope\":\"http_push\"}}\n",
        if ok { "true" } else { "false" },
        generation,
        validated,
        committed,
        quorum
    );

    Ok(Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json; charset=utf-8")
        .body(body_from_string(body))
        .unwrap_or_else(|_| json_error(StatusCode::INTERNAL_SERVER_ERROR, "response build failed")))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GossipJoinBody {
    peer: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct XdpBlacklistAddBody {
    ip: String,
    #[serde(default)]
    ttl_ms: Option<u64>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct XdpIpBody {
    ip: String,
}

async fn handle_cluster_members(
    req: Request<ReqBody>,
    state: Arc<ControlState>,
) -> Result<Response<RespBody>> {
    if !authorized(&req, state.boot_cp.auth_token.as_deref()) {
        return Ok(json_error(StatusCode::UNAUTHORIZED, "unauthorized"));
    }

    let Some(g) = state.gossip.as_ref() else {
        return Ok(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "gossip is disabled",
        ));
    };
    if !g.is_running() {
        return Ok(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "gossip is not running",
        ));
    }

    let members: Vec<GossipMemberView> = g.members_view();
    let body = serde_json::to_string(&members)
        .map_err(|_| ArcError::internal("serialize gossip members failed"))?;
    Ok(json_ok(format!("{body}\n")))
}

async fn handle_gossip_stats(
    req: Request<ReqBody>,
    state: Arc<ControlState>,
) -> Result<Response<RespBody>> {
    if !authorized(&req, state.boot_cp.auth_token.as_deref()) {
        return Ok(json_error(StatusCode::UNAUTHORIZED, "unauthorized"));
    }

    let Some(g) = state.gossip.as_ref() else {
        return Ok(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "gossip is disabled",
        ));
    };
    if !g.is_running() {
        return Ok(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "gossip is not running",
        ));
    }

    let stats: GossipStatsView = g.stats_view();
    let body = serde_json::to_string(&stats)
        .map_err(|_| ArcError::internal("serialize gossip stats failed"))?;
    Ok(json_ok(format!("{body}\n")))
}

async fn handle_gossip_join(
    req: Request<ReqBody>,
    state: Arc<ControlState>,
) -> Result<Response<RespBody>> {
    if !authorized(&req, state.boot_cp.auth_token.as_deref()) {
        return Ok(json_error(StatusCode::UNAUTHORIZED, "unauthorized"));
    }

    let Some(g) = state.gossip.as_ref() else {
        return Ok(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "gossip is disabled",
        ));
    };
    if !g.is_running() {
        return Ok(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "gossip is not running",
        ));
    }

    let max_body = state
        .boot_cp
        .max_body_bytes
        .unwrap_or(DEFAULT_MAX_BODY_BYTES)
        .max(1024);

    let raw = read_body_utf8(req, max_body).await?;
    let body: GossipJoinBody =
        serde_json::from_str(&raw).map_err(|_| ArcError::proto("invalid join body"))?;

    g.join_peer(body.peer.as_str()).await?;

    Ok(json_ok("{\"ok\":true}\n".to_string()))
}

async fn handle_gossip_leave(
    req: Request<ReqBody>,
    state: Arc<ControlState>,
) -> Result<Response<RespBody>> {
    if !authorized(&req, state.boot_cp.auth_token.as_deref()) {
        return Ok(json_error(StatusCode::UNAUTHORIZED, "unauthorized"));
    }

    let Some(g) = state.gossip.as_ref() else {
        return Ok(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "gossip is disabled",
        ));
    };
    if !g.is_running() {
        return Ok(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "gossip is not running",
        ));
    }

    g.leave().await?;
    Ok(json_ok("{\"ok\":true}\n".to_string()))
}

async fn handle_xdp_status(
    req: Request<ReqBody>,
    state: Arc<ControlState>,
) -> Result<Response<RespBody>> {
    if !authorized(&req, state.boot_cp.auth_token.as_deref()) {
        return Ok(json_error(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let Some(xdp) = global_xdp_manager() else {
        return Ok(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "xdp manager is not initialized",
        ));
    };

    let status = xdp.status().await;
    let pin_base = xdp.pin_base().await;
    let bl_cap = xdp.blacklist().await.capacity();
    let wl_cap = xdp.whitelist().await.capacity();
    let body = serde_json::json!({
        "mode": status.mode.to_string(),
        "interface": status.iface,
        "pin_base": pin_base,
        "kernel_release": status.kernel_release,
        "program_version": status.program_version,
        "blacklist_capacity": bl_cap,
        "whitelist_capacity": wl_cap
    })
    .to_string();
    Ok(json_ok(format!("{body}\n")))
}

async fn handle_xdp_blacklist_list(
    req: Request<ReqBody>,
    state: Arc<ControlState>,
) -> Result<Response<RespBody>> {
    if !authorized(&req, state.boot_cp.auth_token.as_deref()) {
        return Ok(json_error(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let Some(xdp) = global_xdp_manager() else {
        return Ok(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "xdp manager is not initialized",
        ));
    };
    let max = parse_query_u64(req.uri().query().unwrap_or(""), "max")
        .unwrap_or(1024)
        .clamp(1, 65_536) as usize;

    let bl = xdp.blacklist().await;
    let rows = bl.list(max)?;
    let now = monotonic_ns();
    let mut items = Vec::with_capacity(rows.len());
    for (ip, ent) in rows {
        let remaining_ms = if ent.ttl_ns == 0 {
            None
        } else {
            let exp = ent.blocked_at_ns.saturating_add(ent.ttl_ns);
            Some(exp.saturating_sub(now) / 1_000_000)
        };
        items.push(serde_json::json!({
            "ip": ip_key_to_string(ip),
            "prefix_len": ip.prefix_len,
            "reason": block_reason_to_str(ent.reason),
            "blocked_at_ns": ent.blocked_at_ns,
            "ttl_ms": ent.ttl_ns / 1_000_000,
            "remaining_ms": remaining_ms
        }));
    }
    let body = serde_json::json!({
        "count": items.len(),
        "items": items
    })
    .to_string();
    Ok(json_ok(format!("{body}\n")))
}

async fn handle_xdp_blacklist_add(
    req: Request<ReqBody>,
    state: Arc<ControlState>,
) -> Result<Response<RespBody>> {
    if !authorized(&req, state.boot_cp.auth_token.as_deref()) {
        return Ok(json_error(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let max_body = state
        .boot_cp
        .max_body_bytes
        .unwrap_or(DEFAULT_MAX_BODY_BYTES)
        .max(1024);
    let raw = read_body_utf8(req, max_body).await?;
    let body: XdpBlacklistAddBody =
        serde_json::from_str(&raw).map_err(|_| ArcError::proto("invalid json body"))?;
    let ip = parse_ip_or_cidr_to_ip_key(body.ip.as_str())
        .ok_or_else(|| ArcError::proto("invalid ip or cidr"))?;
    let reason = body
        .reason
        .as_deref()
        .map(parse_block_reason)
        .unwrap_or(BlockReason::Manual);
    let ttl_ms = body.ttl_ms.unwrap_or(600_000).max(1);

    let Some(xdp) = global_xdp_manager() else {
        return Ok(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "xdp manager is not initialized",
        ));
    };
    let bl = xdp.blacklist().await;
    bl.add(ip, Duration::from_millis(ttl_ms), reason)?;
    let resp = serde_json::json!({
        "ok": true,
        "ip": ip_key_to_string(ip),
        "ttl_ms": ttl_ms,
        "reason": block_reason_to_str(reason)
    });
    Ok(json_ok(format!("{}\n", resp)))
}

async fn handle_xdp_blacklist_remove(
    req: Request<ReqBody>,
    state: Arc<ControlState>,
) -> Result<Response<RespBody>> {
    if !authorized(&req, state.boot_cp.auth_token.as_deref()) {
        return Ok(json_error(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let max_body = state
        .boot_cp
        .max_body_bytes
        .unwrap_or(DEFAULT_MAX_BODY_BYTES)
        .max(1024);
    let raw = read_body_utf8(req, max_body).await?;
    let body: XdpIpBody =
        serde_json::from_str(&raw).map_err(|_| ArcError::proto("invalid json body"))?;
    let ip = parse_ip_or_cidr_to_ip_key(body.ip.as_str())
        .ok_or_else(|| ArcError::proto("invalid ip or cidr"))?;

    let Some(xdp) = global_xdp_manager() else {
        return Ok(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "xdp manager is not initialized",
        ));
    };
    let bl = xdp.blacklist().await;
    let changed = bl.remove(ip)?;
    let resp = serde_json::json!({
        "ok": true,
        "removed": changed,
        "ip": ip_key_to_string(ip)
    });
    Ok(json_ok(format!("{}\n", resp)))
}

async fn handle_xdp_whitelist_list(
    req: Request<ReqBody>,
    state: Arc<ControlState>,
) -> Result<Response<RespBody>> {
    if !authorized(&req, state.boot_cp.auth_token.as_deref()) {
        return Ok(json_error(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let Some(xdp) = global_xdp_manager() else {
        return Ok(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "xdp manager is not initialized",
        ));
    };
    let max = parse_query_u64(req.uri().query().unwrap_or(""), "max")
        .unwrap_or(1024)
        .clamp(1, 65_536) as usize;
    let wl = xdp.whitelist().await;
    let rows = wl.list(max)?;
    let mut items = Vec::with_capacity(rows.len());
    for ip in rows {
        items.push(serde_json::json!({
            "ip": ip_key_to_string(ip),
            "prefix_len": ip.prefix_len
        }));
    }
    let body = serde_json::json!({
        "count": items.len(),
        "items": items
    })
    .to_string();
    Ok(json_ok(format!("{body}\n")))
}

async fn handle_xdp_whitelist_add(
    req: Request<ReqBody>,
    state: Arc<ControlState>,
) -> Result<Response<RespBody>> {
    if !authorized(&req, state.boot_cp.auth_token.as_deref()) {
        return Ok(json_error(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let max_body = state
        .boot_cp
        .max_body_bytes
        .unwrap_or(DEFAULT_MAX_BODY_BYTES)
        .max(1024);
    let raw = read_body_utf8(req, max_body).await?;
    let body: XdpIpBody =
        serde_json::from_str(&raw).map_err(|_| ArcError::proto("invalid json body"))?;
    let ip = parse_ip_or_cidr_to_ip_key(body.ip.as_str())
        .ok_or_else(|| ArcError::proto("invalid ip or cidr"))?;

    let Some(xdp) = global_xdp_manager() else {
        return Ok(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "xdp manager is not initialized",
        ));
    };
    let wl = xdp.whitelist().await;
    wl.add(ip)?;
    let resp = serde_json::json!({
        "ok": true,
        "ip": ip_key_to_string(ip)
    });
    Ok(json_ok(format!("{}\n", resp)))
}

async fn handle_xdp_whitelist_remove(
    req: Request<ReqBody>,
    state: Arc<ControlState>,
) -> Result<Response<RespBody>> {
    if !authorized(&req, state.boot_cp.auth_token.as_deref()) {
        return Ok(json_error(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let max_body = state
        .boot_cp
        .max_body_bytes
        .unwrap_or(DEFAULT_MAX_BODY_BYTES)
        .max(1024);
    let raw = read_body_utf8(req, max_body).await?;
    let body: XdpIpBody =
        serde_json::from_str(&raw).map_err(|_| ArcError::proto("invalid json body"))?;
    let ip = parse_ip_or_cidr_to_ip_key(body.ip.as_str())
        .ok_or_else(|| ArcError::proto("invalid ip or cidr"))?;

    let Some(xdp) = global_xdp_manager() else {
        return Ok(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "xdp manager is not initialized",
        ));
    };
    let wl = xdp.whitelist().await;
    let changed = wl.remove(ip)?;
    let resp = serde_json::json!({
        "ok": true,
        "removed": changed,
        "ip": ip_key_to_string(ip)
    });
    Ok(json_ok(format!("{}\n", resp)))
}

async fn run_follower_longpoll(state: Arc<ControlState>) {
    let cp = state.boot_cp.clone();
    let Some(source) = cp.pull_from.clone() else {
        return;
    };

    let mut since = state.mgr.current_generation();
    let mut backoff_ms: u64 = 200;

    loop {
        if !cp.enabled || cp.role != ControlRole::Follower {
            time::sleep(Duration::from_millis(500)).await;
            continue;
        }

        let lp_timeout_ms = cp
            .longpoll_timeout_ms
            .max(1)
            .min(120_000)
            .max(DEFAULT_LONGPOLL_TIMEOUT_MS);

        let req_timeout_ms = (lp_timeout_ms + cp.peer_timeout_ms.max(100)).min(180_000);

        let uri = match build_uri(
            &source,
            "/v1/config/longpoll",
            &format!("since={since}&timeout_ms={lp_timeout_ms}"),
        ) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("follower longpoll bad leader uri '{source}': {e}");
                time::sleep(Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms * 2).min(5_000);
                continue;
            }
        };

        let mut req = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header(CACHE_CONTROL, "no-store");

        if let Some(t) = cp.auth_token.as_deref() {
            req = req.header(AUTHORIZATION, format!("Bearer {t}"));
        }

        let req = match req.body(empty_body()) {
            Ok(v) => v,
            Err(_) => {
                time::sleep(Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms * 2).min(5_000);
                continue;
            }
        };

        let client = state.http.clone();
        let resp =
            match time::timeout(Duration::from_millis(req_timeout_ms), client.request(req)).await {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => {
                    eprintln!("follower longpoll request failed: {e}");
                    time::sleep(Duration::from_millis(backoff_ms)).await;
                    backoff_ms = (backoff_ms * 2).min(5_000);
                    continue;
                }
                Err(_) => {
                    eprintln!("follower longpoll request timeout after {req_timeout_ms}ms");
                    time::sleep(Duration::from_millis(backoff_ms)).await;
                    backoff_ms = (backoff_ms * 2).min(5_000);
                    continue;
                }
            };

        match resp.status() {
            StatusCode::OK => {
                let gen = parse_etag_u64(resp.headers().get(ETAG)).unwrap_or(0);
                let max_body = cp
                    .max_body_bytes
                    .unwrap_or(DEFAULT_MAX_BODY_BYTES)
                    .max(1024);
                let body = match collect_body(resp.into_body(), max_body).await {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("follower longpoll read body failed: {e}");
                        time::sleep(Duration::from_millis(backoff_ms)).await;
                        backoff_ms = (backoff_ms * 2).min(5_000);
                        continue;
                    }
                };

                let raw = match String::from_utf8(body.to_vec()) {
                    Ok(v) => v,
                    Err(_) => {
                        eprintln!("follower longpoll got non-utf8 body");
                        time::sleep(Duration::from_millis(backoff_ms)).await;
                        backoff_ms = (backoff_ms * 2).min(5_000);
                        continue;
                    }
                };

                // Compile+swap off-thread (hard CPU work never on worker threads).
                let raw_for_compile = raw.clone();
                match tokio::task::spawn_blocking(move || {
                    ConfigManager::compile_raw_json(&raw_for_compile)
                })
                .await
                {
                    Ok(Ok(compiled)) => {
                        let new_gen = state.mgr.apply_compiled(compiled);
                        since = if gen != 0 { gen } else { new_gen };
                        backoff_ms = 200;

                        // Gossip: follower apply should also disseminate (best-effort).
                        if let Some(g) = state.gossip.as_ref() {
                            g.notify_local_config_applied(new_gen);
                        }
                    }
                    Ok(Err(e)) => {
                        eprintln!("follower apply config failed: {e}");
                        time::sleep(Duration::from_millis(backoff_ms)).await;
                        backoff_ms = (backoff_ms * 2).min(5_000);
                    }
                    Err(_) => {
                        eprintln!("follower compile task panicked");
                        time::sleep(Duration::from_millis(backoff_ms)).await;
                        backoff_ms = (backoff_ms * 2).min(5_000);
                    }
                }
            }
            StatusCode::NOT_MODIFIED => {
                // No change; immediately re-issue (leader already held us for lp_timeout_ms).
                backoff_ms = 200;
            }
            code => {
                eprintln!("follower longpoll unexpected status: {code}");
                time::sleep(Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms * 2).min(5_000);
            }
        }
    }
}

async fn run_cluster_circuit_sync(state: Arc<ControlState>) {
    let cp = state.boot_cp.clone();
    if cp.peers.is_empty() || !state.cluster_circuit.enabled() {
        return;
    }

    let poll_interval = state.cluster_circuit.gossip_interval();
    let timeout_ms = cp.peer_timeout_ms.max(100);

    loop {
        let peers = cp.peers.clone();
        let token = cp.auth_token.clone();
        let client = state.http.clone();
        let mut join = tokio::task::JoinSet::new();

        for peer in peers {
            let client = client.clone();
            let token = token.clone();
            join.spawn(async move {
                fetch_peer_circuit_snapshot(&client, &peer, token.as_deref(), timeout_ms).await
            });
        }

        while let Some(res) = join.join_next().await {
            if let Ok(Some(snapshot)) = res {
                state.cluster_circuit.ingest_peer_snapshot(snapshot);
            }
        }

        time::sleep(poll_interval).await;
    }
}

async fn fetch_peer_circuit_snapshot(
    client: &Client<HttpConnector, FixedBody>,
    endpoint: &str,
    token: Option<&str>,
    timeout_ms: u64,
) -> Option<NodeCircuitSnapshot> {
    let uri = build_uri(endpoint, "/v1/cluster/circuit/local", "").ok()?;
    let mut req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(CACHE_CONTROL, "no-store");
    if let Some(t) = token {
        req = req.header(AUTHORIZATION, format!("Bearer {t}"));
    }
    let req = req.body(empty_body()).ok()?;

    let resp = match time::timeout(Duration::from_millis(timeout_ms), client.request(req)).await {
        Ok(Ok(v)) => v,
        _ => return None,
    };
    if resp.status() != StatusCode::OK {
        return None;
    }

    let bytes = collect_body(resp.into_body(), DEFAULT_MAX_BODY_BYTES)
        .await
        .ok()?;
    serde_json::from_slice::<NodeCircuitSnapshot>(&bytes).ok()
}

async fn post_json_status(
    client: &Client<HttpConnector, FixedBody>,
    endpoint: &str,
    path: &str,
    body: &str,
    token: Option<&str>,
    timeout_ms: u64,
) -> bool {
    let uri = match build_uri(endpoint, path, "") {
        Ok(v) => v,
        Err(_) => return false,
    };

    let mut req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(CONTENT_TYPE, "application/json")
        .header(CACHE_CONTROL, "no-store")
        .header(CONTENT_LENGTH, body.len().to_string());

    if let Some(t) = token {
        req = req.header(AUTHORIZATION, format!("Bearer {t}"));
    }

    let req = match req.body(body_from_string(body.to_owned())) {
        Ok(v) => v,
        Err(_) => return false,
    };

    let resp = match time::timeout(Duration::from_millis(timeout_ms), client.request(req)).await {
        Ok(Ok(v)) => v,
        _ => return false,
    };
    resp.status() == StatusCode::OK
}

fn authorized<B>(req: &Request<B>, token: Option<&str>) -> bool {
    let Some(token) = token else {
        return request_is_loopback(req);
    };
    let Some(got) = req.headers().get(AUTHORIZATION) else {
        return false;
    };
    let got = got.to_str().unwrap_or("").trim();
    got == format!("Bearer {token}")
}

fn request_is_loopback<B>(req: &Request<B>) -> bool {
    req.extensions()
        .get::<SocketAddr>()
        .map(|a| a.ip().is_loopback())
        .unwrap_or(false)
}

fn role_str(role: ControlRole) -> &'static str {
    match role {
        ControlRole::Standalone => "standalone",
        ControlRole::Leader => "leader",
        ControlRole::Follower => "follower",
    }
}

#[inline]
fn body_from_string(body: String) -> RespBody {
    Full::new(Bytes::from(body))
}

#[inline]
fn body_from_static(body: &'static str) -> RespBody {
    Full::new(Bytes::from_static(body.as_bytes()))
}

#[inline]
fn empty_body() -> RespBody {
    Full::new(Bytes::new())
}

fn internal_response_build_failed() -> Response<RespBody> {
    let mut resp = Response::new(body_from_static("{\"error\":\"response build failed\"}\n"));
    *resp.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
    resp.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

fn json_ok(body: String) -> Response<RespBody> {
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/json; charset=utf-8")
        .header(CACHE_CONTROL, "no-store")
        .body(body_from_string(body))
        .unwrap_or_else(|_| internal_response_build_failed())
}

fn json_error(status: StatusCode, msg: &str) -> Response<RespBody> {
    let body = format!("{{\"error\":\"{}\"}}\n", escape_json(msg));
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json; charset=utf-8")
        .header(CACHE_CONTROL, "no-store")
        .body(body_from_string(body))
        .unwrap_or_else(|_| internal_response_build_failed())
}

fn json_restart_required(generation: u64, changed_params: &[&str]) -> Response<RespBody> {
    let body = serde_json::json!({
        "ok": false,
        "status": "restart_required",
        "generation": generation,
        "changed_params": changed_params,
        "message": "these parameters require restart; config was not applied"
    })
    .to_string()
        + "\n";
    Response::builder()
        .status(StatusCode::CONFLICT)
        .header(CONTENT_TYPE, "application/json; charset=utf-8")
        .header(CACHE_CONTROL, "no-store")
        .body(body_from_string(body))
        .unwrap_or_else(|_| internal_response_build_failed())
}

fn request_peer_addr(req: &Request<ReqBody>) -> Option<SocketAddr> {
    req.extensions().get::<SocketAddr>().copied()
}

fn emit_config_audit(
    source: &str,
    peer: Option<SocketAddr>,
    status: &str,
    old_generation: u64,
    new_generation: u64,
    diff: &[String],
    error: Option<&str>,
) {
    let level = match status {
        "applied" | "validated" => LogLevel::Info,
        "rejected_restart_required" | "rejected_quorum" | "rejected_invalid_config" => {
            LogLevel::Warn
        }
        _ => LogLevel::Error,
    };

    let mut fields = vec![
        (LogStr::new("event"), LogValue::from("config_changed")),
        (LogStr::new("source"), LogValue::from(source)),
        (LogStr::new("status"), LogValue::from(status)),
        (LogStr::new("old_generation"), LogValue::from(old_generation)),
        (LogStr::new("new_generation"), LogValue::from(new_generation)),
        (
            LogStr::new("diff"),
            LogValue::from(
                serde_json::to_string(diff)
                    .unwrap_or_else(|_| "[\"<diff_serialize_failed>\"]".to_string()),
            ),
        ),
    ];
    if let Some(addr) = peer {
        fields.push((LogStr::new("peer"), LogValue::from(addr.to_string())));
    }
    if let Some(msg) = error {
        fields.push((LogStr::new("error"), LogValue::from(msg.to_string())));
    }
    arc_logging::system_log_fields(level, "config_audit", fields);
}

fn summarize_config_diff(old_raw: &str, new_raw: &str, limit: usize) -> Vec<String> {
    let limit = limit.max(1);
    let old_json = match serde_json::from_str::<JsonValue>(old_raw) {
        Ok(v) => v,
        Err(_) => return vec!["<old_config_parse_failed>".to_string()],
    };
    let new_json = match serde_json::from_str::<JsonValue>(new_raw) {
        Ok(v) => v,
        Err(_) => return vec!["<new_config_parse_failed>".to_string()],
    };
    if old_json == new_json {
        return Vec::new();
    }

    let mut out = Vec::with_capacity(limit.min(16));
    let mut truncated = false;
    diff_json_value("", &old_json, &new_json, limit, &mut out, &mut truncated);
    if out.is_empty() {
        out.push("<config_changed_without_diff_entries>".to_string());
    }
    if truncated && out.len() < limit {
        out.push("<diff_truncated>".to_string());
    }
    out
}

fn diff_json_value(
    path: &str,
    old_v: &JsonValue,
    new_v: &JsonValue,
    limit: usize,
    out: &mut Vec<String>,
    truncated: &mut bool,
) {
    if *truncated || out.len() >= limit || old_v == new_v {
        if out.len() >= limit {
            *truncated = true;
        }
        return;
    }

    match (old_v, new_v) {
        (JsonValue::Object(old_o), JsonValue::Object(new_o)) => {
            let mut keys = BTreeSet::new();
            keys.extend(old_o.keys().cloned());
            keys.extend(new_o.keys().cloned());
            for k in keys {
                let old_child = old_o.get(&k).unwrap_or(&JsonValue::Null);
                let new_child = new_o.get(&k).unwrap_or(&JsonValue::Null);
                let child_path = if path.is_empty() {
                    k
                } else {
                    format!("{path}.{k}")
                };
                diff_json_value(&child_path, old_child, new_child, limit, out, truncated);
                if *truncated {
                    return;
                }
            }
        }
        (JsonValue::Array(old_a), JsonValue::Array(new_a)) => {
            if old_a.len() != new_a.len() {
                push_diff_line(
                    path,
                    &format!("<len:{}>", old_a.len()),
                    &format!("<len:{}>", new_a.len()),
                    limit,
                    out,
                    truncated,
                );
                if *truncated {
                    return;
                }
            }
            let n = old_a.len().min(new_a.len());
            for idx in 0..n {
                let child_path = if path.is_empty() {
                    format!("[{idx}]")
                } else {
                    format!("{path}[{idx}]")
                };
                diff_json_value(
                    &child_path,
                    &old_a[idx],
                    &new_a[idx],
                    limit,
                    out,
                    truncated,
                );
                if *truncated {
                    return;
                }
            }
        }
        _ => {
            push_diff_line(
                path,
                &format_json_value(old_v),
                &format_json_value(new_v),
                limit,
                out,
                truncated,
            );
        }
    }
}

fn push_diff_line(
    path: &str,
    old_repr: &str,
    new_repr: &str,
    limit: usize,
    out: &mut Vec<String>,
    truncated: &mut bool,
) {
    if out.len() >= limit {
        *truncated = true;
        return;
    }
    let p = if path.is_empty() { "<root>" } else { path };
    out.push(format!("{p}: {old_repr} -> {new_repr}"));
}

fn format_json_value(v: &JsonValue) -> String {
    let mut s = match v {
        JsonValue::Null => "null".to_string(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Number(n) => n.to_string(),
        JsonValue::String(t) => format!("\"{}\"", t),
        _ => serde_json::to_string(v).unwrap_or_else(|_| "<json_serialize_failed>".to_string()),
    };
    if s.len() > 120 {
        s.truncate(117);
        s.push_str("...");
    }
    s
}

async fn read_body_utf8(req: Request<ReqBody>, max: usize) -> Result<String> {
    if let Some(v) = req.headers().get(CONTENT_LENGTH) {
        if let Ok(s) = v.to_str() {
            if let Ok(n) = s.parse::<usize>() {
                if n > max {
                    return Err(ArcError::proto("request body too large"));
                }
            }
        }
    }
    let bytes = collect_body(req.into_body(), max).await?;
    String::from_utf8(bytes.to_vec())
        .map_err(|_| ArcError::proto("request body must be utf-8 json"))
}

async fn collect_body(mut body: ReqBody, max: usize) -> Result<bytes::Bytes> {
    let mut buf = BytesMut::with_capacity(4096);
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|e| {
            ArcError::io(
                "control read request body",
                std::io::Error::other(e.to_string()),
            )
        })?;
        let Ok(chunk) = frame.into_data() else {
            continue;
        };
        if buf.len() + chunk.len() > max {
            return Err(ArcError::proto("request body too large"));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf.freeze())
}

fn etag_value(gen: u64) -> String {
    // quoted ETag is standard; generation is u64 stable hash.
    format!("\"{gen}\"")
}

fn parse_etag_u64(v: Option<&hyper::header::HeaderValue>) -> Option<u64> {
    let v = v?.to_str().ok()?.trim();
    let v = v.trim_matches('"');
    v.parse::<u64>().ok()
}

fn etag_matches<B>(req: &Request<B>, gen: u64) -> bool {
    let Some(v) = req.headers().get(IF_NONE_MATCH) else {
        return false;
    };
    parse_etag_u64(Some(v)) == Some(gen)
}

fn parse_query_u64(query: &str, key: &str) -> Option<u64> {
    let mut it = query.split('&');
    while let Some(kv) = it.next() {
        let mut p = kv.splitn(2, '=');
        let k = p.next().unwrap_or("");
        let v = p.next().unwrap_or("");
        if k == key {
            return v.parse::<u64>().ok();
        }
    }
    None
}

fn parse_block_reason(v: &str) -> BlockReason {
    match v.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "syn_flood" => BlockReason::SynFlood,
        "ack_flood" => BlockReason::AckFlood,
        "rst_invalid" | "rst_flood" => BlockReason::RstInvalid,
        "udp_rate_limit" => BlockReason::UdpRateLimit,
        _ => BlockReason::Manual,
    }
}

fn block_reason_to_str(v: BlockReason) -> &'static str {
    match v {
        BlockReason::Unknown => "unknown",
        BlockReason::SynFlood => "syn_flood",
        BlockReason::AckFlood => "ack_flood",
        BlockReason::RstInvalid => "rst_invalid",
        BlockReason::UdpRateLimit => "udp_rate_limit",
        BlockReason::Manual => "manual",
    }
}

fn parse_ip_or_cidr_to_ip_key(s: &str) -> Option<IpKey> {
    let raw = s.trim();
    if raw.is_empty() {
        return None;
    }
    let (ip_part, prefix_part) = match raw.split_once('/') {
        Some((ip, p)) => (ip.trim(), Some(p.trim())),
        None => (raw, None),
    };
    let ip = ip_part.parse::<IpAddr>().ok()?;
    match (ip, prefix_part) {
        (IpAddr::V4(v4), Some(p)) => {
            let plen = p.parse::<u8>().ok()?;
            if plen > 32 {
                return None;
            }
            Some(IpKey::from_ipv4_cidr(v4.octets(), plen))
        }
        (IpAddr::V6(v6), Some(p)) => {
            let plen = p.parse::<u8>().ok()?;
            if plen > 128 {
                return None;
            }
            Some(IpKey::from_ipv6_cidr(v6.octets(), plen))
        }
        (IpAddr::V4(v4), None) => Some(IpKey::from_ipv4_exact(v4.octets())),
        (IpAddr::V6(v6), None) => Some(IpKey::from_ipv6_exact(v6.octets())),
    }
}

fn ip_key_to_string(k: IpKey) -> String {
    if k.is_ipv4_mapped() {
        let ip = std::net::Ipv4Addr::new(k.addr[12], k.addr[13], k.addr[14], k.addr[15]);
        if k.prefix_len >= 128 {
            ip.to_string()
        } else {
            let plen = k.prefix_len.saturating_sub(96);
            format!("{ip}/{plen}")
        }
    } else {
        let ip = std::net::Ipv6Addr::from(k.addr);
        if k.prefix_len >= 128 {
            ip.to_string()
        } else {
            format!("{ip}/{}", k.prefix_len)
        }
    }
}

fn monotonic_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    if rc != 0 {
        return 0;
    }
    (ts.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(ts.tv_nsec as u64)
}

fn build_uri(endpoint: &str, path: &str, query: &str) -> Result<hyper::Uri> {
    let base = normalize_endpoint(endpoint);
    let full = if query.is_empty() {
        format!("{base}{path}")
    } else {
        format!("{base}{path}?{query}")
    };
    full.parse::<hyper::Uri>()
        .map_err(|_| ArcError::config(format!("invalid endpoint uri: {full}")))
}

fn normalize_endpoint(input: &str) -> String {
    let mut s = input.trim().to_string();
    if !s.contains("://") {
        s = format!("http://{s}");
    }
    while s.ends_with('/') {
        s.pop();
    }
    // Drop any path component (keep scheme://host:port)
    if let Ok(uri) = s.parse::<hyper::Uri>() {
        if let Some(auth) = uri.authority() {
            let scheme = uri.scheme_str().unwrap_or("http");
            return format!("{scheme}://{auth}");
        }
    }
    // Fallback: strip after first '/' (after scheme if present)
    if let Some(pos) = s.find("://") {
        if let Some(slash) = s[pos + 3..].find('/') {
            s.truncate(pos + 3 + slash);
        }
    } else if let Some(slash) = s.find('/') {
        s.truncate(slash);
    }
    s
}

fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let code = c as u32;
                out.push_str("\\u");
                out.push_str(&format!("{code:04x}"));
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use hyper::header::HeaderValue;
    use serde_json::Value;

    fn req_with_peer(peer: SocketAddr) -> Request<()> {
        let mut req = Request::builder().uri("/").body(()).unwrap();
        req.extensions_mut().insert(peer);
        req
    }

    #[test]
    fn auth_without_token_allows_only_loopback() {
        let local = req_with_peer(SocketAddr::from(([127, 0, 0, 1], 12000)));
        assert!(authorized(&local, None));

        let remote = req_with_peer(SocketAddr::from(([10, 0, 0, 7], 12000)));
        assert!(!authorized(&remote, None));
    }

    #[test]
    fn auth_with_token_requires_header_even_on_loopback() {
        let local = req_with_peer(SocketAddr::from(([127, 0, 0, 1], 12000)));
        assert!(!authorized(&local, Some("tkn")));
    }

    #[test]
    fn auth_with_token_accepts_trimmed_bearer_value() {
        let mut req = req_with_peer(SocketAddr::from(([10, 0, 0, 8], 13000)));
        req.headers_mut()
            .insert(AUTHORIZATION, HeaderValue::from_static("   Bearer tkn   "));
        assert!(authorized(&req, Some("tkn")));
    }

    #[test]
    fn auth_with_token_rejects_wrong_or_non_bearer_value() {
        let mut wrong = req_with_peer(SocketAddr::from(([10, 0, 0, 8], 13000)));
        wrong
            .headers_mut()
            .insert(AUTHORIZATION, HeaderValue::from_static("Bearer wrong"));
        assert!(!authorized(&wrong, Some("tkn")));

        let mut non_bearer = req_with_peer(SocketAddr::from(([10, 0, 0, 8], 13000)));
        non_bearer
            .headers_mut()
            .insert(AUTHORIZATION, HeaderValue::from_static("Basic abc"));
        assert!(!authorized(&non_bearer, Some("tkn")));
    }

    #[test]
    fn summarize_config_diff_reports_nested_path() {
        let old_raw = r#"{"routes":[{"limits":{"max_request_body_bytes":10485760}}]}"#;
        let new_raw = r#"{"routes":[{"limits":{"max_request_body_bytes":20971520}}]}"#;
        let diff = summarize_config_diff(old_raw, new_raw, 8);
        assert!(diff
            .iter()
            .any(|line| line.contains("routes[0].limits.max_request_body_bytes")));
    }

    #[test]
    fn summarize_config_diff_truncates_by_limit() {
        let old_raw = r#"{"a":1,"b":1,"c":1}"#;
        let new_raw = r#"{"a":2,"b":2,"c":2}"#;
        let diff = summarize_config_diff(old_raw, new_raw, 2);
        assert!(!diff.is_empty());
        assert!(diff.len() <= 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn restart_required_response_has_expected_shape() {
        let resp = json_restart_required(42, &["workers", "listen"]);
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        assert_eq!(
            resp.headers().get(CONTENT_TYPE).unwrap(),
            "application/json; charset=utf-8"
        );

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["status"], "restart_required");
        assert_eq!(v["ok"], false);
        assert_eq!(v["generation"], 42);
        assert_eq!(
            v["changed_params"],
            serde_json::json!(["workers", "listen"])
        );
    }
}
