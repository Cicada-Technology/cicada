//! 分布式锁端口：定时任务选主与「禁止并发」互斥。
//!
//! 两个 adapter：`RedisDistributedLock`（生产，`SET NX PX` + 唯一 token 校验释放）
//! 与 [`InMemoryDistributedLock`]（测试夹具）。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("分布式锁后端失败：{0}")]
    Backend(String),
}

/// 带唯一 token 的租约锁：释放与续期都必须校验 token，避免误删他人持锁。
#[async_trait]
pub trait DistributedLock: Send + Sync {
    /// 尝试获取锁：成功返回 `true`，已被占用返回 `false`。
    async fn try_lock(&self, key: &str, token: &str, ttl_secs: u64) -> Result<bool, LockError>;
    /// 续期：仍持有该 token 才续，成功返回 `true`。
    async fn renew(&self, key: &str, token: &str, ttl_secs: u64) -> Result<bool, LockError>;
    /// 释放：仅释放由该 token 持有的锁。
    async fn unlock(&self, key: &str, token: &str) -> Result<(), LockError>;
}

/// 测试夹具：进程内租约锁，语义与 Redis 实现一致（含 TTL 过期）。
#[derive(Default)]
pub struct InMemoryDistributedLock {
    inner: Mutex<HashMap<String, (String, Instant)>>,
}

impl InMemoryDistributedLock {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl DistributedLock for InMemoryDistributedLock {
    async fn try_lock(&self, key: &str, token: &str, ttl_secs: u64) -> Result<bool, LockError> {
        let mut map = self.inner.lock().expect("lock poisoned");
        let now = Instant::now();
        match map.get(key) {
            Some((_, exp)) if *exp > now => Ok(false),
            _ => {
                map.insert(
                    key.to_string(),
                    (token.to_string(), now + Duration::from_secs(ttl_secs)),
                );
                Ok(true)
            }
        }
    }

    async fn renew(&self, key: &str, token: &str, ttl_secs: u64) -> Result<bool, LockError> {
        let mut map = self.inner.lock().expect("lock poisoned");
        match map.get(key) {
            Some((held, exp)) if held == token && *exp > Instant::now() => {
                map.insert(
                    key.to_string(),
                    (
                        token.to_string(),
                        Instant::now() + Duration::from_secs(ttl_secs),
                    ),
                );
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn unlock(&self, key: &str, token: &str) -> Result<(), LockError> {
        let mut map = self.inner.lock().expect("lock poisoned");
        if let Some((held, _)) = map.get(key)
            && held == token
        {
            map.remove(key);
        }
        Ok(())
    }
}

impl InMemoryDistributedLock {
    /// 仅测试可读的探针：key 是否仍被持有。
    pub fn is_held(&self, key: &str) -> bool {
        let map = self.inner.lock().expect("lock poisoned");
        map.get(key).is_some_and(|(_, exp)| *exp > Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn exclusivity_and_token_checked_release() {
        let lock = InMemoryDistributedLock::new();
        assert!(lock.try_lock("k", "t1", 60).await.unwrap());
        // 已被占用。
        assert!(!lock.try_lock("k", "t2", 60).await.unwrap());
        // 错误 token 释放无效。
        lock.unlock("k", "t2").await.unwrap();
        assert!(lock.is_held("k"));
        // 正确 token 释放。
        lock.unlock("k", "t1").await.unwrap();
        assert!(!lock.is_held("k"));
        assert!(lock.try_lock("k", "t3", 60).await.unwrap());
    }

    #[tokio::test]
    async fn ttl_expiry_frees_lock() {
        let lock = InMemoryDistributedLock::new();
        assert!(lock.try_lock("k", "t1", 0).await.unwrap());
        // ttl = 0 表示立即过期。
        assert!(!lock.is_held("k"));
        assert!(lock.try_lock("k", "t2", 60).await.unwrap());
    }
}
