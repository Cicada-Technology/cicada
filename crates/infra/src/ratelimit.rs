//! 限流原语（第 7 层 `rate_limit`，以及 套餐的 `api_qps_limit`）。
//!
//! 基于 `governor` 的 GCRA/令牌桶：**只提供进程内、按 key 的检查**，配额由调用方按维度解析后
//! 传入——接入层用「单 IP 全局配额」，鉴权之后用「租户套餐的 QPS 上限」。因此本模块不认识
//! HTTP，也不查库，可单测。
//!
//! 计数在进程内：多副本部署时限流按实例生效，要全局精确配额需换 Redis 实现（ADR 0005）。

use std::collections::HashMap;
use std::hash::Hash;
use std::num::NonZeroU32;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use governor::clock::{Clock, DefaultClock};
use governor::state::keyed::DefaultKeyedStateStore;
use governor::{Quota, RateLimiter as Governor};

/// 单个 keyed 限流器保留的最大 key 数；超过即回收已回到初始状态的 key，避免内存无界增长。
const MAX_KEYS_PER_BUCKET: usize = 16 * 1024;

/// 一个维度的配额：每秒 `qps` 次，令牌桶容量 `burst`（允许的瞬时突发）。
///
/// `qps == 0` 表示**不限**（与套餐里 `api_qps_limit < 0` 的约定对齐）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Limit {
    pub qps: u32,
    pub burst: u32,
}

impl Limit {
    /// `burst` 不会小于 `qps`：否则稳态速率达不到 `qps`。
    pub fn new(qps: u32, burst: u32) -> Self {
        Self {
            qps,
            burst: burst.max(qps),
        }
    }

    /// 不限流。
    pub const UNLIMITED: Self = Self { qps: 0, burst: 0 };

    pub fn is_unlimited(self) -> bool {
        self.qps == 0
    }

    fn quota(self) -> Option<Quota> {
        let qps = NonZeroU32::new(self.qps)?;
        let burst = NonZeroU32::new(self.burst.max(1))?;
        Some(Quota::per_second(qps).allow_burst(burst))
    }
}

/// 触发限流后的等待时间，供 429 的 `Retry-After` 使用（向上取整，至少 1 秒）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimited {
    pub retry_after_secs: u64,
}

type KeyedGovernor<K> = Governor<K, DefaultKeyedStateStore<K>, DefaultClock>;

/// 进程内、按 key 的令牌桶集合：每个**不同配额**对应一个 keyed 限流器（配额种类通常极少，
/// 例如「全局默认」与若干套餐档位），因此查表与建桶都是一次哈希。
pub struct RateLimiter<K>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
{
    buckets: RwLock<HashMap<Limit, Arc<KeyedGovernor<K>>>>,
}

impl<K> Default for RateLimiter<K>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
{
    fn default() -> Self {
        Self {
            buckets: RwLock::new(HashMap::new()),
        }
    }
}

impl<K> RateLimiter<K>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
{
    pub fn new() -> Self {
        Self::default()
    }

    /// 检查一次访问：不限配额直接放行，超限返回需要等待的时间。
    pub fn check(&self, key: &K, limit: Limit) -> Result<(), RateLimited> {
        let Some(quota) = limit.quota() else {
            return Ok(());
        };
        let bucket = self.bucket(limit, quota);
        if bucket.len() > MAX_KEYS_PER_BUCKET {
            bucket.retain_recent();
        }
        match bucket.check_key(key) {
            Ok(()) => Ok(()),
            Err(negative) => {
                let wait = negative.wait_time_from(bucket.clock().now());
                Err(RateLimited {
                    retry_after_secs: wait_secs(wait),
                })
            }
        }
    }

    /// 当前已建出的配额桶数量（观测/测试用）。
    pub fn bucket_count(&self) -> usize {
        self.buckets.read().expect("限流器锁中毒").len()
    }

    fn bucket(&self, limit: Limit, quota: Quota) -> Arc<KeyedGovernor<K>> {
        if let Some(existing) = self.buckets.read().expect("限流器锁中毒").get(&limit) {
            return existing.clone();
        }
        let mut map = self.buckets.write().expect("限流器锁中毒");
        map.entry(limit)
            .or_insert_with(|| Arc::new(Governor::keyed(quota)))
            .clone()
    }
}

fn wait_secs(wait: Duration) -> u64 {
    ((wait.as_millis() as u64).div_ceil(1000)).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_passes_forever() {
        let limiter: RateLimiter<&str> = RateLimiter::new();
        for _ in 0..1000 {
            assert_eq!(limiter.check(&"a", Limit::UNLIMITED), Ok(()));
        }
        assert_eq!(limiter.bucket_count(), 0);
    }

    #[test]
    fn burst_is_consumed_then_limited() {
        let limiter: RateLimiter<&str> = RateLimiter::new();
        let limit = Limit::new(1, 3);
        assert_eq!(limiter.check(&"a", limit), Ok(()));
        assert_eq!(limiter.check(&"a", limit), Ok(()));
        assert_eq!(limiter.check(&"a", limit), Ok(()));
        let denied = limiter.check(&"a", limit).expect_err("第 4 次应超限");
        assert!(denied.retry_after_secs >= 1);
    }

    #[test]
    fn keys_are_independent() {
        let limiter: RateLimiter<&str> = RateLimiter::new();
        let limit = Limit::new(1, 1);
        assert_eq!(limiter.check(&"a", limit), Ok(()));
        assert_eq!(limiter.check(&"b", limit), Ok(()));
        assert!(limiter.check(&"a", limit).is_err());
    }

    #[test]
    fn burst_never_below_qps() {
        assert_eq!(Limit::new(10, 1).burst, 10);
        assert_eq!(Limit::new(10, 20).burst, 20);
    }

    #[test]
    fn distinct_limits_get_distinct_buckets() {
        let limiter: RateLimiter<i64> = RateLimiter::new();
        assert_eq!(limiter.check(&1, Limit::new(1, 1)), Ok(()));
        assert_eq!(limiter.check(&1, Limit::new(2, 2)), Ok(()));
        assert_eq!(limiter.bucket_count(), 2);
    }
}
