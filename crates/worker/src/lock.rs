//! Redis 分布式锁 adapter（`SET NX PX` + 唯一 token 校验释放）。

use async_trait::async_trait;

use cicada_infra::Cache;
use cicada_ports::{DistributedLock, LockError};

/// 基于 `cicada-infra::Cache` 的租约锁。token 由调用方生成，释放/续期都校验 token。
#[derive(Clone)]
pub struct RedisDistributedLock {
    cache: Cache,
}

impl RedisDistributedLock {
    pub fn new(cache: Cache) -> Self {
        Self { cache }
    }
}

fn backend(e: impl std::fmt::Display) -> LockError {
    LockError::Backend(e.to_string())
}

#[async_trait]
impl DistributedLock for RedisDistributedLock {
    async fn try_lock(&self, key: &str, token: &str, ttl_secs: u64) -> Result<bool, LockError> {
        self.cache
            .set_nx_ex(key, token, ttl_secs.max(1))
            .await
            .map_err(backend)
    }

    async fn renew(&self, key: &str, token: &str, ttl_secs: u64) -> Result<bool, LockError> {
        self.cache
            .compare_and_expire(key, token, ttl_secs.max(1))
            .await
            .map_err(backend)
    }

    async fn unlock(&self, key: &str, token: &str) -> Result<(), LockError> {
        self.cache
            .compare_and_delete(key, token)
            .await
            .map_err(backend)?;
        Ok(())
    }
}
