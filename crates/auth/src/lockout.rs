//! 登录失败计数与账号锁定：用 Redis 计数器记录连续失败，
//! 对应参考实现的 `maxRetryCount` / `lockTime`。
//!
//! 键布局：计数键 `pwd_err_cnt:{tenant}:{identifier}`（窗口 = 锁定时长），
//! 锁键 `pwd_lock:{tenant}:{identifier}`（值是解锁时间戳，TTL = 锁定时长）。
//! 登录成功与管理员解锁都清空这两个键。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use cicada_infra::Cache;

use crate::error::AuthError;

/// 密码失败锁定策略（配置 `auth.password`）。
#[derive(Clone, Copy, Debug)]
pub struct LockoutPolicy {
    /// 连续失败次数上限；`0` 表示不锁定。
    pub max_retry: u32,
    /// 锁定时长（秒），同时也是计数窗口。
    pub lock_secs: u64,
}

/// 指数退避的封顶（登录失败计数 + 指数退避锁定）。
///
/// 为什么封顶在 24 小时而不是翻倍到顶：退避要解决的是**慢速爆破**，
/// 而无上限的指数锁定等于给攻击者一个免费的「永久拒绝服务」按钮——
/// 对着某个账号打几次错误密码就能把它锁到第二天，那是可用性问题不是安全措施。
pub const MAX_LOCK_SECS: u64 = 24 * 60 * 60;

/// 第 `strike` 次锁定该锁多久：`lock_secs × 2^(strike-1)`，封顶 `MAX_LOCK_SECS`。
///
/// `strike` 从 1 起算，所以第一次锁定与配置值完全一致（不改变既有语义），
/// 只有「被锁过又继续试」才会升级。
pub fn backoff_secs(policy: LockoutPolicy, strike: u32) -> u64 {
    let factor = 1u64 << strike.saturating_sub(1).min(10);
    policy.lock_secs.saturating_mul(factor).min(MAX_LOCK_SECS)
}

impl LockoutPolicy {
    /// 不锁定。
    pub fn disabled() -> Self {
        Self {
            max_retry: 0,
            lock_secs: 0,
        }
    }
}

impl Default for LockoutPolicy {
    fn default() -> Self {
        Self {
            max_retry: 5,
            lock_secs: 10 * 60,
        }
    }
}

/// 一次失败后的状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LockState {
    pub failed: u32,
    pub locked: bool,
    pub retry_after_secs: u64,
}

/// 锁定键（登录与解锁共用，保证两端一致）。
pub fn lockout_key(tenant_id: i64, identifier: &str) -> String {
    format!(
        "{tenant_id}:{}",
        crate::login::canonical_identifier(identifier)
    )
}

/// 登录失败计数与锁定的窄接口。生产用 Redis，测试用内存实现。
#[async_trait]
pub trait LockoutStore: Send + Sync {
    /// 处于锁定中则返回剩余秒数。
    async fn locked_secs(&self, key: &str) -> Result<Option<u64>, AuthError>;
    /// 当前窗口内的连续失败次数（供验证码闸门读取）。
    async fn failures(&self, key: &str) -> Result<u32, AuthError>;
    /// 记一次失败并返回最新状态（达到阈值即锁定）。
    async fn record_failure(
        &self,
        key: &str,
        policy: LockoutPolicy,
    ) -> Result<LockState, AuthError>;
    /// 清空计数与锁定（登录成功 / 管理员解锁）。
    async fn clear(&self, key: &str) -> Result<(), AuthError>;
}

fn cnt_key(key: &str) -> String {
    format!("pwd_err_cnt:{key}")
}

fn lock_key(key: &str) -> String {
    format!("pwd_lock:{key}")
}

/// 「已经被锁过几次」的计数键。与失败计数分开：失败窗口随每次锁定清零，
/// 而退避必须跨窗口记住次数，否则第 100 次爆破和第 1 次拿到的惩罚一样轻。
fn strike_key(key: &str) -> String {
    format!("pwd_strike:{key}")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Redis 实现：计数与锁定都以键的 TTL 自动过期。
#[derive(Clone)]
pub struct RedisLockoutStore {
    cache: Cache,
}

impl RedisLockoutStore {
    pub fn new(cache: Cache) -> Self {
        Self { cache }
    }
}

#[async_trait]
impl LockoutStore for RedisLockoutStore {
    async fn locked_secs(&self, key: &str) -> Result<Option<u64>, AuthError> {
        let raw = self
            .cache
            .get(&lock_key(key))
            .await
            .map_err(|e| AuthError::Session(e.to_string()))?;
        let Some(raw) = raw else { return Ok(None) };
        let until: u64 = raw.parse().unwrap_or(0);
        let now = now_secs();
        Ok((until > now).then(|| until - now))
    }

    async fn failures(&self, key: &str) -> Result<u32, AuthError> {
        let raw = self
            .cache
            .get(&cnt_key(key))
            .await
            .map_err(|e| AuthError::Session(e.to_string()))?;
        Ok(raw.and_then(|v| v.parse().ok()).unwrap_or(0))
    }

    async fn record_failure(
        &self,
        key: &str,
        policy: LockoutPolicy,
    ) -> Result<LockState, AuthError> {
        if policy.max_retry == 0 || policy.lock_secs == 0 {
            return Ok(LockState {
                failed: 0,
                locked: false,
                retry_after_secs: 0,
            });
        }
        let cnt = self
            .cache
            .incr(&cnt_key(key))
            .await
            .map_err(|e| AuthError::Session(e.to_string()))? as u32;
        if cnt == 1 {
            self.cache
                .expire(&cnt_key(key), policy.lock_secs)
                .await
                .map_err(|e| AuthError::Session(e.to_string()))?;
        }
        if cnt >= policy.max_retry {
            // 先数「第几次被锁」，再按它算时长：Redis 的 INCR 自带原子性，
            // 并发失败不会把两次算成一次。strike 的 TTL 取封顶值，
            // 意思是「静置一天，退避回到起点」——而不是永久记仇。
            let strike = self
                .cache
                .incr(&strike_key(key))
                .await
                .map_err(|e| AuthError::Session(e.to_string()))?
                .max(1) as u32;
            let secs = backoff_secs(policy, strike);
            let until = now_secs() + secs;
            self.cache
                .set_ex(&lock_key(key), &until.to_string(), secs)
                .await
                .map_err(|e| AuthError::Session(e.to_string()))?;
            // 退避计数留一天：静置够久就当没发生过，避免把偶发的忘记密码变成累犯标签。
            self.cache
                .expire(&strike_key(key), MAX_LOCK_SECS)
                .await
                .map_err(|e| AuthError::Session(e.to_string()))?;
            self.cache
                .del(&cnt_key(key))
                .await
                .map_err(|e| AuthError::Session(e.to_string()))?;
            return Ok(LockState {
                failed: cnt,
                locked: true,
                retry_after_secs: secs,
            });
        }
        Ok(LockState {
            failed: cnt,
            locked: false,
            retry_after_secs: 0,
        })
    }

    async fn clear(&self, key: &str) -> Result<(), AuthError> {
        // 登录成功与管理员解锁都**连退避一起清零**：一次成功的密码证明这不是爆破，
        // 继续背着「上次被锁过五次」的惩罚是把运维误操作变成用户被歧视。
        for k in [cnt_key(key), lock_key(key), strike_key(key)] {
            self.cache
                .del(&k)
                .await
                .map_err(|e| AuthError::Session(e.to_string()))?;
        }
        Ok(())
    }
}

/// 内存实现：单测 / 无 Redis 环境用。
#[derive(Default)]
pub struct InMemoryLockoutStore {
    inner: Mutex<HashMap<String, Entry>>,
}

#[derive(Default)]
struct Entry {
    failed: u32,
    locked_until: Option<Instant>,
    window_start: Option<Instant>,
    /// 已被锁定的次数（退避的指数），与 `failed` 不同：不随每次锁定清零。
    strikes: u32,
}

impl InMemoryLockoutStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl LockoutStore for InMemoryLockoutStore {
    async fn locked_secs(&self, key: &str) -> Result<Option<u64>, AuthError> {
        let guard = self.inner.lock().unwrap();
        Ok(guard.get(key).and_then(|e| e.locked_until).and_then(|t| {
            let now = Instant::now();
            (t > now).then(|| t.duration_since(now).as_secs().max(1))
        }))
    }

    async fn failures(&self, key: &str) -> Result<u32, AuthError> {
        let guard = self.inner.lock().unwrap();
        Ok(guard.get(key).map(|e| e.failed).unwrap_or(0))
    }

    async fn record_failure(
        &self,
        key: &str,
        policy: LockoutPolicy,
    ) -> Result<LockState, AuthError> {
        if policy.max_retry == 0 || policy.lock_secs == 0 {
            return Ok(LockState {
                failed: 0,
                locked: false,
                retry_after_secs: 0,
            });
        }
        let now = Instant::now();
        let mut guard = self.inner.lock().unwrap();
        let entry = guard.entry(key.to_string()).or_default();
        // 计数窗口到期即重置。
        if let Some(start) = entry.window_start
            && now.duration_since(start).as_secs() >= policy.lock_secs
        {
            entry.failed = 0;
        }
        entry.window_start = Some(now);
        entry.failed += 1;
        if entry.failed >= policy.max_retry {
            entry.strikes += 1;
            let secs = backoff_secs(policy, entry.strikes);
            entry.locked_until = Some(now + std::time::Duration::from_secs(secs));
            entry.failed = 0;
            return Ok(LockState {
                failed: policy.max_retry,
                locked: true,
                retry_after_secs: secs,
            });
        }
        Ok(LockState {
            failed: entry.failed,
            locked: false,
            retry_after_secs: 0,
        })
    }

    async fn clear(&self, key: &str) -> Result<(), AuthError> {
        self.inner.lock().unwrap().remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 退避曲线：起点是配置值，每次翻倍，24 小时封顶。
    #[test]
    fn backoff_doubles_from_the_configured_length_and_caps() {
        let policy = LockoutPolicy {
            max_retry: 5,
            lock_secs: 600,
        };
        assert_eq!(
            backoff_secs(policy, 0),
            600,
            "strike=0 退化成一次，不该缩到 0"
        );
        assert_eq!(
            backoff_secs(policy, 1),
            600,
            "第一次锁定与配置一致（不改变既有语义）"
        );
        assert_eq!(backoff_secs(policy, 2), 1_200);
        assert_eq!(backoff_secs(policy, 3), 2_400);
        assert_eq!(backoff_secs(policy, 30), MAX_LOCK_SECS, "必须有上限");
        // 乘法溢出走 saturating：不能因为翻倍变成一个奇怪的负数/0。
        assert_eq!(
            backoff_secs(
                LockoutPolicy {
                    max_retry: 1,
                    lock_secs: u64::MAX
                },
                8
            ),
            MAX_LOCK_SECS
        );
    }

    /// 退避要跨窗口记住次数：只把失败计数清零、不记「第几次被锁」，
    /// 就等于对慢速爆破永远只罚一次。
    #[tokio::test]
    async fn repeated_locks_escalate_and_a_clear_resets_the_escalation() {
        let store = InMemoryLockoutStore::new();
        let policy = LockoutPolicy {
            max_retry: 2,
            lock_secs: 60,
        };
        let key = "1:backoff-user";

        assert!(!store.record_failure(key, policy).await.unwrap().locked);
        let first = store.record_failure(key, policy).await.unwrap();
        assert!(first.locked && first.retry_after_secs == 60, "{first:?}");

        // 锁定期内继续试：计数重新攒满，第二次锁定时长翻倍。
        assert!(!store.record_failure(key, policy).await.unwrap().locked);
        let second = store.record_failure(key, policy).await.unwrap();
        assert!(
            second.locked && second.retry_after_secs == 120,
            "{second:?}"
        );

        let third = {
            store.record_failure(key, policy).await.unwrap();
            store.record_failure(key, policy).await.unwrap()
        };
        assert!(third.locked && third.retry_after_secs == 240, "{third:?}");

        // 登录成功 / 管理员解锁：连退避一起清零，下次回到配置值。
        store.clear(key).await.unwrap();
        assert_eq!(store.failures(key).await.unwrap(), 0);
        store.record_failure(key, policy).await.unwrap();
        let after_reset = store.record_failure(key, policy).await.unwrap();
        assert!(
            after_reset.locked && after_reset.retry_after_secs == 60,
            "{after_reset:?}"
        );
    }

    #[tokio::test]
    async fn in_memory_locks_after_threshold_and_clears() {
        let store = InMemoryLockoutStore::new();
        let policy = LockoutPolicy {
            max_retry: 3,
            lock_secs: 60,
        };
        assert!(
            !store
                .record_failure("1:admin", policy)
                .await
                .unwrap()
                .locked
        );
        assert!(
            !store
                .record_failure("1:admin", policy)
                .await
                .unwrap()
                .locked
        );
        let third = store.record_failure("1:admin", policy).await.unwrap();
        assert!(third.locked);
        assert!(store.locked_secs("1:admin").await.unwrap().is_some());
        store.clear("1:admin").await.unwrap();
        assert!(store.locked_secs("1:admin").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn disabled_policy_never_locks() {
        let store = InMemoryLockoutStore::new();
        for _ in 0..10 {
            assert!(
                !store
                    .record_failure("1:admin", LockoutPolicy::disabled())
                    .await
                    .unwrap()
                    .locked
            );
        }
    }
}
