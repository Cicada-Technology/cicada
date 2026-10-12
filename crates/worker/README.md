# cicada-worker

L4 接入层（可选独立进程）：任务运行时。

## 职责

触发与执行是两条正交的轴，各有一个端口：

- 触发侧 `runtime::TriggerMode`：`cron::CronTriggerSource`（进程内 cron）或 manual。
- 执行侧 `runtime::ExecutionMode`：`inline::InlineRunSink`（默认，触发即执行）或
  `apalis::ApalisRunSink`（投递到 PostgreSQL 队列，由 `ApalisConsumer` 消费）。
- `executor::JobExecutor`：并发策略、超时、失败重试、执行日志**只有一份实现**，两侧共用。
- `lock::RedisDistributedLock`：集群下**只有触发侧需要选主**；执行侧每个实例都启用。

## 公开 API

`JobExecutor`、`runtime::{Worker, TriggerMode, ExecutionMode}`、`CronTriggerSource`、
`InlineRunSink`、`ApalisRunSink` / `ApalisQueue` / `ApalisConsumer` / `JobEnvelope`、
`RedisDistributedLock`、`schedule::*`。

## 依赖

`cicada-kernel` / `cicada-infra` / `cicada-ports` / `cicada-jobs` / `croner` /
`tokio-cron-scheduler` / `apalis` / `apalis-sql`。

## 不做什么

- 不拥有任务表：`sys_job` / `sys_job_log` 在 `cicada-jobs`，这里只经 `JobStore` 读写。
- 不使用 apalis 的 `MemoryStorage`：进程内队列重启即丢，不能作为生产默认实现。
- 不做反射式调用：目标只能是注册表 key。
