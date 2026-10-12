//! 指标（`metrics-exporter-prometheus`）。
//!
//! 用 `metrics` 门面记录、Prometheus 文本渲染导出；`/metrics` 端点在 `cicada-web`，
//! 本模块只负责「装 recorder + 出文本」，因此记录点可以散落在 web / worker / audit，
//! 而不必让它们反向依赖接入层。
//!
//! 面板要求（可观测性）：QPS 与延迟分位（`http_requests_total` /
//! `http_request_duration_seconds`）、DB 连接池（`db_pool_connections`）、
//! 缓存命中率（`cache_hit_ratio` 等）、任务执行结果（`job_runs_total`）、
//! 按业务码分组的错误率（`app_errors_total{code}`）。

use std::sync::OnceLock;

use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

/// HTTP 延迟分桶（秒）。覆盖 1ms 到 5s，超出上限的落 `+Inf`。
pub const HTTP_LATENCY_BUCKETS: &[f64] = &[
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0,
];

/// 请求计数：labels `method` / `route` / `status`。
pub const HTTP_REQUESTS: &str = "http_requests_total";
/// 请求延迟直方图：labels `method` / `route`。
pub const HTTP_LATENCY: &str = "http_request_duration_seconds";
/// 业务错误计数：labels `code`（业务码目录的 `CICADA_*`）。
pub const APP_ERRORS: &str = "app_errors_total";
/// DB 连接池：labels `state`（`active` / `idle` / `max` / `min`）。
pub const DB_POOL: &str = "db_pool_connections";
/// 缓存命中率与键空间计数。
pub const CACHE_HIT_RATIO: &str = "cache_hit_ratio";
pub const CACHE_HITS: &str = "cache_keyspace_hits";
pub const CACHE_MISSES: &str = "cache_keyspace_misses";
pub const CACHE_EXPIRED: &str = "cache_expired_keys";
pub const CACHE_EVICTED: &str = "cache_evicted_keys";
pub const CACHE_DB_KEYS: &str = "cache_db_keys";
pub const CACHE_USED_MEMORY: &str = "cache_used_memory_bytes";
/// 进程内那一级（moka）的命中/未命中：labels `domain`。
/// 加了 L1 之后 Redis 的 keyspace 命中数会**下降**（请求不再走到 Redis），
/// 没有这两个计数器，运维会把「变好」读成「缓存坏了」。
pub const CACHE_LOCAL_HITS: &str = "cache_local_hits_total";
pub const CACHE_LOCAL_MISSES: &str = "cache_local_misses_total";
/// 定时任务执行结果：labels `status`（`success` / `failed` / `skipped`）。
pub const JOB_RUNS: &str = "job_runs_total";
/// 发件箱投递结果：labels `kind`（`delivered` / `failed` / `abandoned`）。
pub const OUTBOX_DELIVERIES: &str = "outbox_deliveries_total";

static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

#[derive(Debug, thiserror::Error)]
pub enum MetricsError {
    #[error("指标 recorder 安装失败：{0}")]
    Install(String),
    #[error("指标 recorder 已安装，重复安装被忽略")]
    AlreadyInstalled,
}

/// 安装全局 recorder（进程内只应装一次；重复调用安全但会被忽略并报错）。
pub fn install() -> Result<(), MetricsError> {
    let handle = PrometheusBuilder::new()
        .set_buckets(HTTP_LATENCY_BUCKETS)
        .map_err(|e| MetricsError::Install(e.to_string()))?
        .install_recorder()
        .map_err(|e| MetricsError::Install(e.to_string()))?;
    if HANDLE.set(handle).is_err() {
        return Err(MetricsError::AlreadyInstalled);
    }
    metrics::gauge!("cicada_up").set(1.0);
    // 预置零值序列：任务与发件箱在第一次事件之前也要能被面板抓到，否则显示「无数据」。
    for status in ["success", "failed", "skipped"] {
        metrics::counter!(JOB_RUNS, "status" => status).absolute(0);
    }
    for kind in ["delivered", "failed", "abandoned"] {
        metrics::counter!(OUTBOX_DELIVERIES, "kind" => kind).absolute(0);
    }
    Ok(())
}

/// recorder 是否已安装（未安装时 `/metrics` 仍可访问，但只有说明行）。
pub fn installed() -> bool {
    HANDLE.get().is_some()
}

/// 渲染 Prometheus 文本 exposition。
pub fn render() -> String {
    match HANDLE.get() {
        Some(handle) => handle.render(),
        None => "# cicada metrics recorder not installed\n".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_are_sorted_and_cover_expected_range() {
        // Prometheus 要求分桶严格递增，否则 set_buckets 直接报错。
        assert!(HTTP_LATENCY_BUCKETS.windows(2).all(|w| w[0] < w[1]));
        assert!(HTTP_LATENCY_BUCKETS.first().unwrap() < &0.01);
        assert!(HTTP_LATENCY_BUCKETS.last().unwrap() >= &5.0);
    }

    #[test]
    fn render_before_install_is_a_comment_not_a_panic() {
        // 这个测试可能在 `install` 之后跑：两种情况都必须是可解析的 exposition 文本。
        let text = render();
        assert!(text.starts_with('#') || text.contains("# TYPE"));
    }
}
