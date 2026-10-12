# cicada-audit

审计能力：操作日志与登录日志的**采集与投递**端。

## 职责

- `AuditSink`：把一条审计事实写走的缝。
- `OutboxAuditSink`：生产默认。只写本 crate 拥有的 `outbox_event`（审计日志表），
  **不碰 `system` 的 `sys_oper_log` / `sys_logininfor`**——那是依赖规则规则 6 的数据所有权。
- `Outbox`：发件箱的读写门面。`enqueue` / `fetch_pending` / `mark_published` /
  `record_attempt` / `abandon`，以及 relay 每轮调用的 `drain_once`（至少一次投递，
  毒事件到 `max_attempts` 后盖章离场，避免饿死后面的事件）。
- `change`：字段级变更留痕（审计设计，P1）的纯计算部分——`diff(before, after)` 只保留变化字段
  （`{field: [old, new]}`），并滤掉凭据类列（`is_sensitive`）；`ChangeLog` 带
  `insert` / `update` / `delete` 三种动作。
- `InMemoryAuditSink`：测试断言用；`NoopAuditSink`：只要不报错的场合。
- 写失败**绝不阻断业务请求**：实现内部只记 `warn`，错误不外抛给 handler。

落库全链路（`audit` 写事件 → relay 投递 → `cicada-system::SysLogStore` 写表 →
`cicada-web::logs` 读）由 `cicada::bootstrap` 装配，循环任务在组装根里起。取舍与代价见 ADR 0008。

## 公开 API

`AuditSink`（含默认不记录变更的 `record_change`）/ `OutboxAuditSink` / `Outbox` /
`DrainReport` / `InMemoryAuditSink` / `NoopAuditSink`，以及 `OperLog` / `LoginLog` /
`ChangeLog` / `ChangeAction` / `diff` / `is_sensitive` / `AuditMeta` / `OperType`。

## 依赖

`cicada-infra`（`Db`）/ `cicada-ports`（`DomainEvent`、`EventRouter`、事件负载契约）/
`sea-orm` / `serde_json` / `async-trait` / `thiserror`。

## 不做什么

- 不决定「哪些接口要审计」：那是 handler 上的 `#[audit]`（`cicada-macros`）
  与 `cicada-web::audit` 中间件的职责。
- 不写别人的表，也不做日志查询界面：落库在 `cicada-system::log_store`，读路径在 `cicada-web::logs`。
- 不拥有登录策略：登录成败由 `cicada-auth` 判定，这里只记录。
