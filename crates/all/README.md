# cicada（门面 crate）

单一入口 crate（`crates/all`，包名 `cicada`）：re-export + feature 转发 + 装配骨架，
供「不想研究 crate 划分」的使用者一把接入。

使用者工程只需要：

```toml
cicada = { git = "https://github.com/<org>/cicada", tag = "v0.1.0", features = ["full"] }
```

## 职责

- [`bootstrap`]：`AppBuilder` —— 读配置 → 建依赖（PG / Redis / 会话 / 验证码 / 幂等 / 授权缓存 /
  审计 sink / 存储 / OAuth / 通知 / 限流 / 任务运行时）→ 产出 `AppState` + `router_with` 路由，
  业务模块经 `AppBuilder::with_module` 挂上，注册任务经 `with_job` / `with_job_handler`。
  跨模块管道也在这里接线：模块声明的菜单经 `cicada_system::DbMenuRegistry` 落 `sys_menu`
  （模块边界，可用 `AppBuilder::with_menu_registry` 换 adapter），
  `outbox_event` 的 relay 后台任务把 `audit` 的事件投给
  `system` 的落库处理器（审计设计），参数是 `outbox` 配置段。
  日志分区生命周期（审计设计保留期 / 部署自动建分区任务）同样只在这里成形：启动先
  `LogRetention::ensure_partitions` 补窗口，之后由框架自带任务 `JOB_LOGS_RETENTION`
  （`cicada:logs:retention`，每月一次，seed 落 `sys_job` 行）执行「导出 NDJSON → 归档 → 删分区」，
  归档端口 `ArchiveStore` 由 `StorageArchive` 桥到 `Storage`——`system`（L2）不为写归档
  去依赖 `storage`（L3），这座桥是装配的一部分。参数是 `audit` 配置段。
  这里面**只有装配**，没有领域规则。
- 分层 crate 的稳定别名：`cicada::web` / `cicada::kernel` / `cicada::auth` / …，
  与 `cicada_web::…` 等价。
- 统一生态 crate 版本（`axum` / `sea-orm` / `serde` / `tokio` / `utoipa` / …），
  使用者工程不必各写一份。

三条接入路径（模板化机制）：门面（`bootstrap`）、分层（直接依赖某一层）、定制（实现 `ports` 里的
trait 替换默认 adapter）。

## 公开 API

- `bootstrap::{AppBuilder, App, BootstrapError, JOB_LOGS_RETENTION}`；
- 根级高频类型：`App` / `AppBuilder` / `BootstrapError`、`Cache` / `Config` / `Db` / `init_tracing` / `init_tracing_with`、
  `Permission` / `Scope` / `TenantId` / `UserId`、`JobContext`、`AppState`、
  `Manifest` / `Module` / `Routes` / `router` / `router_with`；
- `prelude::*`：业务工程常用 `use` 集合（领域类型 + sea-orm + serde + `async_trait`）；
- 生态 crate 转发：`axum` / `sea_orm` / `sea_orm_migration` / `serde` / `serde_json` / `utoipa`；
- 各层 crate 的模块别名。

## 依赖

`cicada-kernel / infra / ports / auth / rbac / audit / storage / notify / system / monitor / web`
（必需），`worker / migrations / codegen / jobs`（可选 feature）。
feature：`full`（默认 = `worker` + `migrations` + `codegen`）、`["migrations"]`（只读服务）。

## 不做什么

- **不含任何实现**：这个 crate 里除了 `bootstrap` 的装配代码没有第二个逻辑。
- **feature 不用于裁剪领域模块**：`AppState` 的形状决定了这些 crate 必须参与编译，
  领域能力靠「不注册模块」裁掉，而不是靠 feature（见 `crates/all/Cargo.toml` 注释）。
- 不暴露未公布组合的 feature 矩阵：只有 `full` 与 `["migrations"]` 是被测试的组合。
