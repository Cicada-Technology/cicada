# cicada-web

L4 接入层：把能力层的领域服务装配成 HTTP 面。

## 职责

- **模块注册契约**：`Module` 交出 `Manifest`（路由 + 权限点 + 拥有的表 + 菜单节点 + 种子），
  `Assembled::declared_menus()` 把模块声明的菜单（`Manifest.menus` → `MenuSpec`）交给组装根，
  由菜单属主落库（模块边界），本 crate 不写 `sys_menu`。
  `Assembler` 收集并做重名检测与权限点双向对账（`seal()` / `mount()` 失败即 panic，
  不留半个可用服务）；`Slot` / `Extension` / `Merge` 描述同一挂载点上的横向扩展。
  业务侧用 `declare_module!` 宏生成本地 `Module` 类型。最终设计见
  `docs/superpowers/design-it-twice/FINAL.md`。
- **中间件链**（请求生命周期，8 层 tower Layer + 第 2.5 层 HTTP 指标）。`layers.rs` 是业务无关的外圈 1–6
  （request_id → trace → security_headers → cors → compression → body_limit），
  `router.rs` 从 8→1 倒序装配（axum 的 `layer` 后加者在外）；
  第 8 层认证在 `middleware.rs`（验 JWT → `AuthzCache` 装载授权 → 组装 `RequestContext`），
  路由级再叠 `#[require_perm]` 与 `#[audit]`；限流见 `ratelimit.rs`，幂等见 `idempotency.rs`。
- **出参约定**：直接返回领域 VO 的 JSON + **真实 HTTP 状态码**（不是参考实现的「永远 200 + code」），
  失败响应统一是 RFC 9457 的 `application/problem+json`（`problem.rs` 的 `Problem` / `BizCode` /
  `FieldError`，业务码见业务码目录）；各模块用 `type ApiErr = Problem`，状态码由 `BizCode` 决定，
  少数需要覆写状态的场景用 `Problem::with_status`。路由层还有两处兜底（`router.rs`）：
  未知路径由 `not_found` 回 404 problem+json，提取器（`Json`/`Query`/`Path`）默认的
  `text/plain` 拒绝体由 `problem_rejection` 改写成 problem+json 并保留原状态码；
  方法不匹配仍由 axum 回空体 405（业务码目录未给 405 分配业务码）。
- **消息目录重编**：`build.rs` 声明 `rerun-if-changed=locales`——rust-i18n 只监视自己 crate 的
  locales 目录，不声明就会改了 `locales/*.yml` 却不重编、新增 key 静默回退成 fallback 语言。
- **模糊检索**（`search.rs`）：列上的 `fuzzy()` = `ILIKE '%kw%'` + 把输入里的 `\` `%` `_`
  转义成字面量。SeaORM 的 `contains` 是 `LIKE`：大小写敏感，且用户打一个 `%`
  就成了通配符。所有搜索框只走这一个出口，配套的 `pg_trgm` GIN 索引由
  `migrations` 的 `FUZZY_COLUMNS` 清单给出（两边读同一份列清单）。
- **提取器**（`extractor.rs`）：把 `cicada-kernel` 的 `RequestContext` 注入 handler；
  分页与数据范围过滤用 kernel 的 `Query<E, S>` typestate，接入层不重新实现。
- **功能域路由模块**：`auth` / `oauth` / `identity`（`HttpModule`）、`system/*`（用户 部门 岗位
  角色 菜单 字典 参数 公告 租户 API 凭证）、`notify`、`file`、`jobs`、`cache`、`online`、
  `server`、`data`、`logs`（操作/登录日志 + `monitor:auditlog:list` 的字段级留痕只读面）、`export` / `import` / `async_export`、`codegen`、`health`。
  接口清单承诺的接口已全部有 handler（`crates/web/tests/appendix_e.rs` 逐条打真实 HTTP）：
  角色补 `status` / `data-scope` / `users`（GET 带 `belonged` 标记、PUT 批量分配或取消），
  菜单补 `tree` / `{id}` 详情 / `sort`（防环）/ `role-tree/{role_id}`（勾选树），
  部门补 `tree` / `sort`（只改同级顺序，移动仍走 `PUT /depts/{id}`），
  公告补 `{id}/readers` 与 `top`（仅需登录，带当前用户 `read` 标记）。
- **OpenAPI 汇总**：`ApiDoc::openapi()` + `/docs`（swagger-ui）+ `/api-docs/openapi.json`；
  免认证运维面 `/healthz` `/readyz` `/metrics`；`/metrics` 是真实导出（可观测性）：抓取时采样
  连接池与 Redis 计数，其余序列（`http_requests_total` / `http_request_duration_seconds`
  由 `layers::http_observability` 记录，`app_errors_total{code}` 由 `Problem::new` 记录）在事件点写。
- 服务端消息目录：`rust_i18n::i18n!("locales", fallback = "zh-CN")`（横切能力）。

## 公开 API

`Assembler` / `AssembleReport` / `AssembleError` / `Assembled`、
`Module` / `Manifest` / `ModuleName` / `MenuNode` / `MenuType` / `SeedKey` / `SeedOp` / `TableName`、
`Slot` / `SlotKey` / `Extension` / `Merge`、
`Routes` / `RouteMeta` / `RoutePerms` / `RouteAudit`、`AppState`、
`Problem` / `BizCode` / `FieldError` / `ApiErr`、`search::Fuzzy` / `search::escape_like`、
`router` / `router_with` / `router_with_menus`、`healthz` / `readyz` / `metrics`、`RequestContextExt`、
`declare_module!`，并 re-export `cicada_kernel::Permission`。

## 依赖

所有 L2/L3 能力 crate（`auth` / `rbac` / `audit` / `storage` / `system` / `notify` / `monitor` /
`jobs` / `codegen` / `infra` / `ports` / `macros` / `kernel`）+ `axum` / `tower-http` / `utoipa` /
`utoipa-swagger-ui` / `rust-i18n` / `sea-orm` / `rust_xlsxwriter` / `calamine`。

## 不做什么

- **不含领域规则**：权限计算在 `rbac`、会话与授权快照在 `auth`、脱敏与数据范围语义在 `kernel`；
  这里只做「翻译成 HTTP」，因此没有 `AppError` 的业务分支判断。
- 不做持久化决策：表结构与迁移归 `cicada-migrations` 与各能力 crate 的 entity。
- 不起后台任务运行时：cron 触发与队列执行在 `cicada-worker`，本 crate 只交出任务的管理路由。
- 不做内嵌数据库控制台 / 表单构建器（非目标）。
