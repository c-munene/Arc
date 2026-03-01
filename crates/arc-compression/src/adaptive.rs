//! Adaptive compression level controller.
//!
//! Spec mapping:
//! - 3-state machine: Normal / Degraded / Enhanced
//! - CPU > high_threshold持续 check_interval => Degraded (level - 1)
//! - CPU < low_threshold持续 check_interval  => Enhanced (level + 1)
//! - Enter Degraded/Enhanced => cooldown；冷却期不再调整
//! - cooldown结束 => 回到 Normal 重新评估
//! - 只在同算法内调整，不跨算法切换
//! - 只影响新请求，不影响正在压缩中的响应
//!
//! 这个模块 **不创建线程**；数据面可以在已有 tick 或 metrics 刷新点调用 `maybe_adjust`。

use std::sync::atomic::{AtomicI32, AtomicU64, AtomicU8, Ordering};
use std::time::Duration;

use crate::{clamp_i32, Algorithm};

/// 自适应配置（运行时解析后）。
#[derive(Clone, Debug)]
pub struct AdaptiveConfig {
    /// 是否启用自适应。
    ///
    /// Default: true.
    pub enabled: bool,

    /// CPU 使用率高阈值（0.0 - 1.0）。
    ///
    /// Default: 0.80.
    pub cpu_high_threshold: f64,

    /// CPU 使用率低阈值（0.0 - 1.0）。
    ///
    /// Default: 0.30.
    pub cpu_low_threshold: f64,

    /// 检查间隔，防止震荡。
    ///
    /// Default: 5s.
    pub check_interval: Duration,

    /// 冷却时间，防止震荡。
    ///
    /// Default: 30s.
    pub cooldown: Duration,
}

impl Default for AdaptiveConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cpu_high_threshold: 0.80,
            cpu_low_threshold: 0.30,
            check_interval: Duration::from_secs(5),
            cooldown: Duration::from_secs(30),
        }
    }
}

/// 自适应状态机状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdaptiveState {
    /// 正常评估中。
    Normal,
    /// 降级（降低 level）。
    Degraded,
    /// 增强（提高 level）。
    Enhanced,
}

impl AdaptiveState {
    #[inline]
    fn as_u8(self) -> u8 {
        match self {
            AdaptiveState::Normal => 0,
            AdaptiveState::Degraded => 1,
            AdaptiveState::Enhanced => 2,
        }
    }

    #[inline]
    fn from_u8(v: u8) -> AdaptiveState {
        match v {
            1 => AdaptiveState::Degraded,
            2 => AdaptiveState::Enhanced,
            _ => AdaptiveState::Normal,
        }
    }
}

/// 调整方向。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdaptiveDirection {
    /// level 上调。
    Up,
    /// level 下调。
    Down,
}

/// 一次调整事件（用于 metrics 或 routes explain）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdaptiveAdjustment {
    /// 调整方向。
    pub direction: AdaptiveDirection,
    /// 调整后进入的状态。
    pub state: AdaptiveState,
}

/// 自适应控制器（无锁热路径读取；稀疏写入用 CAS 限流）。
#[derive(Debug)]
pub struct AdaptiveController {
    cfg: AdaptiveConfig,

    state: AtomicU8,
    last_check_ns: AtomicU64,
    cooldown_until_ns: AtomicU64,

    // per algorithm level offset (applied to base level).
    // We clamp offset itself to avoid unbounded drift if caller keeps feeding extreme CPU samples.
    zstd_off: AtomicI32,
    br_off: AtomicI32,
    gzip_off: AtomicI32,
}

impl AdaptiveController {
    /// Create a new controller.
    pub fn new(cfg: AdaptiveConfig) -> Self {
        Self {
            cfg,
            state: AtomicU8::new(AdaptiveState::Normal.as_u8()),
            last_check_ns: AtomicU64::new(0),
            cooldown_until_ns: AtomicU64::new(0),
            zstd_off: AtomicI32::new(0),
            br_off: AtomicI32::new(0),
            gzip_off: AtomicI32::new(0),
        }
    }

    /// Current adaptive state.
    #[inline]
    pub fn state(&self) -> AdaptiveState {
        AdaptiveState::from_u8(self.state.load(Ordering::Relaxed))
    }

    /// Current level offset for algorithm (can be negative).
    #[inline]
    pub fn level_offset(&self, alg: Algorithm) -> i32 {
        match alg {
            Algorithm::Zstd => self.zstd_off.load(Ordering::Relaxed),
            Algorithm::Br => self.br_off.load(Ordering::Relaxed),
            Algorithm::Gzip => self.gzip_off.load(Ordering::Relaxed),
            Algorithm::Identity => 0,
        }
    }

    /// Apply the current offset to `base_level` and clamp to algorithm-allowed range.
    ///
    /// Allowed ranges (real-time safe defaults):
    /// - zstd: 1..=5
    /// - gzip: 1..=9
    /// - br:   4..=6
    #[inline]
    pub fn apply_level(&self, alg: Algorithm, base_level: i32) -> i32 {
        let off = self.level_offset(alg);
        let lv = base_level.saturating_add(off);
        match alg {
            Algorithm::Zstd => clamp_i32(lv, 1, 5),
            Algorithm::Gzip => clamp_i32(lv, 1, 9),
            Algorithm::Br => clamp_i32(lv, 4, 6),
            Algorithm::Identity => base_level,
        }
    }

    /// The "current level" used for metrics gauge.
    ///
    /// Spec says: `arc_compression_level_current{algorithm}` should expose the **effective level**
    /// after adaptive adjustments.
    ///
    /// Arc 有多个 size bucket 的 base level；这里选用 1KB-100KB 桶的 base level 作为 gauge 基准：
    /// - zstd base=3
    /// - gzip base=6
    /// - br   base=5
    ///
    /// 调用方（arc-gateway）可直接用此返回值写 metrics gauge。
    #[inline]
    pub fn current_level_for_gauge(&self, alg: Algorithm) -> i32 {
        let base = match alg {
            Algorithm::Zstd => 3,
            Algorithm::Gzip => 6,
            Algorithm::Br => 5,
            Algorithm::Identity => 0,
        };
        self.apply_level(alg, base)
    }

    /// Feed a CPU usage sample and maybe adjust level offsets.
    ///
    /// - `now_ns`: monotonic timestamp
    /// - `cpu`: CPU utilization in [0.0, 1.0] (caller must clamp)
    ///
    /// Returns `Some(AdaptiveAdjustment)` only when an actual adjustment happens.
    pub fn maybe_adjust(&self, now_ns: u64, mut cpu: f64) -> Option<AdaptiveAdjustment> {
        if !self.cfg.enabled {
            return None;
        }

        if cpu.is_nan() {
            return None;
        }
        if cpu < 0.0 {
            cpu = 0.0;
        }
        if cpu > 1.0 {
            cpu = 1.0;
        }

        // If we're in Degraded/Enhanced, enforce cooldown.
        // But when load crosses the opposite threshold, allow immediate flip to avoid
        // "stuck during cooldown" behavior under fast load changes.
        let st = self.state();
        if st != AdaptiveState::Normal {
            if st == AdaptiveState::Enhanced && cpu > self.cfg.cpu_high_threshold {
                self.bump_offsets(-1);
                self.state
                    .store(AdaptiveState::Degraded.as_u8(), Ordering::Relaxed);
                let cd = self.cfg.cooldown.as_nanos().min(u64::MAX as u128) as u64;
                self.cooldown_until_ns
                    .store(now_ns.saturating_add(cd), Ordering::Relaxed);
                self.last_check_ns.store(now_ns, Ordering::Relaxed);
                return Some(AdaptiveAdjustment {
                    direction: AdaptiveDirection::Down,
                    state: AdaptiveState::Degraded,
                });
            }
            if st == AdaptiveState::Degraded && cpu < self.cfg.cpu_low_threshold {
                self.bump_offsets(1);
                self.state
                    .store(AdaptiveState::Enhanced.as_u8(), Ordering::Relaxed);
                let cd = self.cfg.cooldown.as_nanos().min(u64::MAX as u128) as u64;
                self.cooldown_until_ns
                    .store(now_ns.saturating_add(cd), Ordering::Relaxed);
                self.last_check_ns.store(now_ns, Ordering::Relaxed);
                return Some(AdaptiveAdjustment {
                    direction: AdaptiveDirection::Up,
                    state: AdaptiveState::Enhanced,
                });
            }
            let until = self.cooldown_until_ns.load(Ordering::Relaxed);
            if now_ns < until {
                return None;
            }
            // cooldown ended: back to Normal, and restart interval window.
            self.state
                .store(AdaptiveState::Normal.as_u8(), Ordering::Relaxed);
            self.last_check_ns.store(now_ns, Ordering::Relaxed);
            return None;
        }

        let interval_ns = self.cfg.check_interval.as_nanos().min(u64::MAX as u128) as u64;
        let last = self.last_check_ns.load(Ordering::Relaxed);
        if last != 0 && now_ns.saturating_sub(last) < interval_ns {
            return None;
        }
        // CAS to ensure only one caller performs the adjustment per interval.
        if self
            .last_check_ns
            .compare_exchange(last, now_ns, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return None;
        }

        if cpu > self.cfg.cpu_high_threshold {
            self.bump_offsets(-1);
            self.state
                .store(AdaptiveState::Degraded.as_u8(), Ordering::Relaxed);
            let cd = self.cfg.cooldown.as_nanos().min(u64::MAX as u128) as u64;
            self.cooldown_until_ns
                .store(now_ns.saturating_add(cd), Ordering::Relaxed);
            return Some(AdaptiveAdjustment {
                direction: AdaptiveDirection::Down,
                state: AdaptiveState::Degraded,
            });
        }

        if cpu < self.cfg.cpu_low_threshold {
            self.bump_offsets(1);
            self.state
                .store(AdaptiveState::Enhanced.as_u8(), Ordering::Relaxed);
            let cd = self.cfg.cooldown.as_nanos().min(u64::MAX as u128) as u64;
            self.cooldown_until_ns
                .store(now_ns.saturating_add(cd), Ordering::Relaxed);
            return Some(AdaptiveAdjustment {
                direction: AdaptiveDirection::Up,
                state: AdaptiveState::Enhanced,
            });
        }

        // remain Normal
        None
    }

    fn bump_offsets(&self, delta: i32) {
        // clamp offset to [-8, +8] to avoid unbounded drift
        bump_one(&self.zstd_off, delta);
        bump_one(&self.br_off, delta);
        bump_one(&self.gzip_off, delta);

        fn bump_one(a: &AtomicI32, delta: i32) {
            let mut cur = a.load(Ordering::Relaxed);
            loop {
                let next = clamp_i32(cur.saturating_add(delta), -8, 8);
                match a.compare_exchange(cur, next, Ordering::Relaxed, Ordering::Relaxed) {
                    Ok(_) => return,
                    Err(v) => cur = v,
                }
            }
        }
    }
}
