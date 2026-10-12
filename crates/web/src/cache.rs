//! 缓存监控（M3）：观测 Redis 与按域失效。
//!
//! 权限点 `monitor:cache:list`（观测）与 `monitor:cache:clear`（失效）是 cicada 新增的
//! 扩展命名空间——权限点清单的 `monitor:cache:view` 是 C 类菜单权限点，按可观测性约定
//! **不做 REST 守卫**（参考实现把它当成读写通吃的权限，读权限就能 `clearAll`，这里纠正）。
//!
//! 观测只到「键 + 类型 + TTL」：接口不提供按任意键取值，也不接受白名单外的键，
//! 避免管理接口变成会话/令牌的读取面。

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use cicada_macros::{audit, require_perm};
use cicada_monitor::cache::{
    CACHE_DOMAINS, CLEAR_MAX_KEYS, COUNT_CAP, DomainCount, count_keys, domain_for_name,
};
use cicada_ports::{CacheError, CacheInfo};

use crate::Permission;
use crate::extractor::RequestContextExt;
use crate::module::{Manifest, Module};
use crate::routes::Routes;
use crate::state::AppState;

use crate::problem::{ApiErr, BizCode, Problem};

/// 观测类权限点：看键空间概览与键列表。
pub const PERM_LIST: &str = "monitor:cache:list";
/// 破坏性权限点：清键 / 清域。与观测分开，避免「只读角色」能清缓存。
pub const PERM_CLEAR: &str = "monitor:cache:clear";

const PERMS: &[&str] = &[PERM_LIST, PERM_CLEAR];

fn err(status: StatusCode, msg: impl Into<String>) -> ApiErr {
    ApiErr::from_status(status, msg)
}

/// 缓存后端不可用按 503 处理（与就绪探针一致，属依赖故障而非请求错误）。
fn map_cache_err(e: CacheError) -> ApiErr {
    match e {
        CacheError::OutOfNamespace(key) => err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("键或域名不在允许的缓存命名空间内：{key}"),
        ),
        CacheError::Backend(msg) => {
            tracing::error!(error = %msg, "缓存监控访问 Redis 失败");
            Problem::new(BizCode::CacheUnavailable, "缓存后端不可用")
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CacheInfoVo {
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
    /// 命中率（0.0–1.0）。
    pub hit_rate: f64,
}

impl From<CacheInfo> for CacheInfoVo {
    fn from(i: CacheInfo) -> Self {
        Self {
            hit_rate: i.hit_rate(),
            version: i.version,
            role: i.role,
            uptime_secs: i.uptime_secs,
            connected_clients: i.connected_clients,
            used_memory: i.used_memory,
            used_memory_human: i.used_memory_human,
            maxmemory: i.maxmemory,
            db_keys: i.db_keys,
            hits: i.hits,
            misses: i.misses,
            expired_keys: i.expired_keys,
            evicted_keys: i.evicted_keys,
        }
    }
}

/// 一个缓存域的统计。
#[derive(Debug, Serialize, ToSchema)]
pub struct DomainStat {
    pub name: String,
    pub prefix: String,
    pub description: String,
    pub sensitive: bool,
    pub keys: u64,
    /// 键数达到统计上限，真实数量可能更多。
    pub truncated: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CacheOverview {
    pub info: CacheInfoVo,
    pub domains: Vec<DomainStat>,
    /// RFC3339 时间戳。
    pub scanned_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DomainList {
    pub items: Vec<DomainStat>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct KeyListQuery {
    /// 域名（`GET /monitor/cache/domains` 返回的 `name`）。
    pub domain: String,
    /// 上一页返回的游标；首页为 0/不传。
    pub cursor: Option<u64>,
    /// 本页期望键数，1–200。
    pub count: Option<u64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CacheKeyPage {
    pub domain: String,
    pub prefix: String,
    /// 下一页游标；`0` 表示已扫完。
    pub cursor: u64,
    pub has_more: bool,
    pub keys: Vec<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct KeyMetaQuery {
    /// 完整键名。
    pub key: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct KeyMetaVo {
    pub key: String,
    pub kind: String,
    /// 剩余 TTL 秒数；`-1` 永久，`-2` 不存在。
    pub ttl_secs: i64,
    pub domain: String,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ClearQuery {
    pub domain: String,
    /// 只清这一个键；省略则清空整个域。
    pub key: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ClearResult {
    pub domain: String,
    pub removed: u64,
}

async fn domain_stats(state: &AppState, cap: u64) -> Result<Vec<DomainStat>, ApiErr> {
    let mut out = Vec::with_capacity(CACHE_DOMAINS.len());
    for domain in CACHE_DOMAINS {
        let DomainCount { keys, truncated } =
            count_keys(state.cache_inspector.as_ref(), domain.prefix, cap)
                .await
                .map_err(map_cache_err)?;
        out.push(DomainStat {
            name: domain.name.to_string(),
            prefix: domain.prefix.to_string(),
            description: domain.description.to_string(),
            sensitive: domain.sensitive,
            keys,
            truncated,
        });
    }
    Ok(out)
}

fn require_domain(name: &str) -> Result<&'static cicada_monitor::cache::CacheDomain, ApiErr> {
    domain_for_name(name).ok_or_else(|| {
        err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("未知的缓存域：{name}"),
        )
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/monitor/cache/overview",
    responses((status = 200, description = "缓存概览", body = CacheOverview))
)]
#[require_perm("monitor:cache:list")]
pub async fn cache_overview(
    State(state): State<AppState>,
    _ctx: RequestContextExt,
) -> Result<Json<CacheOverview>, ApiErr> {
    let info = state
        .cache_inspector
        .info()
        .await
        .map_err(map_cache_err)?
        .into();
    let domains = domain_stats(&state, COUNT_CAP).await?;
    Ok(Json(CacheOverview {
        info,
        domains,
        scanned_at: Utc::now().to_rfc3339(),
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/monitor/cache/domains",
    responses((status = 200, description = "缓存域列表", body = DomainList))
)]
#[require_perm("monitor:cache:list")]
pub async fn list_domains(
    State(state): State<AppState>,
    _ctx: RequestContextExt,
) -> Result<Json<DomainList>, ApiErr> {
    Ok(Json(DomainList {
        items: domain_stats(&state, COUNT_CAP).await?,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/monitor/cache/keys",
    params(KeyListQuery),
    responses((status = 200, description = "某一域的键分页", body = CacheKeyPage))
)]
#[require_perm("monitor:cache:list")]
pub async fn list_keys(
    State(state): State<AppState>,
    _ctx: RequestContextExt,
    Query(q): Query<KeyListQuery>,
) -> Result<Json<CacheKeyPage>, ApiErr> {
    let domain = require_domain(&q.domain)?;
    let count = q.count.unwrap_or(50).clamp(1, 200);
    let page = state
        .cache_inspector
        .scan(&format!("{}*", domain.prefix), q.cursor.unwrap_or(0), count)
        .await
        .map_err(map_cache_err)?;
    Ok(Json(CacheKeyPage {
        domain: domain.name.to_string(),
        prefix: domain.prefix.to_string(),
        has_more: !page.done(),
        cursor: page.cursor,
        keys: page.keys,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/monitor/cache/key",
    params(KeyMetaQuery),
    responses((status = 200, description = "单键元信息", body = KeyMetaVo))
)]
#[require_perm("monitor:cache:list")]
pub async fn get_key_meta(
    State(state): State<AppState>,
    _ctx: RequestContextExt,
    Query(q): Query<KeyMetaQuery>,
) -> Result<Json<KeyMetaVo>, ApiErr> {
    let domain = cicada_monitor::cache::domain_for_key(&q.key).ok_or_else(|| {
        err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "键不在允许的缓存命名空间内",
        )
    })?;
    let meta = state
        .cache_inspector
        .key_meta(&q.key)
        .await
        .map_err(map_cache_err)?;
    Ok(Json(KeyMetaVo {
        key: meta.key,
        kind: meta.kind,
        ttl_secs: meta.ttl_secs,
        domain: domain.name.to_string(),
    }))
}

#[utoipa::path(
    delete,
    path = "/api/v1/monitor/cache/keys",
    params(ClearQuery),
    responses((status = 200, description = "已清理", body = ClearResult))
)]
#[require_perm("monitor:cache:clear")]
#[audit(module = "monitor", action = "cache:clear", kind = "delete")]
pub async fn clear_keys(
    State(state): State<AppState>,
    _ctx: RequestContextExt,
    Query(q): Query<ClearQuery>,
) -> Result<Json<ClearResult>, ApiErr> {
    let domain = require_domain(&q.domain)?;
    let (removed, scope) = match q.key.as_deref() {
        Some(key) => {
            if !key.starts_with(domain.prefix) {
                return Err(err(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("键 {key} 不属于域 {}", domain.name),
                ));
            }
            let removed = u64::from(
                state
                    .cache_inspector
                    .del(key)
                    .await
                    .map_err(map_cache_err)?,
            );
            (removed, key)
        }
        None => {
            let removed = state
                .cache_inspector
                .clear_prefix(domain.prefix, CLEAR_MAX_KEYS)
                .await
                .map_err(map_cache_err)?;
            (removed, domain.prefix)
        }
    };
    tracing::info!(
        domain = domain.name,
        scope = scope,
        removed,
        "缓存已按域清理"
    );
    Ok(Json(ClearResult {
        domain: domain.name.to_string(),
        removed,
    }))
}

/// 缓存监控模块：权限点与路由一并声明，装配期双向对账。
pub struct CacheModule;

impl Module for CacheModule {
    fn manifest(self: Box<Self>) -> Manifest {
        let perms = PERMS.iter().map(|p| Permission::new(*p));
        Manifest::new("cicada:monitor-cache")
            .perms(perms)
            .routes(routes)
    }
}

pub fn routes(_state: AppState) -> Routes {
    Routes::new()
        .route(
            "/api/v1/monitor/cache/overview",
            get(cache_overview),
            cache_overview__perms(),
        )
        .route(
            "/api/v1/monitor/cache/domains",
            get(list_domains),
            list_domains__perms(),
        )
        .route(
            "/api/v1/monitor/cache/keys",
            get(list_keys),
            list_keys__perms(),
        )
        .route(
            "/api/v1/monitor/cache/key",
            get(get_key_meta),
            get_key_meta__perms(),
        )
        .route_audited(
            "/api/v1/monitor/cache/keys",
            delete(clear_keys),
            clear_keys__perms(),
            clear_keys__audit(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perms_are_declared_once() {
        assert_eq!(PERMS.len(), 2);
        assert!(PERMS.contains(&PERM_LIST));
        assert!(PERMS.contains(&PERM_CLEAR));
    }
}
