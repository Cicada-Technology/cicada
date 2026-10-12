# ADR 0008：审计写入走事务性发件箱，日志落库归表属主 `system`

- 日期：2026-10-16
- 状态：已采纳

## 背景

设计文档 §7.2 定义了分工：`sys_oper_log` / `sys_logininfor` 属主是 `system`，`audit` 只是**生产者**，
`monitor` 只提供只读视图；并且明令「`audit` 不直接写 `sys_oper_log` / `sys_logininfor`」
（§3.3 规则 6 的数据所有权）。§13 把「`outbox_event` 表与 relay、审计改写为 sink」列为 M1/M2 交付项，
§15 未决 11 还留着「outbox 启用范围」这一问。

实现长期落后于这段文字：`crates/audit` 的 `DbAuditSink` 直接 `INSERT INTO sys_oper_log` /
`sys_logininfor`，越界写了别人的表；`outbox_event` 在代码里完全不存在（只有迁移注释里提过一次），
`crates/ports` 也没有事件端口。单体里看不出问题，一旦按 §16.2 拆微服务，这条写入边就是第一个死结。

## 结论

- **表**：`migrations/src/m20261016_000011_outbox.rs` 按附录 F 建 `outbox_event`
  （`payload jsonb`、`published_at` 可空、`attempts`），索引 `(published_at, id)` 正对
  relay 的「按 id 顺序扫未投递」形态。
- **生产者只写自己的表**：`OutboxAuditSink`（`crates/audit/src/outbox.rs`）把 `OperLog` /
  `LoginLog` 序列化成事件写入发件箱；`DbAuditSink` 连同 UA→浏览器/OS 的解析一起删掉——
  那是 `sys_logininfor` 的列，属主自己解析。
- **端口放 `ports`**：`crates/ports/src/event.rs` 定义 `DomainEvent`、`EventHandler`、
  `EventRouter` 与负载契约 `OperLogEvent` / `LoginLogEvent`。L3 与 L2 不得互相依赖（§3.3 规则 2/3），
  把字段契约放在中立 crate 让两端共用同一个类型：改字段名是编译错误，不是运行期才发现的 JSON 键漂移。
- **消费者是属主**：`cicada_system::SysLogStore` 实现 `EventHandler`，是
  `sys_oper_log` / `sys_logininfor` / `sys_audit_log` 三张表的唯一写入者
  （`audit.change_log` 事件与字段级留痕后来并入同一通道，见 §7.2 补记）。
  日志时间取发件箱行的 `created_at`，不是投递时刻——relay 排队不该改写操作发生的时间。
- **投递语义：至少一次 + 有界重试**。盖章 `published_at` 与落库不在同一事务，崩溃在两者之间会重投，
  日志表可能出现重复行；反过来「盖章前投递失败」若丢事件等于销毁证据，所以失败只 `attempts+1`，
  事件留在发件箱。到 `outbox.max_attempts`（默认 5）才盖章离场并打 `error`：毒事件若永不清除，
  会永久占住 `ORDER BY id LIMIT batch` 的扫描窗口，把后面的正常事件饿死。
- **relay 是组装根的后台任务**：`crates/all/src/bootstrap.rs` 起循环（`poll_interval_ms` / `batch`），
  每轮调用 `Outbox::drain_once`。核心动作（取、盖章、记账、排空一轮）留在 `cicada-audit`，
  这样 `crates/web` 的集成测试能显式 drain 而不必依赖门面 crate。
- **`outbox_event` 不进 §6.3 的 RLS 白名单**：策略是 fail-closed（没有 `app.tenant_id` 就读不到行），
  而 relay 是跨租户的系统任务；给它开 RLS 等于让审计停摆。租户信息随事件的 `tenant_id` 走到属主表，
  由属主表自己的策略兜住。
- **启用范围（回答 §15 未决 11）**：当前只有审计走 outbox。`notify` 与租户配额仍是直写——
  §3.3 规则 7 要求「同一接口有两个 adapter 才提端口」，等出现第二个跨模块副作用再迁。

## 代价

- 审计从「同步一次 INSERT」变成「写发件箱 + 最多 `poll_interval_ms` 的可见延迟」。监控页刚做完
  操作就刷新，可能看不到那条日志；对日志类数据可接受，需要即时可见的场景应读业务表而不是日志表。
- 至少一次意味着重复行。日志表没有幂等键（兼容形态如此），去重要么加唯一约束（改动属主表结构），
  要么在查询侧忽略；本期不处理，量级是「崩溃恢复窗口内的尾部若干条」。
- 多一张表、多一个后台任务、多一段配置。相对的直接收益是 §16.2 的拆分前提成立：
  `audit` 与 `system` 之间已经没有直连边，换 MQ 只需替换 relay 的投递实现。
- relay 循环没有优雅停机（随进程退出）；`App` 持有 `JoinHandle` 只为保活，不做在途事件等待。
  真正的优雅停机与 §12 的 K8s 清单一起排期。

## 验证

- `crates/web/tests/monitor_logs.rs`（`--ignored`，真实 PG）：写操作后先断言发件箱有 1 条待发且
  `sys_oper_log` 为 0 条（越界写法会在这里红），drain 后列表/详情/导出/删除/清空全部可读；
  另一条用例断言未注册类型重试 5 轮、第 6 轮被放弃且发件箱清空。
- `crates/ports/src/event.rs` 与 `crates/system/src/log_store.rs` 的单测覆盖负载往返、
  绑定值个数与 UA 解析；`cargo clippy --workspace --all-targets -- -D warnings`、
  `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --workspace` 均通过。
- 实机：独立 schema 全新迁移 + 起服务，登录与建角色后 relay 自动投递，
  `sys_oper_log.oper_time` 与 `outbox_event.created_at` 逐微秒一致，
  带 UA 的登录在 `sys_logininfor` 得到 `Chrome` / `macOS`。
