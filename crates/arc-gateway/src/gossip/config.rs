use serde_json::Value;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

/// Gossip runtime configuration parsed from `ConfigManager.current().raw_json`.
///
/// This is intentionally **decoupled** from `arc-config` schema to avoid requiring schema changes.
/// Unknown fields remain harmless.
#[derive(Debug, Clone)]
pub struct GossipRuntimeConfig {
    pub gossip: GossipConfig,
    pub fallback: GossipFallbackConfig,
}

impl Default for GossipRuntimeConfig {
    fn default() -> Self {
        Self {
            gossip: GossipConfig::default(),
            fallback: GossipFallbackConfig::default(),
        }
    }
}

/// `cluster.fallback` section.
#[derive(Debug, Clone)]
pub struct GossipFallbackConfig {
    /// When `true`, Arc keeps using legacy HTTP push/pull paths as fallback
    /// if gossip is not running/usable.
    ///
    /// Default: `true`.
    pub http_push: bool,
}

impl Default for GossipFallbackConfig {
    fn default() -> Self {
        Self { http_push: true }
    }
}

/// `cluster.gossip` section.
#[derive(Debug, Clone)]
pub struct GossipConfig {
    /// Enable SWIM + gossip state bus.
    ///
    /// Default: `false` (safer rollout).
    pub enabled: bool,

    /// UDP/TCP bind address for gossip runtime (same port, different protocol).
    ///
    /// Unit: socket addr.
    /// Default: `0.0.0.0:7946`.
    pub bind: SocketAddr,

    /// Advertised address in gossip messages (NAT-friendly).
    ///
    /// Unit: socket addr.
    /// Default: derived from `bind`:
    /// - if `bind.ip()` is unspecified => `127.0.0.1:<port>`
    /// - else => `bind`
    pub advertise: SocketAddr,

    /// Seed peers used to join the cluster on startup.
    ///
    /// Default: empty.
    pub peers: Vec<SocketAddr>,

    /// Gossip / SWIM tick interval.
    ///
    /// Unit: duration.
    /// Default: 200ms.
    /// Range: [20ms .. 10s] (clamped).
    pub interval: Duration,

    /// Fanout per gossip round.
    ///
    /// Default: 3.
    /// Range: [1 .. 64] (clamped).
    pub fanout: usize,

    /// How long a suspect can remain suspect before being marked dead.
    ///
    /// Unit: duration.
    /// Default: 5s.
    /// Range: [200ms .. 5min] (clamped).
    pub suspicion_timeout: Duration,

    /// How long a dead member is kept before removal.
    ///
    /// Unit: duration.
    /// Default: 30s.
    /// Range: [1s .. 30min] (clamped).
    pub dead_timeout: Duration,

    /// Max UDP datagram size (bytes). MTU-friendly.
    ///
    /// Default: 1400.
    /// Range: [256 .. 65507] (clamped).
    pub max_message_size: usize,

    /// Retransmit multiplier.
    ///
    /// Retransmit rounds ≈ `multiplier × ceil(log2(n))`.
    ///
    /// Default: 2.
    /// Range: [1 .. 16] (clamped).
    pub retransmit_multiplier: usize,
}

impl Default for GossipConfig {
    fn default() -> Self {
        let bind: SocketAddr = "0.0.0.0:7946"
            .parse()
            .unwrap_or_else(|_| SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 7946));
        let advertise = derive_advertise(bind);
        Self {
            enabled: false,
            bind,
            advertise,
            peers: Vec::new(),
            interval: Duration::from_millis(200),
            fanout: 3,
            suspicion_timeout: Duration::from_secs(5),
            dead_timeout: Duration::from_secs(30),
            max_message_size: 1400,
            retransmit_multiplier: 2,
        }
    }
}

impl GossipRuntimeConfig {
    /// Parse runtime config from Arc's canonical `raw_json`.
    ///
    /// On any parse error we fall back to defaults.
    pub fn parse_from_raw_json(raw_json: &str) -> Self {
        let mut out = GossipRuntimeConfig::default();

        let root: Value = match serde_json::from_str(raw_json) {
            Ok(v) => v,
            Err(_) => return out,
        };

        // cluster.gossip.*
        if let Some(v) = get_path(&root, &["cluster", "gossip", "enabled"]) {
            if let Some(b) = v.as_bool() {
                out.gossip.enabled = b;
            }
        }

        if let Some(v) = get_path(&root, &["cluster", "gossip", "bind"]) {
            if let Some(s) = v.as_str() {
                if let Ok(addr) = s.parse::<SocketAddr>() {
                    out.gossip.bind = addr;
                }
            }
        }

        if let Some(v) = get_path(&root, &["cluster", "gossip", "advertise"]) {
            if let Some(s) = v.as_str() {
                if let Ok(addr) = s.parse::<SocketAddr>() {
                    out.gossip.advertise = addr;
                }
            }
        } else {
            // derive advertise if not present
            out.gossip.advertise = derive_advertise(out.gossip.bind);
        }

        if let Some(v) = get_path(&root, &["cluster", "gossip", "peers"]) {
            if let Some(arr) = v.as_array() {
                let mut peers = Vec::with_capacity(arr.len());
                for it in arr.iter() {
                    let Some(s) = it.as_str() else { continue };
                    let Ok(addr) = s.parse::<SocketAddr>() else {
                        continue;
                    };
                    peers.push(addr);
                }
                out.gossip.peers = peers;
            }
        }

        if let Some(v) = get_path(&root, &["cluster", "gossip", "interval"]) {
            if let Some(d) = parse_duration(v) {
                out.gossip.interval =
                    clamp_duration(d, Duration::from_millis(20), Duration::from_secs(10));
            }
        }

        if let Some(v) = get_path(&root, &["cluster", "gossip", "fanout"]) {
            if let Some(n) = v.as_u64() {
                out.gossip.fanout = (n as usize).clamp(1, 64);
            }
        }

        if let Some(v) = get_path(&root, &["cluster", "gossip", "suspicion_timeout"]) {
            if let Some(d) = parse_duration(v) {
                out.gossip.suspicion_timeout =
                    clamp_duration(d, Duration::from_millis(200), Duration::from_secs(300));
            }
        }

        if let Some(v) = get_path(&root, &["cluster", "gossip", "dead_timeout"]) {
            if let Some(d) = parse_duration(v) {
                out.gossip.dead_timeout =
                    clamp_duration(d, Duration::from_secs(1), Duration::from_secs(1800));
            }
        }

        if let Some(v) = get_path(&root, &["cluster", "gossip", "max_message_size"]) {
            if let Some(n) = v.as_u64() {
                out.gossip.max_message_size = (n as usize).clamp(256, 65_507);
            }
        }

        if let Some(v) = get_path(&root, &["cluster", "gossip", "retransmit_multiplier"]) {
            if let Some(n) = v.as_u64() {
                out.gossip.retransmit_multiplier = (n as usize).clamp(1, 16);
            }
        }

        // cluster.fallback.http_push
        if let Some(v) = get_path(&root, &["cluster", "fallback", "http_push"]) {
            if let Some(b) = v.as_bool() {
                out.fallback.http_push = b;
            }
        }

        out
    }
}

fn derive_advertise(bind: SocketAddr) -> SocketAddr {
    if bind.ip().is_unspecified() {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), bind.port())
    } else {
        bind
    }
}

fn get_path<'a>(root: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cur = root;
    for k in path {
        match cur {
            Value::Object(map) => {
                cur = map.get(*k)?;
            }
            _ => return None,
        }
    }
    Some(cur)
}

fn parse_duration(v: &Value) -> Option<Duration> {
    if let Some(n) = v.as_u64() {
        return Some(Duration::from_millis(n));
    }
    let s = v.as_str()?.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(ms) = s.strip_suffix("ms") {
        let n = ms.trim().parse::<u64>().ok()?;
        return Some(Duration::from_millis(n));
    }
    if let Some(sec) = s.strip_suffix('s') {
        let n = sec.trim().parse::<u64>().ok()?;
        return Some(Duration::from_secs(n));
    }
    // If it's a bare number in string form, interpret as ms.
    if let Ok(n) = s.parse::<u64>() {
        return Some(Duration::from_millis(n));
    }
    None
}

fn clamp_duration(d: Duration, min: Duration, max: Duration) -> Duration {
    if d < min {
        return min;
    }
    if d > max {
        return max;
    }
    d
}
