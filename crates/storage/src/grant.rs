//! 一次性下载凭证（本地存储走一次性 token）。
//!
//! 为什么需要它：`local` / `memory` 驱动没有预签名能力，客户端要拿一个能直接放进
//! `<img src>` 或交给下载器的链接，只能由服务端自己签发。凭证必须
//! **短时 + 单次**：`sys_file` 的路径本身不可猜，但链接会进浏览器历史、代理日志与
//! 用户复制粘贴，长期有效等于把「鉴权后下载」退化成「知道链接就能下载」。
//!
//! 存储选择跟着部署的「应用实例不保存状态」：生产用 Redis（多实例共享、TTL 由
//! Redis 兜底），单进程内存实现只服务测试与本地开发。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use cicada_infra::Cache;

use crate::StorageError;

/// 凭证签发时绑定的事实：文件行 + 存储路径。
///
/// 两个都存：`file_id` 让接入层回到 `sys_file` 取元数据（原始名、MIME），
/// `path` 用来核对那一行的内容确实还是当初授权的对象（行被改写或重建后旧凭证作废）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadGrant {
    pub file_id: i64,
    pub path: String,
}

/// 短时下载凭证的存储端口。
#[async_trait]
pub trait DownloadGrantStore: Send + Sync {
    /// 签发：返回不可猜测的 token；`ttl` 到点即失效。
    async fn issue(&self, grant: &DownloadGrant, ttl: Duration) -> Result<String, StorageError>;
    /// 单次消费：拿到即作废。返回 `Ok(None)` 表示无效、过期或已被用过。
    async fn redeem(&self, token: &str) -> Result<Option<DownloadGrant>, StorageError>;
}

/// Redis 键前缀：必须登记进 `cicada-monitor::cache::CACHE_DOMAINS`（缓存设计），
/// 否则这个域在缓存监控里既看不见也清不掉。
pub const GRANT_KEY_PREFIX: &str = "cicada:dl:";

fn grant_key(token: &str) -> String {
    format!("{GRANT_KEY_PREFIX}{}", token.replace([':', '/'], "_"))
}

/// 生产实现：值存 JSON，TTL 交给 Redis，消费用「比对后删除」保证单次。
pub struct RedisGrantStore {
    cache: Cache,
}

impl RedisGrantStore {
    pub fn new(cache: Cache) -> Self {
        Self { cache }
    }
}

#[async_trait]
impl DownloadGrantStore for RedisGrantStore {
    async fn issue(&self, grant: &DownloadGrant, ttl: Duration) -> Result<String, StorageError> {
        let token = uuid::Uuid::new_v4().simple().to_string();
        let value =
            serde_json::to_string(grant).map_err(|e| StorageError::Config(e.to_string()))?;
        let secs = ttl.as_secs().max(1);
        self.cache
            .set_ex(&grant_key(&token), &value, secs)
            .await
            .map_err(|e| StorageError::Io(e.to_string()))?;
        Ok(token)
    }

    async fn redeem(&self, token: &str) -> Result<Option<DownloadGrant>, StorageError> {
        let key = grant_key(token);
        let Some(raw) = self
            .cache
            .get(&key)
            .await
            .map_err(|e| StorageError::Io(e.to_string()))?
        else {
            return Ok(None);
        };
        // 「取到 → 按原值删」而不是直接 `DEL`：并发下只有一个请求能删成功，
        // 删失败的那个拿到 None，链接就不会被用两次。
        let taken = self
            .cache
            .compare_and_delete(&key, &raw)
            .await
            .map_err(|e| StorageError::Io(e.to_string()))?;
        if !taken {
            return Ok(None);
        }
        let grant: DownloadGrant =
            serde_json::from_str(&raw).map_err(|e| StorageError::Config(e.to_string()))?;
        Ok(Some(grant))
    }
}

/// 单进程实现：测试与本地开发用。过期条目在 `redeem` 时顺手清理，
/// 不做后台回收（键本来就短命）。
#[derive(Default)]
pub struct InMemoryGrantStore {
    inner: Mutex<HashMap<String, (DownloadGrant, Instant)>>,
}

impl InMemoryGrantStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl DownloadGrantStore for InMemoryGrantStore {
    async fn issue(&self, grant: &DownloadGrant, ttl: Duration) -> Result<String, StorageError> {
        let token = uuid::Uuid::new_v4().simple().to_string();
        let mut map = self.inner.lock().expect("凭证表不该中毒");
        let before = map.len();
        map.retain(|_, (_, until)| *until > Instant::now());
        if map.len() < before {
            tracing::debug!(dropped = before - map.len(), "清理已过期的下载凭证");
        }
        map.insert(token.clone(), (grant.clone(), Instant::now() + ttl));
        Ok(token)
    }

    async fn redeem(&self, token: &str) -> Result<Option<DownloadGrant>, StorageError> {
        let entry = self.inner.lock().expect("凭证表不该中毒").remove(token);
        Ok(entry
            .filter(|(_, until)| *until > Instant::now())
            .map(|(grant, _)| grant))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(file_id: i64) -> DownloadGrant {
        DownloadGrant {
            file_id,
            path: format!("1/{file_id}.png"),
        }
    }

    /// 「一次性」是这个凭证的全部意义：链接会进浏览器历史与代理日志，
    /// 能重复使用的短时 URL 只是把鉴权换成了「知道链接即可下载」。
    #[tokio::test]
    async fn a_token_works_exactly_once() {
        let store = InMemoryGrantStore::new();
        let token = store
            .issue(&grant(7), Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(token.len(), 32, "token 应当是不可猜的 uuid v4");
        let back = store.redeem(&token).await.unwrap();
        assert_eq!(back, Some(grant(7)));
        assert_eq!(
            store.redeem(&token).await.unwrap(),
            None,
            "同一凭证不能用两次"
        );
        assert_eq!(store.redeem("deadbeef").await.unwrap(), None);
    }

    #[tokio::test]
    async fn tokens_expire_on_their_own_ttl() {
        let store = InMemoryGrantStore::new();
        let stale = store.issue(&grant(1), Duration::ZERO).await.unwrap();
        assert_eq!(
            store.redeem(&stale).await.unwrap(),
            None,
            "过期凭证必须失效"
        );
        let fresh = store
            .issue(&grant(2), Duration::from_secs(30))
            .await
            .unwrap();
        assert!(store.redeem(&fresh).await.unwrap().is_some());
    }

    /// 签发互不相关的两张凭证，消费其一不应影响其二。
    #[tokio::test]
    async fn grants_are_independent() {
        let store = InMemoryGrantStore::new();
        let a = store
            .issue(&grant(1), Duration::from_secs(60))
            .await
            .unwrap();
        let b = store
            .issue(&grant(2), Duration::from_secs(60))
            .await
            .unwrap();
        assert_ne!(a, b);
        assert_eq!(store.redeem(&b).await.unwrap(), Some(grant(2)));
        assert_eq!(store.redeem(&a).await.unwrap(), Some(grant(1)));
    }

    #[test]
    fn redis_keys_live_in_the_registered_domain() {
        // 缓存设计：域前缀必须与 monitor 的 CACHE_DOMAINS 对白名单一致，且 token 里的
        // 分隔符要被抹掉——否则 `{token}` 本身就能构造出越界的键。
        assert_eq!(grant_key("a/b:c"), "cicada:dl:a_b_c");
        assert!(GRANT_KEY_PREFIX.starts_with("cicada:"));
    }
}
