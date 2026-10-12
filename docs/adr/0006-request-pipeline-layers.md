# ADR 0006：请求流水线补齐 §3.4 的 1–6 层（tower-http），body_limit 自研

- 日期：2026-10-10
- 状态：已采纳

## 背景

设计文档 §3.4 把请求生命周期定义为 12 层，但此前只有第 7（限流，ADR 0005）、8（认证）、
9（RBAC）、11（审计）落地；1–6 层（request_id / trace / security_headers / cors /
compression / body_limit）缺失。副作用是 `server.body_limit_mb` 与 `storage.max_upload_mb`
只是配置项：上传接口 `field.bytes()` 会把任意大的请求体读进内存。

## 结论

- 1/2/4/5 直接用 `tower-http`：
  `SetRequestIdLayer::x_request_id(MakeRequestUuid)` + `PropagateRequestIdLayer`（生成并回传
  `X-Request-Id`，`auth_middleware` 改为从该头读取，全链路同一个 ID）、
  `TraceLayer::new_for_http()`（方法/路径/耗时的 tracing span）、
  `CorsLayer`（配置驱动）、`CompressionLayer`（gzip）。
- 3 `security_headers` 与 6 `body_limit` 自研为 `from_fn` 中间件（`crates/web/src/layers.rs`）：
  - CSP 需要按路径放开：Swagger UI 的内联脚本套 `default-src 'none'` 会白屏，
    因此 `/docs` 与 `/api-docs` 只加通用头，其余路径加严格 CSP；HSTS 只在
    `X-Forwarded-Proto: https` 时下发（明文下发了没有意义）。
  - `body_limit` 需要按路由给不同额度：上传/导入路由用 `storage.max_upload_mb + 1MB`，
    其余路由用 `server.body_limit_mb`。`tower-http` 的 `RequestBodyLimitLayer` 是整层统一额度，
    做不到按路由分档。实现先用 `Content-Length` 快速拒绝，再套
    `http_body_util::Limited` 兜住不带长度（chunked）的请求。
- 超限按设计文档附录 C 的 `CICADA_VALIDATION_4002` 返回 **400**（不是 413）。
- 装配顺序在 `crates/web/src/router.rs` 里显式倒序书写并逐步注释：axum 的 `layer` 是
  「后加者在外」，代码里按 8→1 挂，运行时才是 1→8。
- 版本上锁 `tower-http = 0.6` 而不是设计文档快照里的 0.7.1：0.7 会与 `reqwest` 已引入的 0.6
  形成同一 crate 的两个 major 版本，违反 §3.3 的 cargo-deny 重复版本约束。

## 代价

- `cors` 默认关闭（`enabled: false`），需要放行独立域名前端时才开；`allowed_origins: ["*"]`
  与 `allow_credentials: true` 不能同时生效（已按显式源列表降级并记 warn，避免线上 panic）。
- `Content-Length` 快速拒绝对撒谎的客户端无效，真正的兜底是 `Limited`；二者都命中时给的是
  400 而不是 413，这是跟随设计文档附录 C 的错误码映射。
- 压缩只开 gzip（未开 br/zstd），与设计文档 §4.1 的 `compression-gzip` 一致；大文件下载
  （xlsx）会被无谓压缩，代价可忽略，需要时再加谓词跳过。
- `TraceLayer` 的 span 级别是 DEBUG，默认 `RUST_LOG=cicada=info` 下不产生额外日志。
