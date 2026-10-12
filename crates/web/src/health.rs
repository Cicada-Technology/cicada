//! 健康检查与指标。

use axum::Json;
use axum::extract::State;
use axum::http::header::{CONTENT_TYPE, HeaderValue};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;

use cicada_infra::metrics::{
    CACHE_DB_KEYS, CACHE_EVICTED, CACHE_EXPIRED, CACHE_HIT_RATIO, CACHE_HITS, CACHE_MISSES,
    CACHE_USED_MEMORY, DB_POOL, render,
};

use crate::problem::{BizCode, Problem};
use crate::state::AppState;

/// Prometheus 文本暴露的媒体类型（`version=0.0.4` 是抓取端约定写法）。
const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

#[derive(Serialize, ToSchema)]
pub struct Health {
    pub status: String,
}

/// 存活探针：不查依赖，进程在即 200。
#[utoipa::path(
    get,
    path = "/healthz",
    responses((status = 200, description = "存活", body = Health))
)]
pub async fn healthz() -> Json<Health> {
    Json(Health {
        status: "ok".to_string(),
    })
}

/// 就绪探针：查 PG 与 Redis。
///
/// 失败时按统一错误响应返回 `problem+json`，并区分是哪一个依赖（业务码目录的
/// `CICADA_INTERNAL_9002` / `CICADA_INTERNAL_9003`）：探针本身只需要 503 这个信号，
/// 但看响应的人需要知道该去重启哪个——把两者都压成「unavailable」等于让他去猜。
#[utoipa::path(
    get,
    path = "/readyz",
    responses(
        (status = 200, description = "就绪", body = Health),
        (status = 503, description = "依赖不可用（problem+json）", body = Problem)
    )
)]
pub async fn readyz(State(state): State<AppState>) -> Response {
    let db_ok = state.db.ping().await.is_ok();
    let redis_ok = state.sessions.ping().await.is_ok();
    let Some((code, what)) = readiness_failure(db_ok, redis_ok) else {
        return Json(Health {
            status: "ok".to_string(),
        })
        .into_response();
    };
    tracing::warn!(dependency = what, "就绪检查失败");
    Problem::new(code, format!("就绪检查失败：{what}")).into_response()
}

/// 依赖状态 → 该报哪个业务码。纯函数，两个分支都有单测（HTTP 侧只容易造出 Redis 故障）。
fn readiness_failure(db_ok: bool, redis_ok: bool) -> Option<(BizCode, &'static str)> {
    match (db_ok, redis_ok) {
        (true, true) => None,
        // 两个都坏时报 PG：它是更根本的依赖，`detail` 里仍写清两个都不可用。
        (false, false) => Some((BizCode::DbUnavailable, "PostgreSQL 与 Redis 均不可用")),
        (false, true) => Some((BizCode::DbUnavailable, "PostgreSQL 不可用")),
        (true, false) => Some((BizCode::CacheUnavailable, "Redis 不可用")),
    }
}

/// 指标端点：QPS 与延迟分位、按业务码的错误率、任务执行结果、
/// DB 连接池与缓存命中率。
///
/// 前几项在事件点记录（`layers::http_observability` / `Problem::new` / worker / 发件箱 relay），
/// 本函数只补两项「抓取时才有意义」的即时量：连接池状态与 Redis 计数。
/// 采样放在这里而不是后台轮询——它们只有被 scrape 时才需要，代价是一条 `INFO` 命令。
#[utoipa::path(
    get,
    path = "/metrics",
    responses((status = 200, description = "Prometheus 指标文本", body = String))
)]
pub async fn metrics(State(state): State<AppState>) -> Response {
    sample_pool(&state);
    sample_cache(&state).await;
    (
        [(
            CONTENT_TYPE,
            HeaderValue::from_static(PROMETHEUS_CONTENT_TYPE),
        )],
        render(),
    )
        .into_response()
}

fn sample_pool(state: &AppState) {
    let Some(live) = state.db.pool_live() else {
        return;
    };
    metrics::gauge!(DB_POOL, "state" => "total").set(live.total as f64);
    metrics::gauge!(DB_POOL, "state" => "idle").set(live.idle as f64);
    metrics::gauge!(DB_POOL, "state" => "max")
        .set(state.config.db.resolved_max_connections() as f64);
    metrics::gauge!(DB_POOL, "state" => "min").set(state.config.db.min_connections as f64);
}

async fn sample_cache(state: &AppState) {
    match state.cache_inspector.info().await {
        Ok(info) => {
            metrics::gauge!(CACHE_HIT_RATIO).set(info.hit_rate());
            metrics::gauge!(CACHE_HITS).set(info.hits as f64);
            metrics::gauge!(CACHE_MISSES).set(info.misses as f64);
            metrics::gauge!(CACHE_EXPIRED).set(info.expired_keys as f64);
            metrics::gauge!(CACHE_EVICTED).set(info.evicted_keys as f64);
            metrics::gauge!(CACHE_DB_KEYS).set(info.db_keys as f64);
            metrics::gauge!(CACHE_USED_MEMORY).set(info.used_memory as f64);
        }
        // 缓存不可用本身由 `/readyz` 与 `CICADA_INTERNAL_9003` 报出去，指标侧只留痕。
        Err(e) => tracing::debug!(error = %e, "缓存指标采样失败"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_dependencies_healthy_is_ready() {
        assert!(readiness_failure(true, true).is_none());
    }

    #[test]
    fn db_failure_reports_9002_even_when_redis_is_up() {
        let (code, what) = readiness_failure(false, true).unwrap();
        assert_eq!(code, BizCode::DbUnavailable);
        assert_eq!(code.code(), "CICADA_INTERNAL_9002");
        assert_eq!(what, "PostgreSQL 不可用");
        // 两个都坏时仍以 PG 为准，但描述要说全。
        let (both, what_both) = readiness_failure(false, false).unwrap();
        assert_eq!(both, BizCode::DbUnavailable);
        assert!(what_both.contains("Redis"), "{what_both}");
    }

    #[test]
    fn redis_failure_reports_9003() {
        let (code, _) = readiness_failure(true, false).unwrap();
        assert_eq!(code, BizCode::CacheUnavailable);
        assert_eq!(code.code(), "CICADA_INTERNAL_9003");
    }
}
