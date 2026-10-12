# cicada-jobs

L2 领域：定时任务的定义与执行日志，`sys_job` / `sys_job_log` 的唯一属主。

## 职责

- `store::JobStore`：任务 CRUD、下次触发时间维护、执行日志写入。
- `cron::validate_cron`：cron 表达式校验（保存时即拒绝非法表达式）。
- 调用目标是**注册表 key**（`cicada-ports::JobRegistry`），不是字符串反射——
  这是对参考实现定时任务反序列化 RCE 的根治：Rust 没有反射，字符串只能映射到显式注册的 handler。

## 公开 API

`JobStore` / `JobDefinition` / `JobStatus` / `NewJobLog` / `JobStoreError`、
`entity::*`、`validate_cron`。

## 依赖

`cicada-kernel` / `cicada-infra` / `sea-orm` / `croner` / `chrono`。

## 不做什么

- 不调度、不执行：运行时在 `cicada-worker`（触发侧与执行侧两条轴）。
- 不拥有 HTTP 面：接口在 `cicada-web::jobs`。
