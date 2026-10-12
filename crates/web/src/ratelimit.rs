//! 限流层（第 7 层）：请求一进来先按**单 IP** 令牌桶准入，鉴权后再按**租户**
//! 套餐的 `api_qps_limit`（多租户设计）准入。
//!
//! 两处调用的位置由「能不能拿到身份」决定：
//! - IP 维度：最外层中间件，未认证的洪水请求在鉴权前就被挡掉（省掉 JWT 解析与查库）；
//! - 租户维度：`auth_middleware` 里算完 `RequestContext` 之后，`KeyExtractor` 就是 `tenant_id`。
//!
//! 探测端点（`/healthz` `/readyz` `/metrics`）**不计入**限流，避免编排探针抖动。

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rust_i18n::t;

use cicada_infra::{Limit, RateLimitConfig, RateLimited, RateLimiter};
use cicada_kernel::TenantId;
use cicada_system::TenantQuota;

use crate::middleware::client_ip;
use crate::problem::{BizCode, Problem};
use crate::state::AppState;

/// 限流豁免路径（探针与抓取端点）。
const EXEMPT_PATHS: [&str; 3] = ["/healthz", "/readyz", "/metrics"];

/// 探测/抓取端点不计入任何维度的限流：编排探针抖动会直接导致重启。
pub(crate) fn is_exempt(path: &str) -> bool {
    EXEMPT_PATHS.contains(&path)
}

/// 租户 QPS 上限的进程内缓存条目：`None` 表示不限。
struct CachedLimit {
    limit: Option<Limit>,
    fetched_at: Instant,
}

/// 限流状态（`AppState.rate_limit`）：IP 与租户两个维度各一个令牌桶集合。
pub struct RateLimit {
    enabled: bool,
    ip_limit: Limit,
    cache_ttl: Duration,
    ip: RateLimiter<IpAddr>,
    tenant: RateLimiter<i64>,
    tenant_cache: RwLock<HashMap<i64, CachedLimit>>,
}

impl RateLimit {
    pub fn new(cfg: &RateLimitConfig) -> Self {
        Self {
            enabled: cfg.enabled,
            ip_limit: Limit::new(cfg.per_ip_qps, cfg.per_ip_burst),
            cache_ttl: Duration::from_secs(cfg.tenant_cache_ttl_secs),
            ip: RateLimiter::new(),
            tenant: RateLimiter::new(),
            tenant_cache: RwLock::new(HashMap::new()),
        }
    }

    /// 关闭限流（测试或明确不要限流的部署）。
    pub fn disabled() -> Self {
        Self::new(&RateLimitConfig {
            enabled: false,
            ..RateLimitConfig::default()
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// IP 维度准入。
    pub fn check_ip(&self, ip: IpAddr) -> Result<(), RateLimited> {
        if !self.enabled {
            return Ok(());
        }
        self.ip.check(&ip, self.ip_limit)
    }

    /// 租户维度准入：配额取套餐 `api_qps_limit`，读库失败按「不限」放行（可用性优先），
    /// 结果按 `tenant_cache_ttl_secs` 缓存，避免每请求查库。
    pub async fn check_tenant(
        &self,
        tenant: TenantId,
        quota: &TenantQuota,
    ) -> Result<(), RateLimited> {
        if !self.enabled {
            return Ok(());
        }
        let Some(limit) = self.tenant_limit(tenant, quota).await else {
            return Ok(());
        };
        self.tenant.check(&tenant.get(), limit)
    }

    async fn tenant_limit(&self, tenant: TenantId, quota: &TenantQuota) -> Option<Limit> {
        if let Some(hit) = self.cached_limit(tenant) {
            return hit;
        }
        let resolved = match quota.api_qps_limit(tenant).await {
            Ok(qps) => qps.map(|qps| Limit::new(qps, qps)),
            Err(e) => {
                tracing::warn!(error = %e, tenant = tenant.get(), "租户 QPS 上限读取失败，按不限处理");
                return None;
            }
        };
        self.tenant_cache
            .write()
            .expect("租户限流缓存锁中毒")
            .insert(
                tenant.get(),
                CachedLimit {
                    limit: resolved,
                    fetched_at: Instant::now(),
                },
            );
        resolved
    }

    fn cached_limit(&self, tenant: TenantId) -> Option<Option<Limit>> {
        let cache = self.tenant_cache.read().expect("租户限流缓存锁中毒");
        let entry = cache.get(&tenant.get())?;
        (entry.fetched_at.elapsed() < self.cache_ttl).then_some(entry.limit)
    }
}

/// 最外层中间件：IP 维度限流。
pub async fn rate_limit_middleware(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    if !state.rate_limit.is_enabled() || is_exempt(req.uri().path()) {
        return next.run(req).await;
    }
    if let Err(limited) = state.rate_limit.check_ip(client_ip(&req)) {
        let locale = crate::i18n::negotiate_headers(req.headers());
        return limited_response(limited, &locale);
    }
    next.run(req).await
}

/// 429（`CICADA_RATE_5001`）：`Retry-After` 是整数秒。
pub(crate) fn limited_response(limited: RateLimited, locale: &str) -> Response {
    (
        [(header::RETRY_AFTER, limited.retry_after_secs.to_string())],
        Problem::new(
            BizCode::RateLimited,
            t!("common.rate_limited", locale = locale),
        ),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn cfg(qps: u32, burst: u32) -> RateLimitConfig {
        RateLimitConfig {
            enabled: true,
            per_ip_qps: qps,
            per_ip_burst: burst,
            tenant_cache_ttl_secs: 60,
        }
    }

    #[test]
    fn disabled_passes_everything() {
        let limit = RateLimit::disabled();
        for _ in 0..100 {
            assert_eq!(
                limit.check_ip(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4))),
                Ok(())
            );
        }
    }

    #[test]
    fn ip_budget_is_per_address() {
        let limit = RateLimit::new(&cfg(1, 1));
        let a = IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));
        let b = IpAddr::V4(Ipv4Addr::new(5, 6, 7, 8));
        assert_eq!(limit.check_ip(a), Ok(()));
        assert_eq!(limit.check_ip(b), Ok(()));
        assert!(limit.check_ip(a).is_err());
    }

    #[test]
    fn zero_qps_means_unlimited() {
        let limit = RateLimit::new(&cfg(0, 0));
        for _ in 0..100 {
            assert_eq!(limit.check_ip(IpAddr::V4(Ipv4Addr::LOCALHOST)), Ok(()));
        }
    }

    #[test]
    fn probe_paths_are_exempt() {
        assert!(is_exempt("/healthz"));
        assert!(is_exempt("/readyz"));
        assert!(is_exempt("/metrics"));
        assert!(!is_exempt("/api/v1/system/users"));
    }
}
