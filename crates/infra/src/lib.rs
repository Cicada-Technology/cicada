//! cicada-infra（L1 基础设施）
//!
//! `config` / `telemetry` / `db`（连接池、事务）/ `cache` / `metrics` / `mq` 作为内部模块。
//! 按依赖规则规则：只依赖 `cicada-kernel`。

pub mod cache;
pub mod config;
pub mod db;
pub mod metrics;
pub mod password_policy;
pub mod ratelimit;
pub mod rls;
pub mod telemetry;

pub use cache::{Cache, CacheError};
pub use config::{
    AuditConfig, AuthConfig, CaptchaConfig, Config, ConfigError, CorsConfig, DbConfig, GenConfig,
    JobConfig, JwtConfig, JwtKey, MIN_JWT_SECRET_LEN, OAuthConfig, OAuthProviderConfig,
    OutboxConfig, PasswordConfig, RateLimitConfig, RedisConfig, Secret, ServerConfig,
    StorageConfig, TelemetryConfig, TenantConfig, WEAK_JWT_SAMPLE,
};
pub use db::{Db, DbError, PoolLive, ScopedDb};
pub use metrics::{MetricsError, install as install_metrics, render as render_metrics};
pub use password_policy::{password_problem_text, password_problems};

pub use ratelimit::{Limit, RateLimited, RateLimiter};
pub use telemetry::{TelemetryError, TracingGuard, init_tracing, init_tracing_with};
