//! 可观测性：tracing 结构化输出（stdout）+ 可选 OTLP 链路导出。
//!
//! 日志由采集器统一收集，因此 stdout 恒开；链路只有在
//! `telemetry.otlp_endpoint` 配置了 collector 地址时才导出（本地开发默认不导）。
//!
//! DB 查询作为 span 子节点由 SeaORM 的 `record_stmt_in_spans` 提供（见 `db::Db::connect`）：
//! 语句记录挂在当前 tracing span 上，于是 `TraceLayer` 的请求 span 里能看到这条请求跑过哪些 SQL。

use opentelemetry::{KeyValue, trace::TracerProvider as _};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::trace::Sampler;
use opentelemetry_sdk::{
    Resource,
    trace::{SdkTracer, SdkTracerProvider as TracerProvider},
};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry, fmt};

use crate::config::TelemetryConfig;

#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    #[error("构建 OTLP 导出器失败：{0}")]
    Exporter(String),
    #[error("初始化 subscriber 失败：{0}")]
    Init(String),
}

/// 持有 tracer provider，drop 时把批处理队列里的 span 冲出去。
///
/// 短命令（CLI）不 flush 就会丢掉最后几条链路，因此 guard 必须由调用方持有到进程结束。
pub struct TracingGuard {
    provider: Option<TracerProvider>,
}

impl TracingGuard {
    /// 未配置 OTLP 时的空 guard。
    pub fn none() -> Self {
        Self { provider: None }
    }

    /// 链路是否真的在导出（测试与 `/readyz` 之类的诊断用）。
    pub fn otlp_enabled(&self) -> bool {
        self.provider.is_some()
    }

    /// 立即 flush 当前批次（不释放 guard）。
    pub fn force_flush(&self) {
        if let Some(provider) = &self.provider {
            let _ = provider.force_flush();
        }
    }
}

impl Drop for TracingGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.provider.take() {
            let _ = provider.force_flush();
            let _ = provider.shutdown();
        }
    }
}

/// 只开 stdout（`telemetry` 段取默认值 = 不导出链路）。CLI 与测试用它。
pub fn init_tracing(default_filter: &str) {
    let _ = init_tracing_with(default_filter, &TelemetryConfig::default());
}

/// stdout + 可选 OTLP 链路。返回的 guard 要一直持有到进程结束。
///
/// 幂等：subscriber 已初始化时（同进程多次 build，如测试）静默跳过，不报错。
pub fn init_tracing_with(
    default_filter: &str,
    cfg: &TelemetryConfig,
) -> Result<TracingGuard, TelemetryError> {
    // `RUST_LOG` 优先，解析不了则回落到调用方给的默认级别（不在启动时因一个坏字符串而 panic）。
    let mut filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    // 可观测性「DB 查询作为 span 子节点」：SeaORM 的语句 span 是 trace 级的
    // （`sea_orm::database` / `sea_orm::driver::*`）。不打开它链路上就没有子 span，
    // 而 SeaORM 在该 target 上不产生额外事件，stdout 不会因此变吵。
    // 操作方自己写了 `sea_orm=` 时不覆盖——那是对级别有主张的显式选择。
    let operator_set_db_level = std::env::var("RUST_LOG")
        .map(|spec| spec.contains("sea_orm"))
        .unwrap_or(false);
    if !operator_set_db_level {
        filter = filter.add_directive("sea_orm=trace".parse().expect("固定指令串必然可解析"));
    }
    let (provider, tracer) = build_otlp(cfg)?;
    let guard = TracingGuard { provider };

    let base = Registry::default()
        .with(filter)
        .with(fmt::layer().with_target(true));
    // layer 在这里现场构造：它的 `S` 由挂载位置推断，写死具体类型反而装不上。
    let inited = match tracer {
        Some(tracer) => base
            .with(tracing_opentelemetry::layer().with_tracer(tracer))
            .try_init(),
        None => base.try_init(),
    };
    if inited.is_err() {
        // 已经有人装过 subscriber：把刚建的 provider 立刻 flush 掉，不留悬空批次。
        return Ok(TracingGuard::none());
    }
    Ok(guard)
}

/// 按配置构建 provider 与 tracer；没配 endpoint 就返回 `(None, None)`，
/// 由调用方决定挂不挂 OTLP layer（layer 的 `S` 要在挂载点现场推断）。
fn build_otlp(
    cfg: &TelemetryConfig,
) -> Result<(Option<TracerProvider>, Option<SdkTracer>), TelemetryError> {
    let Some(raw) = cfg
        .otlp_endpoint
        .as_deref()
        .filter(|e| !e.trim().is_empty())
    else {
        return Ok((None, None));
    };
    let endpoint = format!("{}/v1/traces", raw.trim_end_matches('/'));
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .build()
        .map_err(|e| TelemetryError::Exporter(e.to_string()))?;
    let sampler = if (cfg.sample_ratio - 1.0).abs() < f64::EPSILON {
        Sampler::ParentBased(Box::new(Sampler::AlwaysOn))
    } else {
        Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(cfg.sample_ratio)))
    };
    let provider = TracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_sampler(sampler)
        .with_resource(
            Resource::builder()
                .with_attributes([KeyValue::new("service.name", cfg.service_name.clone())])
                .build(),
        )
        .build();
    let tracer = provider.tracer("cicada");
    Ok((Some(provider), Some(tracer)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_endpoint_no_layer_is_built() {
        let cfg = TelemetryConfig::default();
        let (provider, tracer) = build_otlp(&cfg).unwrap();
        assert!(provider.is_none());
        assert!(tracer.is_none());
    }

    #[test]
    fn blank_endpoint_is_treated_as_absent() {
        let cfg = TelemetryConfig {
            otlp_endpoint: Some("   ".to_string()),
            ..Default::default()
        };
        assert!(build_otlp(&cfg).unwrap().0.is_none());
    }

    #[test]
    fn guard_without_provider_is_a_noop() {
        let guard = TracingGuard::none();
        assert!(!guard.otlp_enabled());
        guard.force_flush();
    }
}
