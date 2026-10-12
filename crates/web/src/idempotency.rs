//! `Idempotency-Key` 防重复提交：写接口按需开启——客户端在 POST/PUT/PATCH
//! 上带 `Idempotency-Key` 头即受保护，未带头的请求完全不受影响。
//!
//! 语义：首个请求执行并把 `{status, body}` 结果存档（短 TTL）；同键的后续请求
//! 直接重放首个结果；首个请求仍在执行中时返回 409，避免同一写操作并发跑两遍。
//! 键的作用域含当前用户（或客户端 IP），不同客户端同名键互不干扰。
//! 存储故障时**放行**（fail-open）：这是重复提交的缓解措施，不是正确性闸门。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderName, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use cicada_infra::Cache;
use cicada_kernel::RequestContext;

use crate::problem::{BizCode, Problem};
use crate::state::AppState;
use rust_i18n::t;

/// 请求头名（IETF draft-idempotency 同名约定）。
pub const IDEMPOTENCY_HEADER: HeaderName = HeaderName::from_static("idempotency-key");

/// 执行中标记的 TTL（秒）：进程崩溃后同键请求最迟这么久可重试。
pub const PROC_TTL_SECS: u64 = 10;
/// 结果可重放的窗口（秒）。
pub const RESULT_TTL_SECS: u64 = 600;
/// 只缓存 JSON 响应，且体超过该上限不缓存（下载/导出等不走重放）。
const MAX_CACHED_BODY_BYTES: usize = 1024 * 1024;

/// 幂等存储的窄接口：Redis 生产、内存测试。
#[async_trait]
pub trait IdempotencyStore: Send + Sync {
    /// `SET NX`：占到执行位返回 `true`。
    async fn claim(&self, key: &str, ttl_secs: u64) -> Result<bool, String>;
    async fn get(&self, key: &str) -> Result<Option<String>, String>;
    async fn set_ex(&self, key: &str, value: &str, ttl_secs: u64) -> Result<(), String>;
}

/// Redis 实现。键前缀 `cicada:idem:` 已登记为缓存观测域。
pub struct RedisIdempotencyStore {
    cache: Cache,
}

impl RedisIdempotencyStore {
    pub fn new(cache: Cache) -> Self {
        Self { cache }
    }
}

#[async_trait]
impl IdempotencyStore for RedisIdempotencyStore {
    async fn claim(&self, key: &str, ttl_secs: u64) -> Result<bool, String> {
        self.cache
            .set_nx_ex(key, "1", ttl_secs)
            .await
            .map_err(|e| e.to_string())
    }

    async fn get(&self, key: &str) -> Result<Option<String>, String> {
        self.cache.get(key).await.map_err(|e| e.to_string())
    }

    async fn set_ex(&self, key: &str, value: &str, ttl_secs: u64) -> Result<(), String> {
        self.cache
            .set_ex(key, value, ttl_secs)
            .await
            .map_err(|e| e.to_string())
    }
}

/// 内存实现：单测 / 无 Redis。
#[derive(Default)]
pub struct InMemoryIdempotencyStore {
    inner: std::sync::Mutex<HashMap<String, (String, Instant)>>,
}

impl InMemoryIdempotencyStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl IdempotencyStore for InMemoryIdempotencyStore {
    async fn claim(&self, key: &str, ttl_secs: u64) -> Result<bool, String> {
        let mut guard = self.inner.lock().unwrap();
        let now = Instant::now();
        if let Some((_, until)) = guard.get(key)
            && *until > now
        {
            return Ok(false);
        }
        guard.insert(
            key.to_string(),
            ("1".to_string(), now + Duration::from_secs(ttl_secs)),
        );
        Ok(true)
    }

    async fn get(&self, key: &str) -> Result<Option<String>, String> {
        let guard = self.inner.lock().unwrap();
        Ok(guard
            .get(key)
            .filter(|(_, until)| *until > Instant::now())
            .map(|(v, _)| v.clone()))
    }

    async fn set_ex(&self, key: &str, value: &str, ttl_secs: u64) -> Result<(), String> {
        self.inner.lock().unwrap().insert(
            key.to_string(),
            (
                value.to_string(),
                Instant::now() + Duration::from_secs(ttl_secs),
            ),
        );
        Ok(())
    }
}

/// 幂等服务句柄（挂在 `AppState` 上供中间件使用）。
pub struct IdempotencyService {
    store: Arc<dyn IdempotencyStore>,
}

impl IdempotencyService {
    pub fn new(store: Arc<dyn IdempotencyStore>) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &Arc<dyn IdempotencyStore> {
        &self.store
    }
}

#[derive(Debug, Deserialize)]
struct CachedResult {
    status: u16,
    body: String,
}

/// 键作用域：已认证按用户，匿名按客户端 IP。
fn scope_of(req: &Request) -> String {
    match req.extensions().get::<RequestContext>() {
        Some(ctx) => match ctx.principal.as_ref() {
            Some(p) => format!("u{}", p.user_id.get()),
            None => format!("i{}", ctx.ip),
        },
        None => "anon".to_string(),
    }
}

fn valid_key(raw: &str) -> bool {
    (8..=128).contains(&raw.len())
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'.'))
}

fn result_key(scope: &str, raw: &str) -> String {
    format!("cicada:idem:{scope}:{raw}")
}

fn is_json(ct: Option<&axum::http::HeaderValue>) -> bool {
    ct.and_then(|v| v.to_str().ok())
        .is_some_and(|s| s.starts_with("application/json"))
}

fn replay(cached: &str) -> Response {
    match serde_json::from_str::<CachedResult>(cached) {
        Ok(r) => (
            StatusCode::from_u16(r.status).unwrap_or(StatusCode::OK),
            [(header::CONTENT_TYPE, "application/json")],
            r.body,
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "幂等缓存记录损坏，按未缓存处理");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn protected(method: &Method) -> bool {
    matches!(*method, Method::POST | Method::PUT | Method::PATCH)
}

/// tower 中间件：只拦截「写方法 + 合法 `Idempotency-Key` 头」的请求，其余原样放行。
pub async fn idempotency_middleware(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let raw = match req
        .headers()
        .get(IDEMPOTENCY_HEADER)
        .and_then(|v| v.to_str().ok())
    {
        Some(v) if protected(req.method()) && valid_key(v) => v.to_string(),
        _ => return next.run(req).await,
    };
    let rkey = result_key(&scope_of(&req), &raw);
    let pkey = format!("{rkey}:proc");

    // 1) 已有完成结果 → 重放首个请求的响应。
    match state.idempotency.store().get(&rkey).await {
        Ok(Some(cached)) => return replay(&cached),
        Ok(None) => {}
        Err(e) => {
            tracing::error!(error = %e, "幂等存储读取失败，放行本次请求（fail-open）");
            return next.run(req).await;
        }
    }

    // 2) 占执行位；占不到说明同键请求仍在跑。
    match state.idempotency.store().claim(&pkey, PROC_TTL_SECS).await {
        Ok(true) => {}
        Ok(false) => {
            let locale = req
                .extensions()
                .get::<cicada_kernel::RequestContext>()
                .map(|c| c.locale.clone())
                .unwrap_or_else(|| crate::i18n::negotiate_headers(req.headers()));
            // 同键请求互斥 = 键的唯一约束冲突（业务码目录 `CICADA_SYS_3002`，409）。
            return Problem::new(
                BizCode::SysUnique,
                t!("idempotency.in_progress", locale = locale.as_str()).into_owned(),
            )
            .into_response();
        }
        Err(e) => {
            tracing::error!(error = %e, "幂等存储写入失败，放行本次请求（fail-open）");
            return next.run(req).await;
        }
    }

    // 3) 执行业务并缓冲响应；2xx JSON 且尺寸合规才存档供重放。
    let resp = next.run(req).await;
    let status = resp.status();
    let ct = resp.headers().get(header::CONTENT_TYPE).cloned();
    let (parts, body) = resp.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_CACHED_BODY_BYTES * 2).await else {
        return Response::from_parts(parts, Body::empty());
    };
    if status.is_success()
        && bytes.len() <= MAX_CACHED_BODY_BYTES
        && is_json(ct.as_ref())
        && let Ok(text) = String::from_utf8(bytes.to_vec())
        && let Ok(record) = serde_json::to_string(&serde_json::json!({
            "status": status.as_u16(),
            "body": text,
        }))
        && let Err(e) = state
            .idempotency
            .store()
            .set_ex(&rkey, &record, RESULT_TTL_SECS)
            .await
    {
        tracing::warn!(error = %e, "幂等结果存档失败（不影响本次响应）");
    }
    Response::from_parts(parts, Body::from(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_validation_is_conservative() {
        assert!(valid_key("550e8400-e29b-41d4-a716-446655440000"));
        assert!(valid_key("order-42_create"));
        assert!(!valid_key("short")); // < 8
        assert!(!valid_key("has space"));
        assert!(!valid_key("注入\r\n键"));
        assert!(!valid_key(&"x".repeat(129)));
    }

    #[test]
    fn only_write_methods_protected() {
        assert!(protected(&Method::POST));
        assert!(protected(&Method::PUT));
        assert!(protected(&Method::PATCH));
        assert!(!protected(&Method::GET));
        assert!(!protected(&Method::DELETE));
    }

    #[tokio::test]
    async fn in_memory_store_claim_is_exclusive_until_expiry() {
        let store = InMemoryIdempotencyStore::new();
        assert!(store.claim("k", 60).await.unwrap());
        assert!(!store.claim("k", 60).await.unwrap());
        assert!(store.set_ex("r", "v", 60).await.is_ok());
        assert_eq!(store.get("r").await.unwrap().as_deref(), Some("v"));
        assert_eq!(store.get("missing").await.unwrap(), None);
    }
}
