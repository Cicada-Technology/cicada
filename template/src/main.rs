//! {{project-name}}：组合根。
//!
//! 只做三件事：读配置 → 声明事实（业务模块、定时任务处理器）→ 起服务。
//! 基础设施连接、中间件链、模块装配与权限点对账都在 `cicada::bootstrap` 里，
//! 与框架自己的 `apps/server` 走同一条路径，因此升级框架不需要改这里。

// 生成的 handler 一律 `Result<T, ApiErr>`，Err 是 problem+json 结构体（>128 字节）。
// 失败路径构造，装箱换来的收益抵不上每个业务模块重复压一行 lint 的噪音。
#![allow(clippy::result_large_err)]

use cicada::{AppBuilder, Config, init_tracing_with};

/// 业务代码在 `business/`，与框架物理分离。
#[path = "../business/mod.rs"]
mod business;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load()?;
    // stdout 日志恒开；配置里给了 telemetry.otlp_endpoint 才导出链路。
    // guard 必须持有到进程结束，否则批量 span 来不及 flush。
    let _tracing = init_tracing_with("{{project-name}}=info", &config.telemetry)?;
    let app = AppBuilder::new(config)
        // 业务模块：只声明「我有哪些表、哪些权限点、哪些路由」，装配顺序由装配器推导。
        .with_module(Box::new(business::goods::GoodsModule))
        // 定时任务处理器：`sys_job.invoke_target` 里写的就是这个 key，不做字符串反射。
        .with_job("{{project-name}}:noop", |_ctx| async move {
            Ok(Some("noop".to_string()))
        })
        .build()
        .await?;

    // 路由、健康检查（/healthz、/readyz）、指标（/metrics）、OpenAPI（/docs）随之就位。
    app.serve().await?;
    Ok(())
}
