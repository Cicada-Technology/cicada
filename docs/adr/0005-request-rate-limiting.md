# ADR 0005：限流用进程内令牌桶，按 IP 与租户两个维度准入

- 日期：2026-10-10
- 状态：已采纳

## 背景

设计文档 §3.4 把 `rate_limit` 定为请求生命周期的第 7 层（`governor` 令牌桶，「按 IP/用户/接口维度」），
§6.4 又给租户套餐留了 `api_qps_limit`。此前两者都没落地：`api_qps_limit` 只落库与回显
（ADR 0004 的代价里记了这笔账），请求路径上没有任何准入控制。

难点在于**身份的顺序**：IP 在鉴权前就能拿到（要挡住未认证的洪水请求），
租户要等鉴权算完 `RequestContext` 才有；而按租户读取配额要查库，不能每请求一次。

## 结论

- 原语放在 L1：`cicada_infra::ratelimit::{Limit, RateLimiter, RateLimited}`，基于
  `governor` 的 GCRA/令牌桶，**只做进程内、按 key 的检查**，不认识 HTTP、不查库，可单测。
  每个不同配额（`Limit`）对应一个 keyed 限流器，配额种类很少（全局默认 + 若干套餐档位）。
- 编排放在 L4：`cicada_web::ratelimit::RateLimit`（`AppState.rate_limit`），两个调用点：
  - **IP 维度**：最外层中间件 `rate_limit_middleware`，在鉴权之前，挡住未认证请求；
  - **租户维度**：`auth_middleware` 里算完上下文之后，配额取 `TenantQuota::api_qps_limit`
    （套餐 `api_qps_limit <= 0` 视为不限），按 `tenant_cache_ttl_secs`（默认 60s）本地缓存，
    避免每请求查库。
- 超限返回 **429** + `Retry-After`（整数秒），对应附录 C 的 `CICADA_RATE_5001`；
  探针端点 `/healthz` `/readyz` `/metrics` 两个维度都豁免（探针抖动会导致编排重启）。
- 配额在启动期由配置固定：`rate_limit.enabled / per_ip_qps / per_ip_burst / tenant_cache_ttl_secs`
  （默认开、单 IP 200 QPS、突发 400）。`per_ip_qps: 0` 表示不限。
- 客户端 IP 取用顺序：`ConnectInfo` 的 TCP 对端 → `X-Forwarded-For` 首个地址 → `X-Real-IP`
  → 未指定地址占位。组装根用 `into_make_service_with_connect_info` 挂上 `ConnectInfo`。

## 代价

- 计数在**进程内**：多副本部署时每个实例各有一份配额，实际放行量 ≈ 副本数 × 配置值。
  需要全局精确配额时把 `RateLimiter` 换成 Redis 实现（接口不变，`Limit` 已经与存储解耦）。
- 走 `X-Forwarded-For` 意味着该头可信：若直连公网且未挂 `ConnectInfo`，客户端可伪造头绕过
  IP 限流。部署形态（设计文档 §12）假定在可信反代之后，由网关改写该头；否则应改为只信 TCP 对端。
- 未接「用户维度」与「接口维度」：用户维度没有配额来源（`sys_user` 无 QPS 字段），
  接口维度需要给每个路由标注配额（收益低于管理后台的量级），二者都留待有真实需求时再加。
- 租户配额读取失败按**不限**放行（fail-open）：限流是可用性保护而非安全边界，
  数据库抖动时不应把全站请求打成 429；失败会记 `warn`。
- keyed 桶的 key 无界增长（多 IP 攻击）由 `retain_recent` 兜底：单个桶超过 16k key 时回收
  已回到初始状态的 key。
