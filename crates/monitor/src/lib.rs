//! cicada-monitor（L2 领域）
//!
//! 在线用户/缓存/服务指标/日志的只读管理视图。
//!
//! 在线用户的权威源是 Redis 会话（见 `cicada-auth::SessionStore`），HTTP 面在
//! `cicada-web` 的 `online` 模块。本 crate 还承载**缓存域白名单**（[`cache`]）与
//! Redis 观测 adapter，HTTP 面在 `cicada-web` 的 `cache` 模块；服务指标等其余只读
//! 视图后续按同一形状补进来。
//! **定时任务不在这里**：`sys_job` / `sys_job_log` 与其运行时归 `cicada-jobs`
//! 与 `cicada-worker`，HTTP 面在 `cicada-web` 的 `jobs` 模块。

pub mod cache;
pub mod server;

pub use cache::{
    CACHE_DOMAINS, CONFIG_DOMAIN_PREFIX, CacheDomain, DomainCount, RedisCacheInspector, count_keys,
    domain_for_key, domain_for_name,
};
