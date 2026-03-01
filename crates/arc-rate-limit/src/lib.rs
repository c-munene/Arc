//! arc-rate-limit
//!
//! 这个 crate 是什么：
//! - 无锁限流器实现，热路径仅原子 CAS，无 Mutex/RwLock。
//!
//! 算法选择：GCRA（Generic Cell Rate Algorithm）
//! - 可视为 token-bucket 的等价实现方式：
//!   - interval = 1/rate
//!   - burst 通过 “允许的最大超前量” 控制
//! - 优点：只需要一个 atomic(u64) 保存 TAT（theoretical arrival time），比维护 (tokens,last) 更易无锁实现。
//!
//! 精度取舍：
//! - 使用纳秒时间戳，避免浮点。
//! - 所有运算使用饱和/扩展到 u128 以避免溢出。

use arc_common::{ArcError, Result};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug)]
pub struct Limiter {
    interval_ns: u64,
    burst_ns: u64,
    tat_ns: AtomicU64,
}

impl Limiter {
    pub fn new(rate_per_sec: u64, burst: u64) -> Result<Self> {
        if rate_per_sec == 0 {
            return Err(ArcError::rate_limit("rate_per_sec must be > 0"));
        }
        if burst == 0 {
            return Err(ArcError::rate_limit("burst must be > 0"));
        }

        let interval_ns = (1_000_000_000u64).checked_div(rate_per_sec).unwrap_or(1);

        let burst_ns = (interval_ns as u128)
            .saturating_mul(burst as u128)
            .min(u64::MAX as u128) as u64;

        Ok(Self {
            interval_ns,
            burst_ns,
            tat_ns: AtomicU64::new(0),
        })
    }

    /// Try to consume 1 token at `now_ns`.
    ///
    /// Returns `true` if allowed.
    #[inline]
    pub fn allow(&self, now_ns: u64) -> bool {
        // GCRA:
        // new_tat = max(tat, now) + interval
        // if new_tat - now > burst_ns => reject
        // else CAS(tat=new_tat)
        let mut tat = self.tat_ns.load(Ordering::Relaxed);

        loop {
            let base = if tat > now_ns { tat } else { now_ns };
            let new_tat = base.saturating_add(self.interval_ns);
            let ahead = new_tat.saturating_sub(now_ns);

            if ahead > self.burst_ns {
                return false;
            }

            match self.tat_ns.compare_exchange_weak(
                tat,
                new_tat,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(cur) => tat = cur,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_params_rejected() {
        assert!(Limiter::new(0, 1).is_err());
        assert!(Limiter::new(10, 0).is_err());
    }

    #[test]
    fn burst_and_recovery_behaviour() {
        let lim = Limiter::new(10, 2).expect("limiter");
        let t0 = 1_000_000_000u64;

        assert!(lim.allow(t0));
        assert!(lim.allow(t0));
        assert!(!lim.allow(t0));

        // 10 rps => interval 100ms; after 100ms there should be one token-equivalent window.
        let t1 = t0 + 100_000_000u64;
        assert!(lim.allow(t1));
    }
}
