//! Redis 句柄：连接自动重连的异步客户端 + 会话/缓存原语。
//!
//! 深模块：把连接管理藏在 `Cache` 后面，调用方只看到字符串键值与会话集合操作。
//! 会话键布局与 TTL 语义由上层（`cicada-auth`）决定，本层只做无状态的读写。

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, RedisError};

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("Redis 连接失败：{0}")]
    Connect(#[source] RedisError),
    #[error("Redis 操作失败：{0}")]
    Op(#[source] RedisError),
}

/// 异步 Redis 句柄。内部是 `ConnectionManager`（断线自动重连），克隆廉价。
///
#[derive(Clone)]
pub struct Cache {
    conn: ConnectionManager,
}

impl Cache {
    pub async fn connect(url: &str) -> Result<Self, CacheError> {
        let client = redis::Client::open(url).map_err(CacheError::Connect)?;
        let conn = ConnectionManager::new(client)
            .await
            .map_err(CacheError::Connect)?;
        Ok(Self { conn })
    }

    pub async fn ping(&self) -> Result<(), CacheError> {
        let mut conn = self.conn.clone();
        redis::cmd("PING")
            .query_async::<String>(&mut conn)
            .await
            .map_err(CacheError::Op)?;
        Ok(())
    }

    pub async fn get(&self, key: &str) -> Result<Option<String>, CacheError> {
        let mut conn = self.conn.clone();
        conn.get(key).await.map_err(CacheError::Op)
    }

    pub async fn set_ex(&self, key: &str, value: &str, ttl_secs: u64) -> Result<(), CacheError> {
        let mut conn = self.conn.clone();
        conn.set_ex(key, value, ttl_secs)
            .await
            .map_err(CacheError::Op)
    }

    pub async fn del(&self, key: &str) -> Result<(), CacheError> {
        let mut conn = self.conn.clone();
        let _: () = conn.del(key).await.map_err(CacheError::Op)?;
        Ok(())
    }

    pub async fn expire(&self, key: &str, ttl_secs: u64) -> Result<(), CacheError> {
        let mut conn = self.conn.clone();
        let _: () = conn
            .expire(key, ttl_secs as i64)
            .await
            .map_err(CacheError::Op)?;
        Ok(())
    }

    /// 自增计数器（版本号等），返回自增后的值。
    pub async fn incr(&self, key: &str) -> Result<i64, CacheError> {
        let mut conn = self.conn.clone();
        conn.incr(key, 1).await.map_err(CacheError::Op)
    }

    // ---- 集合操作：按用户索引会话（`user_sessions:{user_id}`），供在线用户/强退使用 ----

    pub async fn sadd(&self, key: &str, member: &str) -> Result<(), CacheError> {
        let mut conn = self.conn.clone();
        let _: () = conn.sadd(key, member).await.map_err(CacheError::Op)?;
        Ok(())
    }

    pub async fn srem(&self, key: &str, member: &str) -> Result<(), CacheError> {
        let mut conn = self.conn.clone();
        let _: () = conn.srem(key, member).await.map_err(CacheError::Op)?;
        Ok(())
    }

    pub async fn smembers(&self, key: &str) -> Result<Vec<String>, CacheError> {
        let mut conn = self.conn.clone();
        conn.smembers(key).await.map_err(CacheError::Op)
    }

    pub async fn mget(&self, keys: &[String]) -> Result<Vec<Option<String>>, CacheError> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self.conn.clone();
        conn.mget(keys).await.map_err(CacheError::Op)
    }

    // ---- 分布式锁原语：`SET NX PX` + token 校验的释放/续期 ----

    /// 原子获取带 TTL 的锁：写入成功返回 `true`，键已存在返回 `false`。
    pub async fn set_nx_ex(
        &self,
        key: &str,
        value: &str,
        ttl_secs: u64,
    ) -> Result<bool, CacheError> {
        let mut conn = self.conn.clone();
        let res: Option<String> = redis::cmd("SET")
            .arg(key)
            .arg(value)
            .arg("NX")
            .arg("EX")
            .arg(ttl_secs)
            .query_async(&mut conn)
            .await
            .map_err(CacheError::Op)?;
        Ok(res.is_some())
    }

    /// 仅当键值等于 `expected` 时删除（Lua CAS），返回是否删除成功。
    pub async fn compare_and_delete(&self, key: &str, expected: &str) -> Result<bool, CacheError> {
        let script = redis::Script::new(
            "if redis.call('get', KEYS[1]) == ARGV[1] then \
             return redis.call('del', KEYS[1]) else return 0 end",
        );
        let mut conn = self.conn.clone();
        let n: i64 = script
            .key(key)
            .arg(expected)
            .invoke_async(&mut conn)
            .await
            .map_err(CacheError::Op)?;
        Ok(n == 1)
    }

    /// 仅当键值等于 `expected` 时续期（Lua CAS），返回是否续期成功。
    pub async fn compare_and_expire(
        &self,
        key: &str,
        expected: &str,
        ttl_secs: u64,
    ) -> Result<bool, CacheError> {
        let script = redis::Script::new(
            "if redis.call('get', KEYS[1]) == ARGV[1] then \
             return redis.call('expire', KEYS[1], ARGV[2]) else return 0 end",
        );
        let mut conn = self.conn.clone();
        let n: i64 = script
            .key(key)
            .arg(expected)
            .arg(ttl_secs)
            .invoke_async(&mut conn)
            .await
            .map_err(CacheError::Op)?;
        Ok(n == 1)
    }

    // ---- 观测原语（缓存监控）：只暴露只读的键空间视图 ----

    /// `INFO` 解析成键值表（`server`/`memory`/`stats`/`clients`/`keyspace` 段合并）。
    pub async fn info(&self) -> Result<std::collections::HashMap<String, String>, CacheError> {
        let mut conn = self.conn.clone();
        let raw: String = redis::cmd("INFO")
            .query_async(&mut conn)
            .await
            .map_err(CacheError::Op)?;
        let mut out = std::collections::HashMap::new();
        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once(':') {
                out.insert(k.to_string(), v.to_string());
            }
        }
        Ok(out)
    }

    /// 当前库键数（`DBSIZE`）。
    pub async fn dbsize(&self) -> Result<u64, CacheError> {
        let mut conn = self.conn.clone();
        redis::cmd("DBSIZE")
            .query_async(&mut conn)
            .await
            .map_err(CacheError::Op)
    }

    /// 按键前缀分页扫描（`SCAN`，绝不用 `KEYS`）：返回下一个游标与这一页的键。
    pub async fn scan(
        &self,
        pattern: &str,
        cursor: u64,
        count: u64,
    ) -> Result<(u64, Vec<String>), CacheError> {
        let mut conn = self.conn.clone();
        let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(pattern)
            .arg("COUNT")
            .arg(count.max(1))
            .query_async(&mut conn)
            .await
            .map_err(CacheError::Op)?;
        Ok((next, keys))
    }

    /// 键的类型（`TYPE`，不存在返回 `none`）。
    pub async fn key_type(&self, key: &str) -> Result<String, CacheError> {
        let mut conn = self.conn.clone();
        redis::cmd("TYPE")
            .arg(key)
            .query_async(&mut conn)
            .await
            .map_err(CacheError::Op)
    }

    /// 键的剩余 TTL 秒数（`TTL`：-1 永久，-2 不存在/已过期）。
    pub async fn ttl(&self, key: &str) -> Result<i64, CacheError> {
        let mut conn = self.conn.clone();
        redis::cmd("TTL")
            .arg(key)
            .query_async(&mut conn)
            .await
            .map_err(CacheError::Op)
    }
}

/// Redis 版的 [`cicada_ports::KeyValueStore`]（缓存设计）：把 `CICADA_INTERNAL_9003` 那一类
/// 「缓存层抖动」翻译成「没命中」，策略层（`KvCache`）因此不需要处理错误分支。
#[async_trait]
impl cicada_ports::KeyValueStore for Cache {
    async fn get(&self, key: &str) -> Option<String> {
        match Cache::get(self, key).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, key, "读缓存失败，按未命中处理");
                None
            }
        }
    }

    async fn set_ex(&self, key: &str, value: &str, ttl_secs: u64) {
        if let Err(e) = Cache::set_ex(self, key, value, ttl_secs).await {
            tracing::warn!(error = %e, key, "写缓存失败，本次结果只回给调用方");
        }
    }

    async fn del(&self, key: &str) {
        if let Err(e) = Cache::del(self, key).await {
            tracing::warn!(error = %e, key, "删除缓存键失败");
        }
    }

    async fn incr(&self, key: &str) -> i64 {
        match Cache::incr(self, key).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, key, "版本号自增失败，缓存将等到自然过期");
                0
            }
        }
    }
}
