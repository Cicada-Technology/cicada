//! 缓存观测端口（缓存监控）。
//!
//! 只做两件事：**观测**（键空间概览、按前缀分页扫描、单键元信息）与**按域失效**。
//! 两个 adapter：`cicada-monitor::RedisCacheInspector`（生产）与
//! [`InMemoryCacheInspector`]（测试夹具）。
//!
//! 接口刻意**不含**「按任意键读值」：Redis 里同时放着会话与令牌索引，
//! 逐键渲染值会把只读管理接口变成凭据泄漏面。观测只到「键 + 类型 + TTL」这一层。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("缓存后端不可用：{0}")]
    Backend(String),
    #[error("键不属于任何已登记的缓存命名空间：{0}")]
    OutOfNamespace(String),
}

/// 服务端概览：Redis 自身指标 + 当前库键数。
#[derive(Clone, Debug, Default)]
pub struct CacheInfo {
    pub version: String,
    pub role: String,
    pub uptime_secs: u64,
    pub connected_clients: u64,
    pub used_memory: u64,
    pub used_memory_human: String,
    pub maxmemory: u64,
    pub db_keys: u64,
    pub hits: u64,
    pub misses: u64,
    pub expired_keys: u64,
    pub evicted_keys: u64,
}

impl CacheInfo {
    /// 命中率（0.0–1.0）；无读写样本时为 0。
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

/// 一页扫描结果。`cursor == 0` 表示已扫完。
#[derive(Clone, Debug, Default)]
pub struct ScanPage {
    pub cursor: u64,
    pub keys: Vec<String>,
}

impl ScanPage {
    pub fn done(&self) -> bool {
        self.cursor == 0
    }
}

/// 单键元信息：类型、剩余 TTL（秒）、近似占用。（`ttl_secs`：-1 永久，-2 已过期）
#[derive(Clone, Debug)]
pub struct KeyMeta {
    pub key: String,
    pub kind: String,
    pub ttl_secs: i64,
}

/// 观测与按域失效的窄接口。
#[async_trait]
pub trait CacheInspector: Send + Sync {
    /// 服务端概览（对应 `INFO` + `DBSIZE`）。
    async fn info(&self) -> Result<CacheInfo, CacheError>;

    /// 按前缀模式分页扫描（对应 `SCAN`，禁止 `KEYS`）。
    async fn scan(&self, pattern: &str, cursor: u64, count: u64) -> Result<ScanPage, CacheError>;

    /// 单键元信息（对应 `TYPE` + `TTL`）。
    async fn key_meta(&self, key: &str) -> Result<KeyMeta, CacheError>;

    /// 删除单键，返回是否真的删除（不存在返回 `false`）。
    async fn del(&self, key: &str) -> Result<bool, CacheError>;

    /// 删除某前缀下的全部键（`SCAN` + `DEL` 循环），返回删除数。
    ///
    /// `max` 是安全上限，避免一次误操作扫空整个库。
    async fn clear_prefix(&self, prefix: &str, max: u64) -> Result<u64, CacheError>;

    /// 当前库键数（对应 `DBSIZE`）。
    async fn dbsize(&self) -> Result<u64, CacheError>;
}

/// 测试夹具：进程内键值 + TTL，语义与 Redis 对齐（扫描分页、TTL、前缀清除）。
///
/// 这里不校验命名空间白名单——白名单是 `cicada-monitor` 的领域知识，
/// 校验发生在 HTTP 面与生产 adapter（双重防线）。
#[derive(Default)]
pub struct InMemoryCacheInspector {
    inner: Mutex<HashMap<String, Entry>>,
}

#[derive(Clone)]
struct Entry {
    value: String,
    expires_at: Option<Instant>,
}

impl InMemoryCacheInspector {
    pub fn new() -> Self {
        Self::default()
    }

    /// 写入一个键。`ttl_secs = None` 表示永久。
    pub fn set(&self, key: &str, value: &str, ttl_secs: Option<u64>) {
        let expires_at = ttl_secs.map(|s| Instant::now() + Duration::from_secs(s));
        self.inner.lock().unwrap().insert(
            key.to_string(),
            Entry {
                value: value.to_string(),
                expires_at,
            },
        );
    }

    /// 读取一个键（夹具自用，生产端口不暴露）。
    pub fn get(&self, key: &str) -> Option<String> {
        let mut guard = self.inner.lock().unwrap();
        let expired = guard
            .get(key)
            .and_then(|e| e.expires_at)
            .is_some_and(|t| t <= Instant::now());
        if expired {
            guard.remove(key);
            return None;
        }
        guard.get(key).map(|e| e.value.clone())
    }

    fn live_keys(&self) -> Vec<String> {
        let now = Instant::now();
        let mut guard = self.inner.lock().unwrap();
        guard.retain(|_, e| e.expires_at.is_none_or(|t| t > now));
        let mut keys: Vec<String> = guard.keys().cloned().collect();
        keys.sort();
        keys
    }
}

/// 只支持尾随 `*` 的模式匹配——本项目所有扫描都是「前缀 + *」。
fn matches(pattern: &str, key: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => key.starts_with(prefix),
        None => key == pattern,
    }
}

#[async_trait]
impl CacheInspector for InMemoryCacheInspector {
    async fn info(&self) -> Result<CacheInfo, CacheError> {
        Ok(CacheInfo {
            version: "memory-fixture".to_string(),
            role: "standalone".to_string(),
            uptime_secs: 0,
            connected_clients: 1,
            used_memory: 0,
            used_memory_human: "0B".to_string(),
            maxmemory: 0,
            db_keys: self.live_keys().len() as u64,
            hits: 0,
            misses: 0,
            expired_keys: 0,
            evicted_keys: 0,
        })
    }

    async fn scan(&self, pattern: &str, cursor: u64, count: u64) -> Result<ScanPage, CacheError> {
        let all: Vec<String> = self
            .live_keys()
            .into_iter()
            .filter(|k| matches(pattern, k))
            .collect();
        let count = count.max(1);
        let start = (cursor as usize).min(all.len());
        let end = (start + count as usize).min(all.len());
        let keys = all[start..end].to_vec();
        let next = if end >= all.len() { 0 } else { end as u64 };
        Ok(ScanPage { cursor: next, keys })
    }

    async fn key_meta(&self, key: &str) -> Result<KeyMeta, CacheError> {
        let guard = self.inner.lock().unwrap();
        match guard.get(key) {
            None => Ok(KeyMeta {
                key: key.to_string(),
                kind: "none".to_string(),
                ttl_secs: -2,
            }),
            Some(entry) => Ok(KeyMeta {
                key: key.to_string(),
                kind: "string".to_string(),
                ttl_secs: entry
                    .expires_at
                    .map(|t| t.saturating_duration_since(Instant::now()).as_secs() as i64)
                    .unwrap_or(-1),
            }),
        }
    }

    async fn del(&self, key: &str) -> Result<bool, CacheError> {
        Ok(self.inner.lock().unwrap().remove(key).is_some())
    }

    async fn clear_prefix(&self, prefix: &str, max: u64) -> Result<u64, CacheError> {
        let targets: Vec<String> = self
            .live_keys()
            .into_iter()
            .filter(|k| k.starts_with(prefix))
            .take(max as usize)
            .collect();
        let mut guard = self.inner.lock().unwrap();
        let mut removed = 0;
        for key in targets {
            if guard.remove(&key).is_some() {
                removed += 1;
            }
        }
        Ok(removed)
    }

    async fn dbsize(&self) -> Result<u64, CacheError> {
        Ok(self.live_keys().len() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fixture_scans_by_prefix_with_pagination() {
        let cache = InMemoryCacheInspector::new();
        for i in 0..7 {
            cache.set(&format!("cicada:authz:1:{i}:v1"), "x", Some(60));
        }
        cache.set("session:abc", "y", Some(60));

        let first = cache.scan("cicada:authz:*", 0, 3).await.unwrap();
        assert_eq!(first.keys.len(), 3);
        assert!(!first.done());
        let second = cache.scan("cicada:authz:*", first.cursor, 3).await.unwrap();
        assert_eq!(second.keys.len(), 3);
        let third = cache
            .scan("cicada:authz:*", second.cursor, 3)
            .await
            .unwrap();
        assert_eq!(third.keys.len(), 1);
        assert!(third.done());
        assert_eq!(cache.dbsize().await.unwrap(), 8);
    }

    #[tokio::test]
    async fn fixture_honours_ttl_and_reports_meta() {
        let cache = InMemoryCacheInspector::new();
        cache.set("cicada:job:leader", "tok", None);
        cache.set("pwd_err_cnt:admin", "3", Some(60));

        let meta = cache.key_meta("cicada:job:leader").await.unwrap();
        assert_eq!(meta.kind, "string");
        assert_eq!(meta.ttl_secs, -1, "永久键的 TTL 是 -1");
        assert!(cache.key_meta("pwd_err_cnt:admin").await.unwrap().ttl_secs > 0);
        assert_eq!(cache.key_meta("missing").await.unwrap().ttl_secs, -2);
    }

    #[tokio::test]
    async fn fixture_clears_prefix_and_respects_max() {
        let cache = InMemoryCacheInspector::new();
        for i in 0..5 {
            cache.set(&format!("pwd_lock:{i}"), "1", Some(60));
        }
        cache.set("cicada:permver:1", "2", Some(60));

        assert_eq!(cache.clear_prefix("pwd_lock:", 3).await.unwrap(), 3);
        assert_eq!(cache.clear_prefix("pwd_lock:", 100).await.unwrap(), 2);
        assert_eq!(cache.dbsize().await.unwrap(), 1, "只剩 cicada:permver:1");
        assert!(cache.del("cicada:permver:1").await.unwrap());
        assert!(!cache.del("cicada:permver:1").await.unwrap());
    }

    #[test]
    fn hit_rate_is_zero_without_samples() {
        let info = CacheInfo::default();
        assert_eq!(info.hit_rate(), 0.0);
    }
}
