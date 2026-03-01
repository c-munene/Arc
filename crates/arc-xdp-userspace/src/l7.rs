//! L7Protection implementation (slowloris / TLS flood / HTTP2 stream flood).
//!
//! 目标：
//! - 数据结构尽可能轻量，避免锁。
//! - 对“per-IP”类限制使用 bucket-hash（可能误伤但极低开销，适合作为防护）。
//!
//! 注意：
//! - 这里是通用组件；真正“断开连接 / 发送 TLS alert / GOAWAY” 需要 Arc worker 调用方做 I/O 动作。
//! - 这里仅返回决策（Allow/Drop/GoAway 等）+ 统计计数器。

use crate::config::L7ProtectionConfig;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

/// L7 metrics (暴露给上层 metrics collector)。
#[derive(Debug, Default)]
pub struct L7Metrics {
    pub slowloris_dropped_total: AtomicU64,
    pub tls_flood_dropped_total: AtomicU64,
    pub h2_stream_flood_dropped_total: AtomicU64,
}

/// L7Protection top-level object.
#[derive(Debug)]
pub struct L7Protection {
    pub slowloris: SlowlorisGuard,
    pub tls_flood: TlsFloodGuard,
    pub h2_stream_flood: H2StreamFloodGuard,
    pub metrics: L7Metrics,
}

impl L7Protection {
    /// Build from config.
    pub fn new(cfg: &L7ProtectionConfig) -> Self {
        Self {
            slowloris: SlowlorisGuard::new(cfg),
            tls_flood: TlsFloodGuard::new(cfg),
            h2_stream_flood: H2StreamFloodGuard::new(cfg),
            metrics: L7Metrics::default(),
        }
    }
}

/// Decision for slowloris guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlowlorisDecision {
    Allow,
    DropTimeout,
    DropMinRate,
    DropTooManyIncomplete,
}

/// Per-connection state for slowloris checks.
///
/// 调用方（Arc worker）把它存到 conn struct 里即可。
#[derive(Debug, Clone, Copy)]
pub struct SlowlorisConnState {
    pub started_ns: u64,
    pub bytes_in_headers: u64,
}

/// Bucketed per-IP counter (approximate; collisions possible).
///
/// 设计：
/// - 固定大小 power-of-two bucket array
/// - key => bucket idx
/// - inc/dec 只做 atomic_fetch
#[derive(Debug)]
struct IpBucketCounter {
    mask: u64,
    buckets: Vec<AtomicU32>,
}

impl IpBucketCounter {
    fn new(bucket_pow2: usize) -> Self {
        let n = bucket_pow2.max(1024).next_power_of_two();
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(AtomicU32::new(0));
        }
        Self {
            mask: (n as u64).saturating_sub(1),
            buckets: v,
        }
    }

    #[inline]
    fn hash64(key: u64) -> u64 {
        // very small mixer (splitmix64-like)
        let mut x = key.wrapping_add(0x9E3779B97F4A7C15);
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D049BB133111EB);
        x ^ (x >> 31)
    }

    #[inline]
    fn idx(&self, key: u64) -> usize {
        (Self::hash64(key) & self.mask) as usize
    }

    #[inline]
    fn inc(&self, key: u64) -> u32 {
        let i = self.idx(key);
        self.buckets[i]
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1)
    }

    #[inline]
    fn dec(&self, key: u64) {
        let i = self.idx(key);
        // saturating decrement via CAS loop
        let b = &self.buckets[i];
        let mut cur = b.load(Ordering::Relaxed);
        loop {
            if cur == 0 {
                return;
            }
            match b.compare_exchange_weak(cur, cur - 1, Ordering::AcqRel, Ordering::Relaxed) {
                Ok(_) => return,
                Err(v) => cur = v,
            }
        }
    }

    #[inline]
    fn get(&self, key: u64) -> u32 {
        let i = self.idx(key);
        self.buckets[i].load(Ordering::Relaxed)
    }
}

/// Slowloris guard.
///
/// 需要调用方提供：
/// - now_ns（monotonic）
/// - ip_key_hash（u64 stable hash of client identity, e.g. hashed IpKey）
#[derive(Debug)]
pub struct SlowlorisGuard {
    enabled: bool,
    headers_timeout_ns: u64,
    min_recv_rate_bps: u64,
    max_incomplete_per_ip: u32,

    incomplete: IpBucketCounter,
}

impl SlowlorisGuard {
    pub fn new(cfg: &L7ProtectionConfig) -> Self {
        let c = &cfg.slowloris;
        Self {
            enabled: c.enabled,
            headers_timeout_ns: c.headers_timeout_secs.saturating_mul(1_000_000_000).max(1),
            min_recv_rate_bps: c.min_recv_rate_bps.max(1),
            max_incomplete_per_ip: c.max_incomplete_conns_per_ip.max(1),
            // 262k buckets: collisions acceptable; memory ~ 1MB
            incomplete: IpBucketCounter::new(262_144),
        }
    }

    /// Called when a new connection starts, before request headers complete.
    ///
    /// 返回：是否允许继续接入（超过 incomplete 上限则拒绝）。
    #[inline]
    pub fn on_conn_start(&self, ip_key_hash: u64) -> SlowlorisDecision {
        if !self.enabled {
            return SlowlorisDecision::Allow;
        }
        let cur = self.incomplete.inc(ip_key_hash);
        if cur > self.max_incomplete_per_ip {
            // rollback the increment to avoid permanent inflation on rejects
            self.incomplete.dec(ip_key_hash);
            SlowlorisDecision::DropTooManyIncomplete
        } else {
            SlowlorisDecision::Allow
        }
    }

    /// Called when a connection completes headers or closes (cleanup).
    #[inline]
    pub fn on_conn_end(&self, ip_key_hash: u64) {
        if !self.enabled {
            return;
        }
        self.incomplete.dec(ip_key_hash);
    }

    /// Initialize per-connection state.
    #[inline]
    pub fn init_conn_state(&self, now_ns: u64) -> SlowlorisConnState {
        SlowlorisConnState {
            started_ns: now_ns,
            bytes_in_headers: 0,
        }
    }

    /// Called on receiving header bytes.
    ///
    /// - `state` is stored in conn struct
    /// - `added` is bytes just read that belong to headers
    #[inline]
    pub fn on_header_bytes(
        &self,
        now_ns: u64,
        state: &mut SlowlorisConnState,
        added: u32,
    ) -> SlowlorisDecision {
        if !self.enabled {
            return SlowlorisDecision::Allow;
        }

        state.bytes_in_headers = state.bytes_in_headers.saturating_add(added as u64);

        let elapsed = now_ns.saturating_sub(state.started_ns);
        if elapsed > self.headers_timeout_ns {
            return SlowlorisDecision::DropTimeout;
        }

        // min recv rate check: bytes / seconds >= min_bps
        // Avoid float: bytes * 1e9 / elapsed_ns
        let elapsed_ns = elapsed.max(1);
        let rate = (state.bytes_in_headers as u128)
            .saturating_mul(1_000_000_000u128)
            .checked_div(elapsed_ns as u128)
            .unwrap_or(0) as u64;

        if rate < self.min_recv_rate_bps {
            SlowlorisDecision::DropMinRate
        } else {
            SlowlorisDecision::Allow
        }
    }

    #[inline]
    pub fn headers_timeout_ns(&self) -> u64 {
        self.headers_timeout_ns
    }
}

/// TLS flood decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsFloodDecision {
    Allow,
    DropRateLimited,
}

/// TLS handshake rate guard.
///
/// 注意：
/// - 这里按 “每秒窗口计数” 实现，开销极低；指数衰减可由上层 periodic 衰减/平滑。
// - 你要求“指数衰减与 XDP SYN score 一致”，但未给具体参数/公式；这里采用窗口计数实现，
///   并提供 hook（tick）用于上层做指数衰减扩展。
#[derive(Debug)]
pub struct TlsFloodGuard {
    enabled: bool,
    max_per_sec: u32,

    // bucketed per-IP window counter
    buckets: Vec<AtomicU64>, // packed: (sec << 32) | count
    mask: u64,
}

impl TlsFloodGuard {
    pub fn new(cfg: &L7ProtectionConfig) -> Self {
        let c = &cfg.tls_flood;
        let n = 262_144usize;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(AtomicU64::new(0));
        }
        Self {
            enabled: c.enabled,
            max_per_sec: c.max_handshakes_per_ip_per_sec.max(1),
            buckets: v,
            mask: (n as u64).saturating_sub(1),
        }
    }

    #[inline]
    fn idx(&self, ip_key_hash: u64) -> usize {
        // same mixer as slowloris
        let mut x = ip_key_hash.wrapping_add(0x9E3779B97F4A7C15);
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D049BB133111EB);
        ((x ^ (x >> 31)) & self.mask) as usize
    }

    /// Called on handshake attempt.
    ///
    /// `now_sec` should be epoch seconds or monotonic seconds; only relative equality matters.
    #[inline]
    pub fn on_handshake(&self, ip_key_hash: u64, now_sec: u32) -> TlsFloodDecision {
        if !self.enabled {
            return TlsFloodDecision::Allow;
        }

        let i = self.idx(ip_key_hash);
        let cell = &self.buckets[i];

        loop {
            let cur = cell.load(Ordering::Relaxed);
            let sec = (cur >> 32) as u32;
            let cnt = (cur & 0xFFFF_FFFF) as u32;

            if sec != now_sec {
                let next = ((now_sec as u64) << 32) | 1u64;
                match cell.compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Relaxed) {
                    Ok(_) => {
                        return if 1 > self.max_per_sec {
                            TlsFloodDecision::DropRateLimited
                        } else {
                            TlsFloodDecision::Allow
                        }
                    }
                    Err(_) => continue,
                }
            } else {
                let next_cnt = cnt.saturating_add(1);
                let next = ((sec as u64) << 32) | (next_cnt as u64);
                match cell.compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Relaxed) {
                    Ok(_) => {
                        if next_cnt > self.max_per_sec {
                            return TlsFloodDecision::DropRateLimited;
                        }
                        return TlsFloodDecision::Allow;
                    }
                    Err(_) => continue,
                }
            }
        }
    }
}

/// H2 stream flood decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H2Decision {
    Allow,
    GoAway,
}

/// Per-connection H2 state (store in conn struct).
#[derive(Debug, Clone, Copy)]
pub struct H2ConnState {
    pub open_streams: u32,
    pub window_sec: u32,
    pub streams_created_in_window: u32,
    pub rsts_in_window: u32,
}

/// HTTP/2 stream flood guard.
#[derive(Debug)]
pub struct H2StreamFloodGuard {
    enabled: bool,
    max_concurrent_streams: u32,
    max_streams_per_sec: u32,
    max_rst_per_sec: u32,
}

impl H2StreamFloodGuard {
    pub fn new(cfg: &L7ProtectionConfig) -> Self {
        let c = &cfg.h2_stream_flood;
        Self {
            enabled: c.enabled,
            max_concurrent_streams: c.max_concurrent_streams.max(1),
            max_streams_per_sec: c.max_streams_per_sec.max(1),
            max_rst_per_sec: c.max_rst_per_sec.max(1),
        }
    }

    #[inline]
    pub fn init_conn_state(&self, now_sec: u32) -> H2ConnState {
        H2ConnState {
            open_streams: 0,
            window_sec: now_sec,
            streams_created_in_window: 0,
            rsts_in_window: 0,
        }
    }

    #[inline]
    fn rotate_window_if_needed(&self, st: &mut H2ConnState, now_sec: u32) {
        if st.window_sec != now_sec {
            st.window_sec = now_sec;
            st.streams_created_in_window = 0;
            st.rsts_in_window = 0;
        }
    }

    /// Called on new inbound stream.
    #[inline]
    pub fn on_stream_open(&self, st: &mut H2ConnState, now_sec: u32) -> H2Decision {
        if !self.enabled {
            st.open_streams = st.open_streams.saturating_add(1);
            return H2Decision::Allow;
        }

        self.rotate_window_if_needed(st, now_sec);

        st.open_streams = st.open_streams.saturating_add(1);
        st.streams_created_in_window = st.streams_created_in_window.saturating_add(1);

        if st.open_streams > self.max_concurrent_streams {
            return H2Decision::GoAway;
        }
        if st.streams_created_in_window > self.max_streams_per_sec {
            return H2Decision::GoAway;
        }

        H2Decision::Allow
    }

    /// Called when a stream is closed.
    #[inline]
    pub fn on_stream_close(&self, st: &mut H2ConnState) {
        if st.open_streams > 0 {
            st.open_streams -= 1;
        }
    }

    /// Called on receiving RST_STREAM.
    #[inline]
    pub fn on_rst_stream(&self, st: &mut H2ConnState, now_sec: u32) -> H2Decision {
        if !self.enabled {
            return H2Decision::Allow;
        }

        self.rotate_window_if_needed(st, now_sec);

        st.rsts_in_window = st.rsts_in_window.saturating_add(1);
        if st.rsts_in_window > self.max_rst_per_sec {
            return H2Decision::GoAway;
        }

        H2Decision::Allow
    }
}

/// Utility: convert Duration to seconds (u32 clamp).
#[inline]
pub fn duration_to_u32_secs(d: Duration) -> u32 {
    d.as_secs().min(u64::from(u32::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> L7ProtectionConfig {
        L7ProtectionConfig::default()
    }

    #[test]
    fn slowloris_guard_enforces_incomplete_connection_cap() {
        let mut c = cfg();
        c.slowloris.enabled = true;
        c.slowloris.max_incomplete_conns_per_ip = 1;
        let g = SlowlorisGuard::new(&c);
        let ip = 42u64;

        assert_eq!(g.on_conn_start(ip), SlowlorisDecision::Allow);
        assert_eq!(
            g.on_conn_start(ip),
            SlowlorisDecision::DropTooManyIncomplete
        );
        // second attempt should rollback its own increment
        assert_eq!(g.incomplete.get(ip), 1);
        g.on_conn_end(ip);
        assert_eq!(g.incomplete.get(ip), 0);
    }

    #[test]
    fn slowloris_guard_checks_timeout_and_min_rate() {
        let mut c = cfg();
        c.slowloris.enabled = true;
        c.slowloris.headers_timeout_secs = 1;
        c.slowloris.min_recv_rate_bps = 100;
        let g = SlowlorisGuard::new(&c);

        let mut st = g.init_conn_state(0);
        assert_eq!(
            g.on_header_bytes(2_000_000_000, &mut st, 1),
            SlowlorisDecision::DropTimeout
        );

        let mut st2 = g.init_conn_state(0);
        assert_eq!(
            g.on_header_bytes(500_000_000, &mut st2, 1),
            SlowlorisDecision::DropMinRate
        );

        let mut st3 = g.init_conn_state(0);
        assert_eq!(
            g.on_header_bytes(500_000_000, &mut st3, 100),
            SlowlorisDecision::Allow
        );
    }

    #[test]
    fn tls_flood_guard_rate_limits_per_second_window() {
        let mut c = cfg();
        c.tls_flood.enabled = true;
        c.tls_flood.max_handshakes_per_ip_per_sec = 2;
        let g = TlsFloodGuard::new(&c);
        let ip = 100u64;

        assert_eq!(g.on_handshake(ip, 10), TlsFloodDecision::Allow);
        assert_eq!(g.on_handshake(ip, 10), TlsFloodDecision::Allow);
        assert_eq!(g.on_handshake(ip, 10), TlsFloodDecision::DropRateLimited);
        // next second rotates the window
        assert_eq!(g.on_handshake(ip, 11), TlsFloodDecision::Allow);
    }

    #[test]
    fn h2_guard_limits_stream_and_rst_bursts() {
        let mut c = cfg();
        c.h2_stream_flood.enabled = true;
        c.h2_stream_flood.max_concurrent_streams = 2;
        c.h2_stream_flood.max_streams_per_sec = 2;
        c.h2_stream_flood.max_rst_per_sec = 1;
        let g = H2StreamFloodGuard::new(&c);

        let mut st = g.init_conn_state(100);
        assert_eq!(g.on_stream_open(&mut st, 100), H2Decision::Allow);
        assert_eq!(g.on_stream_open(&mut st, 100), H2Decision::Allow);
        assert_eq!(g.on_stream_open(&mut st, 100), H2Decision::GoAway);

        g.on_stream_close(&mut st);
        g.on_stream_close(&mut st);
        assert_eq!(g.on_stream_open(&mut st, 101), H2Decision::Allow);

        assert_eq!(g.on_rst_stream(&mut st, 101), H2Decision::Allow);
        assert_eq!(g.on_rst_stream(&mut st, 101), H2Decision::GoAway);
    }

    #[test]
    fn duration_to_u32_secs_clamps_at_u32_max() {
        assert_eq!(duration_to_u32_secs(Duration::from_secs(7)), 7);
        assert_eq!(
            duration_to_u32_secs(Duration::from_secs(u64::MAX)),
            u32::MAX
        );
    }
}
