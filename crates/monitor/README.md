# cicada-monitor

L2 领域：运行期只读观测的数据侧——在线用户、缓存域、服务指标。

## 职责

- `server::collect()`：一次采集 CPU（差值采样）、内存、Swap、负载、磁盘、
  进程与系统运行时长，产出 `ServerStat`。
  两次刷新的间隔必须 ≥ `sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`（Linux 上 200 ms），
  短于它 sysinfo 直接返回 0；回归测试见 `tests/server_stat.rs`。
  磁盘只保留有容量的挂载点（Docker 的 overlay 挂载会报 0 容量并淹没真实磁盘）。
- `cache`：缓存域白名单与 Redis 观测 adapter（`CacheInspector` 的实现侧）。
- 在线用户的权威源是 Redis 会话（`cicada-auth::SessionStore`），本 crate 只做读取整形。

## 公开 API

`ServerStat` / `DiskStat` / `collect`、`cache::*`（域定义与 inspector）。

## 依赖

`cicada-infra` / `cicada-ports` / `sysinfo` / `serde` / `async-trait`。

## 不做什么

- **定时任务不在这里**：`sys_job` / `sys_job_log` 归 `cicada-jobs` 与 `cicada-worker`。
- 不做内嵌数据库控制台（Druid 式）：那是的明确非目标。
- 不出 HTTP 面：路由与出参在 `cicada-web` 的 `server` / `cache` / `online` 模块。
