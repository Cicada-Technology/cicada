# cicada-infra

L1 基础设施：配置、可观测、数据库、缓存、限流与 RLS 开关。

## 职责

- `config`：`config/default.yaml` + 环境变量覆盖，反序列化成带默认值的结构体；
  启动时校验（非法值直接拒绝启动，不在运行时兜圈子）。
- `password_policy`：口令规则（长度按**字符**计 + 大小写与数字各至少一个）。
  放这里而不是 `cicada-auth`：`migrations`（`init_admin` / `tenant init` / `admin reset`）要用同一份规则，
  而数据层不许反向依赖认证域；阈值本就配在 `config::PasswordConfig`，规则与阈值分家迟早会漏掉一半。
- `telemetry`：`init_tracing` / `init_tracing_with` 初始化 tracing subscriber（stdout 恒开），
  配了 `telemetry.otlp_endpoint` 时在同一 subscriber 上挂 OTLP layer。
  返回的 `TracingGuard` 要持有到进程结束，drop 时 force_flush + shutdown。
  filter 里会自动补一条 `sea_orm=trace`，让 DB 语句成为 span 子节点而不增加日志噪声。
- `metrics`：指标注册表——装 `metrics-exporter-prometheus` 的全局 recorder、
  定义指标名与 HTTP 延迟分桶、`render()` 出 Prometheus 文本。记录点分散在 web / worker /
  audit，都只依赖 `metrics` 门面，不反向依赖接入层。
- `db`：SeaORM 连接池（`Db`）、事务闭包、`ScopedDb`；连接参数来自 `config.db`。
- `cache`：`Cache` trait + Redis 实现，会话、权限缓存、幂等键都走这把缝。
- `ratelimit`：与 HTTP 无关的令牌桶原语（`Limit` / `RateLimiter` / `RateLimited`），
  编排在 `cicada-web::ratelimit`（ADR 0005）。
- `rls`：PostgreSQL 行级安全的按表开关与白名单校验（ADR 0001）。

## 公开 API

`Db` / `DbError` / `ScopedDb`、`Cache` / `CacheError`、`Config`（及 `config::*` 子结构）、
`Limit` / `RateLimiter` / `RateLimited`、`rls::*`、`init_tracing` / `init_tracing_with` / `TracingGuard`、
`metrics::{install, render, MetricsError, DB_POOL, HTTP_REQUESTS, ...}`、`Db::pool_live` / `PoolLive`。

## 依赖

`cicada-kernel` / `sea-orm` / `redis` / `tracing` / `governor` / `serde_yaml` /
`metrics` / `metrics-exporter-prometheus` / `opentelemetry*` / `tracing-opentelemetry`。

## 不做什么

- 不含业务规则：这里只提供能力，领域逻辑在 L2/L3。
- 不定义 HTTP 路由与中间件：那是 `cicada-web`。
- 除 `cicada-kernel` 外不依赖任何 cicada crate（依赖规则）。
