# cicada-server

框架自己的发行二进制 / 组装根。

## 职责

四步装配：`Config::load()` → `init_tracing_with(filter, &config.telemetry)`（stdout 恒开，
配了 `telemetry.otlp_endpoint` 才导链路）→ `AppBuilder::new(config).with_job(...).build()`
→ `app.serve()`。全部重活都在 `cicada::bootstrap` 里（建 PG 连接、RLS 兜底、Redis 会话 /
锁定 / 验证码 / 幂等 / 授权缓存、存储、OAuth、通知、限流、任务运行时、发件箱 relay、`router_with`）。

关键约定：**本进程与 `cicada new` 生成的使用者工程走同一条装配路径**，因此框架发行版不会与
模板分叉——模板里 `src/main.rs` 的写法就是这里的写法。挂业务模块 = `with_module(Box::new(...))`。

进程内只注册一个示例任务 `cicada:noop`，用来证明「任务注册 → JobRuntime → 执行日志」这条链路
在真实启动路径上是通的。

## 公开 API

没有 lib target，只有 `main.rs`；对外契约是**运行时**而非类型：
监听 `config.server.port`，暴露请求生命周期的整条中间件链、`/docs`、`/healthz` `/readyz` `/metrics`。

## 依赖

`cicada`（门面，默认 `full`：worker + migrations + codegen）+ `axum` / `tokio` / `tracing`，
并直接声明各能力 crate（`web / system / monitor / auth / rbac / audit / storage / notify /
codegen / infra / ports / kernel / jobs / worker`）以固定发行版的编译面。

## 不做什么

- **不带业务模块**：这里不放任何示例业务；业务工程由 `cicada new` 生成。
- 不做进程管理 / 平滑重启 / systemd 集成（部署面在 `deploy/` 与 `docker-compose.yaml`）。
- 不在 `main.rs` 里手写路由或中间件：装配顺序属于 `cicada-web` 与 `bootstrap`，
  组装根改动一次就意味着框架发行版与模板开始分叉。
