use crate::config::{
    EndpointConfig, LoadBalancing, UpstreamConfig, UpstreamDiscovery, UpstreamTimeouts,
};
use ahash::AHashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

/// Upstream registry holds all named upstream groups.
#[derive(Debug)]
pub struct UpstreamRegistry {
    groups: AHashMap<Arc<str>, Arc<UpstreamGroup>>,
}

impl UpstreamRegistry {
    /// Build from config.
    pub fn build(cfgs: &[UpstreamConfig]) -> anyhow::Result<Self> {
        let mut groups = AHashMap::new();
        for u in cfgs {
            let g = UpstreamGroup::from_config(u)?;
            groups.insert(Arc::<str>::from(u.name.clone()), Arc::new(g));
        }
        Ok(Self { groups })
    }

    /// Get by name.
    pub fn get(&self, name: &str) -> Option<Arc<UpstreamGroup>> {
        self.groups.get(name).cloned()
    }
}

/// Upstream group contains endpoint set, lb algo, health + stats.
#[derive(Debug)]
pub struct UpstreamGroup {
    pub name: Arc<str>,
    pub algo: LoadBalancing,
    pub endpoints: Vec<EndpointState>,
    pub rr: AtomicU64,
    pub timeouts: UpstreamTimeouts,
    pub decay: Duration,
    start: Instant,
}

#[derive(Debug)]
pub struct EndpointState {
    pub address: Arc<str>,
    pub weight: u32,
    pub healthy: AtomicBool,
    pub inflight: AtomicU64,

    /// Peak EWMA (microseconds).
    ewma_us: AtomicU64,
    /// Last update (unix nanos) for decay.
    last_nanos: AtomicU64,
}

impl UpstreamGroup {
    pub fn from_config(cfg: &UpstreamConfig) -> anyhow::Result<Self> {
        let mut endpoints: Vec<EndpointState> = match &cfg.discovery {
            UpstreamDiscovery::Static { endpoints } => endpoints
                .iter()
                .map(|e| EndpointState::from_cfg(e))
                .collect(),
            UpstreamDiscovery::Dns { hostname, port, .. } => {
                // DNS discovery is async and runs in background.
                // For compilation, create an empty placeholder; control plane will fill.
                vec![EndpointState::from_cfg(&EndpointConfig {
                    address: format!("{hostname}:{port}"),
                    weight: 1,
                })]
            }
        };

        // Ensure deterministic ordering.
        endpoints.sort_by(|a, b| a.address.as_ref().cmp(b.address.as_ref()));

        let decay = match &cfg.lb {
            LoadBalancing::PeakEwma { decay } => *decay,
            _ => Duration::from_secs(10),
        };

        Ok(Self {
            name: Arc::<str>::from(cfg.name.clone()),
            algo: cfg.lb.clone(),
            endpoints,
            rr: AtomicU64::new(0),
            timeouts: cfg.timeouts.clone(),
            decay,
            start: Instant::now(),
        })
    }

    /// Select an endpoint index.
    ///
    /// `hash_key` is already computed (for consistent hash / deterministic split).
    pub fn select(&self, hash_key: u64) -> Option<usize> {
        // Fast path: single-endpoint group should not pay per-request LB bookkeeping costs.
        if self.endpoints.len() == 1 {
            return self
                .endpoints
                .first()
                .and_then(|e| e.healthy.load(Ordering::Relaxed).then_some(0));
        }

        match &self.algo {
            LoadBalancing::RoundRobin => self.select_rr(),
            LoadBalancing::WeightedRoundRobin => self.select_wrr(),
            LoadBalancing::LeastRequests => self.select_least(),
            LoadBalancing::ConsistentHash { .. } => self.select_consistent(hash_key),
            LoadBalancing::PeakEwma { .. } => self.select_peak_ewma(),
        }
    }

    fn needs_inflight(&self) -> bool {
        self.endpoints.len() > 1
            && matches!(
                self.algo,
                LoadBalancing::LeastRequests | LoadBalancing::PeakEwma { .. }
            )
    }

    fn needs_ewma(&self) -> bool {
        self.endpoints.len() > 1 && matches!(self.algo, LoadBalancing::PeakEwma { .. })
    }

    fn select_rr(&self) -> Option<usize> {
        let n = self.endpoints.len();
        if n == 0 {
            return None;
        }
        for _ in 0..n {
            let i = (self.rr.fetch_add(1, Ordering::Relaxed) as usize) % n;
            if self.endpoints[i].healthy.load(Ordering::Relaxed) {
                return Some(i);
            }
        }
        None
    }

    fn select_wrr(&self) -> Option<usize> {
        // For simplicity, do weighted random via prefix sum.
        // For production: use alias table per update.
        let mut total: u64 = 0;
        for e in &self.endpoints {
            if e.healthy.load(Ordering::Relaxed) {
                total += e.weight as u64;
            }
        }
        if total == 0 {
            return None;
        }
        let mut x = self.rr.fetch_add(1, Ordering::Relaxed) % total;
        for (i, e) in self.endpoints.iter().enumerate() {
            if !e.healthy.load(Ordering::Relaxed) {
                continue;
            }
            let w = e.weight as u64;
            if x < w {
                return Some(i);
            }
            x -= w;
        }
        None
    }

    fn select_least(&self) -> Option<usize> {
        let mut best: Option<(usize, u64)> = None;
        for (i, e) in self.endpoints.iter().enumerate() {
            if !e.healthy.load(Ordering::Relaxed) {
                continue;
            }
            let inflight = e.inflight.load(Ordering::Relaxed);
            match best {
                None => best = Some((i, inflight)),
                Some((_, best_in)) if inflight < best_in => best = Some((i, inflight)),
                _ => {}
            }
        }
        best.map(|(i, _)| i)
    }

    fn select_consistent(&self, hash_key: u64) -> Option<usize> {
        // Minimal consistent hash: modulo on healthy endpoints.
        // For production: use ketama ring with virtual nodes.
        let healthy: Vec<usize> = self
            .endpoints
            .iter()
            .enumerate()
            .filter(|(_, e)| e.healthy.load(Ordering::Relaxed))
            .map(|(i, _)| i)
            .collect();
        if healthy.is_empty() {
            return None;
        }
        Some(healthy[(hash_key as usize) % healthy.len()])
    }

    fn select_peak_ewma(&self) -> Option<usize> {
        // Peak EWMA score = ewma_latency * (inflight + 1)
        let mut best: Option<(usize, u128)> = None;
        for (i, e) in self.endpoints.iter().enumerate() {
            if !e.healthy.load(Ordering::Relaxed) {
                continue;
            }
            let ewma = e.ewma_us.load(Ordering::Relaxed) as u128;
            let inflight = e.inflight.load(Ordering::Relaxed) as u128;
            let score = ewma.saturating_mul(inflight.saturating_add(1));
            match best {
                None => best = Some((i, score)),
                Some((_, s)) if score < s => best = Some((i, score)),
                _ => {}
            }
        }
        best.map(|(i, _)| i)
    }

    /// Called when a request is assigned to endpoint.
    pub fn on_request_start(&self, idx: usize) {
        if !self.needs_inflight() {
            return;
        }
        let e = &self.endpoints[idx];
        e.inflight.fetch_add(1, Ordering::Relaxed);
    }

    /// Called when a request finishes; updates EWMA.
    pub fn on_request_end(&self, idx: usize, latency: Duration, ok: bool) {
        let e = &self.endpoints[idx];
        if self.needs_inflight() {
            e.inflight.fetch_sub(1, Ordering::Relaxed);
        }

        // Passive health signal.
        if !ok {
            // In production: feed outlier detector / ejection.
        }

        if !self.needs_ewma() {
            return;
        }

        let sample_us = latency.as_micros() as u64;
        // Monotonic timestamp relative to group creation.
        let now_nanos = (self.start.elapsed().as_nanos() as u64).max(1);
        let last = e.last_nanos.swap(now_nanos, Ordering::Relaxed);
        let dt = now_nanos.saturating_sub(last);

        let old = e.ewma_us.load(Ordering::Relaxed);
        let updated = ewma_decay(old, sample_us, dt, self.decay);
        e.ewma_us.store(updated, Ordering::Relaxed);
    }
}

impl EndpointState {
    fn from_cfg(cfg: &EndpointConfig) -> Self {
        Self {
            address: Arc::<str>::from(cfg.address.clone()),
            weight: cfg.weight,
            healthy: AtomicBool::new(true),
            inflight: AtomicU64::new(0),
            ewma_us: AtomicU64::new(1_000), // start at 1ms
            last_nanos: AtomicU64::new(1),
        }
    }
}

fn ewma_decay(old: u64, sample: u64, dt_nanos: u64, decay: Duration) -> u64 {
    // Peak EWMA uses exp(-dt/decay).
    // We approximate exp with a rational approximation for speed.
    let decay_nanos = decay.as_nanos() as f64;
    if decay_nanos <= 0.0 {
        return sample;
    }
    let dt = dt_nanos as f64;
    let w = (-dt / decay_nanos).exp();
    let newf = (old as f64) * w + (sample as f64) * (1.0 - w);
    newf.max(1.0) as u64
}
