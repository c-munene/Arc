use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct ClusterCircuitConfig {
    pub enabled: bool,
    pub fail_streak_threshold: u32,
    pub open_ms: u64,
    pub half_open_probe_interval_ms: u64,
    pub peer_sync_interval_ms: u64,
    pub peer_ttl_ms: u64,
    pub peer_open_quorum: usize,
}

impl Default for ClusterCircuitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            fail_streak_threshold: 8,
            open_ms: 3_000,
            half_open_probe_interval_ms: 200,
            peer_sync_interval_ms: 1_000,
            peer_ttl_ms: 5_000,
            peer_open_quorum: 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeCircuitSnapshot {
    pub node_id: String,
    pub ts_ms: u64,
    pub open_until_ms: HashMap<String, u64>,
}

#[derive(Debug, Clone, Default)]
struct LocalCircuitEntry {
    fail_streak: u32,
    open_until_ms: u64,
    half_open_next_probe_ms: u64,
}

#[derive(Debug, Clone)]
struct PeerSnapshot {
    received_ms: u64,
    open_until_ms: HashMap<String, u64>,
}

pub struct ClusterCircuit {
    node_id: Arc<str>,
    cfg: ClusterCircuitConfig,
    open_ms: AtomicU64,
    half_open_probe_interval_ms: AtomicU64,
    local: Mutex<HashMap<String, LocalCircuitEntry>>,
    peers: Mutex<HashMap<String, PeerSnapshot>>,
    merged_open_until: ArcSwap<HashMap<String, u64>>,
}

impl ClusterCircuit {
    pub fn new(node_id: impl Into<String>, cfg: ClusterCircuitConfig) -> Self {
        Self {
            node_id: Arc::from(node_id.into()),
            open_ms: AtomicU64::new(cfg.open_ms.max(1)),
            half_open_probe_interval_ms: AtomicU64::new(cfg.half_open_probe_interval_ms.max(1)),
            cfg,
            local: Mutex::new(HashMap::new()),
            peers: Mutex::new(HashMap::new()),
            merged_open_until: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    #[inline]
    pub fn enabled(&self) -> bool {
        self.cfg.enabled
    }

    #[inline]
    pub fn gossip_interval(&self) -> Duration {
        Duration::from_millis(self.cfg.peer_sync_interval_ms.max(100))
    }

    #[inline]
    fn open_ms(&self) -> u64 {
        self.open_ms.load(Ordering::Relaxed).max(1)
    }

    #[inline]
    fn half_open_probe_interval_ms(&self) -> u64 {
        self.half_open_probe_interval_ms
            .load(Ordering::Relaxed)
            .max(1)
    }

    pub fn apply_hot_settings(&self, open_ms: u64, half_open_probe_interval_ms: u64) {
        self.open_ms.store(open_ms.max(1), Ordering::Relaxed);
        self.half_open_probe_interval_ms
            .store(half_open_probe_interval_ms.max(1), Ordering::Relaxed);
    }

    pub fn local_snapshot(&self) -> NodeCircuitSnapshot {
        let now_ms = now_ms();
        let mut open = HashMap::new();
        {
            let mut local = lock_unpoison(&self.local);
            local.retain(|_, v| {
                v.open_until_ms > now_ms || v.fail_streak > 0 || v.half_open_next_probe_ms > now_ms
            });
            for (addr, state) in local.iter() {
                let until = state.open_until_ms.max(state.half_open_next_probe_ms);
                if until > now_ms {
                    open.insert(addr.clone(), until);
                }
            }
        }
        NodeCircuitSnapshot {
            node_id: self.node_id.to_string(),
            ts_ms: now_ms,
            open_until_ms: open,
        }
    }

    pub fn ingest_peer_snapshot(&self, snapshot: NodeCircuitSnapshot) {
        if !self.cfg.enabled {
            return;
        }
        if snapshot.node_id.is_empty() || snapshot.node_id.as_str() == self.node_id.as_ref() {
            return;
        }
        let now = now_ms();
        {
            let mut peers = lock_unpoison(&self.peers);
            peers.insert(
                snapshot.node_id,
                PeerSnapshot {
                    received_ms: now,
                    open_until_ms: snapshot.open_until_ms,
                },
            );
        }
        self.rebuild(now);
    }

    pub fn record_failure(&self, addr: SocketAddr) {
        if !self.cfg.enabled {
            return;
        }
        let now = now_ms();
        {
            let mut local = lock_unpoison(&self.local);
            let key = addr.to_string();
            let entry = local.entry(key).or_default();
            if now < entry.open_until_ms {
                return;
            }

            // Half-open probe failed: re-open immediately.
            if entry.half_open_next_probe_ms > now && entry.open_until_ms <= now {
                entry.fail_streak = 0;
                entry.open_until_ms = now.saturating_add(self.open_ms());
                entry.half_open_next_probe_ms = entry.open_until_ms;
            } else {
                entry.fail_streak = entry.fail_streak.saturating_add(1);
                if entry.fail_streak >= self.cfg.fail_streak_threshold.max(1) {
                    entry.fail_streak = 0;
                    entry.open_until_ms = now.saturating_add(self.open_ms());
                    entry.half_open_next_probe_ms = entry.open_until_ms;
                } else if entry.half_open_next_probe_ms > 0 && now >= entry.half_open_next_probe_ms
                {
                    entry.half_open_next_probe_ms = 0;
                }
            }
        }
        self.rebuild(now);
    }

    pub fn record_success(&self, addr: SocketAddr) {
        if !self.cfg.enabled {
            return;
        }
        let now = now_ms();
        {
            let mut local = lock_unpoison(&self.local);
            if let Some(entry) = local.get_mut(&addr.to_string()) {
                entry.fail_streak = 0;
                entry.open_until_ms = 0;
                entry.half_open_next_probe_ms = 0;
            }
        }
        self.rebuild(now);
    }

    pub fn is_open(&self, addr: SocketAddr) -> bool {
        if !self.cfg.enabled {
            return false;
        }
        let now = now_ms();
        let key = addr.to_string();

        if self
            .merged_open_until
            .load()
            .get(&key)
            .copied()
            .unwrap_or(0)
            > now
        {
            return true;
        }

        let mut allow_probe_now = false;
        let mut open = false;
        {
            let mut local = lock_unpoison(&self.local);
            if let Some(entry) = local.get_mut(&key) {
                if now < entry.open_until_ms {
                    open = true;
                } else if entry.half_open_next_probe_ms > 0 {
                    if now < entry.half_open_next_probe_ms {
                        open = true;
                    } else {
                        // Open window elapsed: allow one probe and throttle next probes.
                        entry.half_open_next_probe_ms =
                            now.saturating_add(self.half_open_probe_interval_ms());
                        allow_probe_now = true;
                    }
                }
            }
        }

        if allow_probe_now {
            self.rebuild(now);
        }

        open
    }

    fn rebuild(&self, now_ms: u64) {
        let mut merged: HashMap<String, u64> = HashMap::new();

        {
            let mut local = lock_unpoison(&self.local);
            local.retain(|_, v| {
                v.open_until_ms > now_ms || v.fail_streak > 0 || v.half_open_next_probe_ms > now_ms
            });
            for (addr, state) in local.iter() {
                let until = state.open_until_ms.max(state.half_open_next_probe_ms);
                if until > now_ms {
                    merged.insert(addr.clone(), until);
                }
            }
        }

        {
            let mut peers = lock_unpoison(&self.peers);
            peers.retain(|_, s| s.received_ms.saturating_add(self.cfg.peer_ttl_ms) > now_ms);

            let mut votes: HashMap<String, (usize, u64)> = HashMap::new();
            for snap in peers.values() {
                for (addr, until) in snap.open_until_ms.iter() {
                    if *until <= now_ms {
                        continue;
                    }
                    let e = votes.entry(addr.clone()).or_insert((0, 0));
                    e.0 = e.0.saturating_add(1);
                    e.1 = e.1.max(*until);
                }
            }

            let quorum = self.cfg.peer_open_quorum.max(1);
            for (addr, (count, until)) in votes {
                if count < quorum {
                    continue;
                }
                merged
                    .entry(addr)
                    .and_modify(|v| *v = (*v).max(until))
                    .or_insert(until);
            }
        }

        self.merged_open_until.store(Arc::new(merged));
    }
}

#[inline]
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[inline]
fn lock_unpoison<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn half_open_probe_is_throttled() {
        let mut cfg = ClusterCircuitConfig::default();
        cfg.fail_streak_threshold = 1;
        cfg.open_ms = 20;
        cfg.half_open_probe_interval_ms = 40;

        let circuit = ClusterCircuit::new("node-a", cfg);
        let up: SocketAddr = "127.0.0.1:19099".parse().expect("valid addr");

        circuit.record_failure(up);
        assert!(circuit.is_open(up));

        thread::sleep(Duration::from_millis(25));
        assert!(!circuit.is_open(up));
        assert!(circuit.is_open(up));
    }

    #[test]
    fn half_open_probe_failure_reopens_immediately() {
        let mut cfg = ClusterCircuitConfig::default();
        cfg.fail_streak_threshold = 3;
        cfg.open_ms = 20;
        cfg.half_open_probe_interval_ms = 50;

        let circuit = ClusterCircuit::new("node-a", cfg);
        let up: SocketAddr = "127.0.0.1:19199".parse().expect("valid addr");

        circuit.record_failure(up);
        circuit.record_failure(up);
        circuit.record_failure(up);
        assert!(circuit.is_open(up));

        thread::sleep(Duration::from_millis(25));
        assert!(!circuit.is_open(up));
        circuit.record_failure(up);
        assert!(circuit.is_open(up));
    }

    #[test]
    fn hot_settings_apply_to_next_open_cycle() {
        let mut cfg = ClusterCircuitConfig::default();
        cfg.fail_streak_threshold = 1;
        cfg.open_ms = 1_000;
        cfg.half_open_probe_interval_ms = 1_000;

        let circuit = ClusterCircuit::new("node-a", cfg);
        let up: SocketAddr = "127.0.0.1:19299".parse().expect("valid addr");

        // Apply hot settings for next cycle, then re-enter open.
        circuit.apply_hot_settings(20, 10);
        circuit.record_failure(up);
        assert!(circuit.is_open(up));

        // open_ms hot value should take effect in newly-opened cycle.
        thread::sleep(Duration::from_millis(30));
        assert!(!circuit.is_open(up));
        assert!(circuit.is_open(up));

        // half_open_probe_interval_ms hot value should also take effect.
        thread::sleep(Duration::from_millis(15));
        assert!(!circuit.is_open(up));
    }
}
