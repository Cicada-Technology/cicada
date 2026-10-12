//! 会话存储：`session:{jti}` 权威在 Redis，`sys_user_online` 只是快照。
//!
//! `SessionStore` 是可替换的缝：生产用 `RedisSessionStore`，测试用
//! `InMemorySessionStore`。上层（登录/刷新/登出/中间件）只依赖 trait。

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use cicada_infra::Cache;

use crate::error::AuthError;

/// 会话记录：`session:{jti}` 的值。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    pub user_id: i64,
    pub tenant_id: i64,
    pub username: String,
    /// 刷新令牌哈希（不存明文）。
    pub refresh_hash: String,
    pub ip: String,
    pub user_agent: String,
    /// 登录时间（Unix 秒）。
    pub login_time: i64,
    /// 最近活跃时间（Unix 秒）。
    pub last_seen: i64,
}

impl Session {
    /// 恒定时间比较由哈希等长保证足够；此处只判相等。
    pub fn accepts_refresh(&self, refresh_token: &str) -> bool {
        self.refresh_hash == hash_refresh(refresh_token)
    }
}

/// 刷新令牌哈希：SHA-256 十六进制（库中只存哈希）。
pub fn hash_refresh(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// 会话存储的窄接口。
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// 新建/覆盖会话并续期用户索引。
    async fn create(&self, jti: &str, session: &Session, ttl_secs: u64) -> Result<(), AuthError>;
    async fn get(&self, jti: &str) -> Result<Option<Session>, AuthError>;
    /// 续期（更新 TTL），不改内容。
    async fn touch(&self, jti: &str, ttl_secs: u64) -> Result<(), AuthError>;
    /// 撤销单条会话（强制下线/登出）。
    async fn revoke(&self, jti: &str) -> Result<(), AuthError>;
    /// 撤销某用户的全部会话（改密即失效 / 判定刷新令牌重放）。
    async fn revoke_user(&self, user_id: i64) -> Result<(), AuthError>;
    /// 列出某用户的全部会话。
    async fn list_user(&self, user_id: i64) -> Result<Vec<(String, Session)>, AuthError>;

    /// 列出全部在线会话（管理视图；「在线用户以 Redis 为唯一权威源」）。
    async fn list_all(&self) -> Result<Vec<(String, Session)>, AuthError>;

    /// 会话数（最大会话数判定）。
    async fn count_user(&self, user_id: i64) -> Result<usize, AuthError> {
        Ok(self.list_user(user_id).await?.len())
    }

    /// 依赖探活（就绪检查）。
    async fn ping(&self) -> Result<(), AuthError> {
        Ok(())
    }
}

fn session_key(jti: &str) -> String {
    format!("session:{jti}")
}

fn user_key(user_id: i64) -> String {
    format!("user_sessions:{user_id}")
}

/// 全局在线会话索引（管理视图用；成员是 `jti`）。
const SESSIONS_INDEX: &str = "session_index";

/// Redis 实现：以 Redis 为唯一权威源。
#[derive(Clone)]
pub struct RedisSessionStore {
    cache: Cache,
}

impl RedisSessionStore {
    pub fn new(cache: Cache) -> Self {
        Self { cache }
    }
}

#[async_trait]
impl SessionStore for RedisSessionStore {
    async fn ping(&self) -> Result<(), AuthError> {
        self.cache
            .ping()
            .await
            .map_err(|e| AuthError::Session(e.to_string()))
    }

    async fn create(&self, jti: &str, session: &Session, ttl_secs: u64) -> Result<(), AuthError> {
        let json = serde_json::to_string(session).map_err(|e| AuthError::Session(e.to_string()))?;
        self.cache
            .set_ex(&session_key(jti), &json, ttl_secs)
            .await
            .map_err(|e| AuthError::Session(e.to_string()))?;
        let ukey = user_key(session.user_id);
        self.cache
            .sadd(&ukey, jti)
            .await
            .map_err(|e| AuthError::Session(e.to_string()))?;
        self.cache
            .expire(&ukey, ttl_secs)
            .await
            .map_err(|e| AuthError::Session(e.to_string()))?;
        self.cache
            .sadd(SESSIONS_INDEX, jti)
            .await
            .map_err(|e| AuthError::Session(e.to_string()))?;
        Ok(())
    }

    async fn get(&self, jti: &str) -> Result<Option<Session>, AuthError> {
        let Some(raw) = self
            .cache
            .get(&session_key(jti))
            .await
            .map_err(|e| AuthError::Session(e.to_string()))?
        else {
            return Ok(None);
        };
        let session = serde_json::from_str(&raw).map_err(|e| AuthError::Session(e.to_string()))?;
        Ok(Some(session))
    }

    async fn touch(&self, jti: &str, ttl_secs: u64) -> Result<(), AuthError> {
        self.cache
            .expire(&session_key(jti), ttl_secs)
            .await
            .map_err(|e| AuthError::Session(e.to_string()))
    }

    async fn revoke(&self, jti: &str) -> Result<(), AuthError> {
        if let Some(session) = self.get(jti).await? {
            self.cache
                .srem(&user_key(session.user_id), jti)
                .await
                .map_err(|e| AuthError::Session(e.to_string()))?;
        }
        self.cache
            .srem(SESSIONS_INDEX, jti)
            .await
            .map_err(|e| AuthError::Session(e.to_string()))?;
        self.cache
            .del(&session_key(jti))
            .await
            .map_err(|e| AuthError::Session(e.to_string()))
    }

    async fn revoke_user(&self, user_id: i64) -> Result<(), AuthError> {
        let ukey = user_key(user_id);
        let jtis = self
            .cache
            .smembers(&ukey)
            .await
            .map_err(|e| AuthError::Session(e.to_string()))?;
        for jti in &jtis {
            self.cache
                .del(&session_key(jti))
                .await
                .map_err(|e| AuthError::Session(e.to_string()))?;
            self.cache
                .srem(SESSIONS_INDEX, jti)
                .await
                .map_err(|e| AuthError::Session(e.to_string()))?;
        }
        self.cache
            .del(&ukey)
            .await
            .map_err(|e| AuthError::Session(e.to_string()))
    }

    async fn list_user(&self, user_id: i64) -> Result<Vec<(String, Session)>, AuthError> {
        let jtis = self
            .cache
            .smembers(&user_key(user_id))
            .await
            .map_err(|e| AuthError::Session(e.to_string()))?;
        if jtis.is_empty() {
            return Ok(Vec::new());
        }
        let keys: Vec<String> = jtis.iter().map(|jti| session_key(jti)).collect();
        let values = self
            .cache
            .mget(&keys)
            .await
            .map_err(|e| AuthError::Session(e.to_string()))?;
        let mut out = Vec::new();
        for (jti, value) in jtis.into_iter().zip(values) {
            if let Some(raw) = value
                && let Ok(session) = serde_json::from_str::<Session>(&raw)
            {
                out.push((jti, session));
            }
        }
        Ok(out)
    }

    async fn list_all(&self) -> Result<Vec<(String, Session)>, AuthError> {
        let jtis = self
            .cache
            .smembers(SESSIONS_INDEX)
            .await
            .map_err(|e| AuthError::Session(e.to_string()))?;
        if jtis.is_empty() {
            return Ok(Vec::new());
        }
        let keys: Vec<String> = jtis.iter().map(|jti| session_key(jti)).collect();
        let values = self
            .cache
            .mget(&keys)
            .await
            .map_err(|e| AuthError::Session(e.to_string()))?;
        let mut out = Vec::new();
        for (jti, value) in jtis.into_iter().zip(values) {
            match value {
                Some(raw) => {
                    if let Ok(session) = serde_json::from_str::<Session>(&raw) {
                        out.push((jti, session));
                    }
                }
                // 会话键已过期但索引成员残留：顺手清理，避免索引无限增长。
                None => {
                    self.cache
                        .srem(SESSIONS_INDEX, &jti)
                        .await
                        .map_err(|e| AuthError::Session(e.to_string()))?;
                }
            }
        }
        Ok(out)
    }
}

/// 内存实现：单测/无 Redis 环境用（不含 TTL 语义）。
#[derive(Default)]
pub struct InMemorySessionStore {
    inner: Mutex<HashMap<String, Session>>,
}

impl InMemorySessionStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn create(&self, jti: &str, session: &Session, _ttl_secs: u64) -> Result<(), AuthError> {
        self.inner
            .lock()
            .unwrap()
            .insert(jti.to_string(), session.clone());
        Ok(())
    }

    async fn get(&self, jti: &str) -> Result<Option<Session>, AuthError> {
        Ok(self.inner.lock().unwrap().get(jti).cloned())
    }

    async fn touch(&self, _jti: &str, _ttl_secs: u64) -> Result<(), AuthError> {
        Ok(())
    }

    async fn revoke(&self, jti: &str) -> Result<(), AuthError> {
        self.inner.lock().unwrap().remove(jti);
        Ok(())
    }

    async fn revoke_user(&self, user_id: i64) -> Result<(), AuthError> {
        self.inner
            .lock()
            .unwrap()
            .retain(|_, s| s.user_id != user_id);
        Ok(())
    }

    async fn list_user(&self, user_id: i64) -> Result<Vec<(String, Session)>, AuthError> {
        let guard = self.inner.lock().unwrap();
        Ok(guard
            .iter()
            .filter(|(_, s)| s.user_id == user_id)
            .map(|(jti, s)| (jti.clone(), s.clone()))
            .collect())
    }

    async fn list_all(&self) -> Result<Vec<(String, Session)>, AuthError> {
        let guard = self.inner.lock().unwrap();
        Ok(guard
            .iter()
            .map(|(jti, s)| (jti.clone(), s.clone()))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(user_id: i64, refresh: &str) -> Session {
        Session {
            user_id,
            tenant_id: 1,
            username: "admin".to_string(),
            refresh_hash: hash_refresh(refresh),
            ip: "127.0.0.1".to_string(),
            user_agent: "test".to_string(),
            login_time: 0,
            last_seen: 0,
        }
    }

    #[tokio::test]
    async fn in_memory_create_get_revoke() {
        let store = InMemorySessionStore::new();
        let s = session(1, "r1");
        store.create("jti-1", &s, 60).await.unwrap();
        assert_eq!(store.get("jti-1").await.unwrap().as_ref(), Some(&s));
        assert!(
            store
                .get("jti-1")
                .await
                .unwrap()
                .unwrap()
                .accepts_refresh("r1")
        );
        store.revoke("jti-1").await.unwrap();
        assert!(store.get("jti-1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn in_memory_revoke_user_clears_all() {
        let store = InMemorySessionStore::new();
        store.create("a", &session(7, "r"), 60).await.unwrap();
        store.create("b", &session(7, "r"), 60).await.unwrap();
        store.create("c", &session(8, "r"), 60).await.unwrap();
        store.revoke_user(7).await.unwrap();
        assert_eq!(store.count_user(7).await.unwrap(), 0);
        assert_eq!(store.count_user(8).await.unwrap(), 1);
    }

    #[test]
    fn hash_is_stable_and_not_plaintext() {
        let h = hash_refresh("r1");
        assert_eq!(h, hash_refresh("r1"));
        assert_ne!(h, "r1");
        assert_eq!(h.len(), 64);
    }
}
