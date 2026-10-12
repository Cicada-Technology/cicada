//! cicada-server：组装根。
//!
//! 与 `cicada new` 生成的使用者工程走**同一条装配路径**（`cicada::bootstrap`）：
//! 读配置 → 建依赖 → 挂模块 → 起服务。框架自己的发行版因此不会与模板分叉。
//!
//! 本进程不带业务模块；要挂业务模块就用
//! `AppBuilder::with_module(...)`（见 `template/src/main.rs` 与）。

use cicada::{AppBuilder, Config, init_tracing_with};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load()?;
    // 可观测在任何日志之前初始化：stdout 恒开，配了 `telemetry.otlp_endpoint` 才导出链路（可观测性）。
    // guard 要活到进程结束，drop 时把批处理队列里的 span 冲出去。
    let _tracing = init_tracing_with("cicada=info", &config.telemetry)?;
    let app = AppBuilder::new(config)
        .with_job(
            "cicada:noop",
            |_ctx| async move { Ok(Some("noop".to_string())) },
        )
        .build()
        .await?;
    app.serve().await?;
    Ok(())
}
