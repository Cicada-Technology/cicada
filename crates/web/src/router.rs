//! 基础路由：健康检查 + 认证接口（`cicada:http` 内置模块）+ Swagger UI。
//!
//! 路由经 `Routes` 声明并走装配器 `mount`，权限点在启动期与 `Manifest.perms` 双向对账，
//! 对账失败即不启动。

use axum::extract::Extension;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{
    Router, middleware,
    routing::{get, post},
};
use http_body_util::BodyExt;
use rust_i18n::t;
use tower_http::compression::CompressionLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::assemble::Assembler;
use crate::async_export::ExportTaskModule;
use crate::auth::{captcha_image, login, logout, profile, refresh, routers};
use crate::cache::CacheModule;
use crate::codegen::CodeModule;
use crate::data::DataModule;
use crate::file::FileModule;
use crate::health::{healthz, metrics, readyz};
use crate::identity;
use crate::jobs::JobModule;
use crate::layers;
use crate::logs::LogModule;
use crate::middleware::auth_middleware;
use crate::module::{Manifest, Module};
use crate::notify::NotifyModule;
use crate::oauth;
use crate::online::OnlineModule;
use crate::openapi::ApiDoc;
use crate::problem::{BizCode, Problem};
use crate::ratelimit::rate_limit_middleware;
use crate::routes::Routes;
use crate::server::ServerModule;
use crate::state::AppState;
use crate::system::SystemModule;
use cicada_kernel::RequestContext;

/// 框架内置路由。健康检查、认证与身份/TOTP 接口不声明权限点（`&[]`）。
pub fn core_routes(state: AppState) -> Routes {
    Routes::new()
        .route("/healthz", get(healthz), &[])
        .route("/readyz", get(readyz), &[])
        .route("/metrics", get(metrics), &[])
        .route("/auth/login", post(login), &[])
        .route("/auth/captcha", get(captcha_image), &[])
        .route("/auth/refresh", post(refresh), &[])
        .route("/auth/logout", post(logout), &[])
        .route("/auth/profile", get(profile), &[])
        .route("/auth/routers", get(routers), &[])
        .merge(identity::routes(state.clone()))
        .merge(oauth::routes(state))
}

/// 内置 HTTP 模块：把框架路由接入同一套装配与对账。
struct HttpModule;

impl Module for HttpModule {
    fn manifest(self: Box<Self>) -> Manifest {
        Manifest::new("cicada:http").routes(core_routes)
    }
}

pub fn router(state: AppState) -> Router {
    router_with(state, Vec::new())
}

/// 未匹配到任何路由的请求：axum 默认回空体 404，统一响应要求错误体统一是 problem+json。
async fn not_found(req: Request) -> Response {
    let path = req.uri().path().to_string();
    let locale = crate::i18n::negotiate_headers(req.headers());
    match req.extensions().get::<RequestContext>() {
        Some(ctx) => Problem::from_context(
            BizCode::SysNotFound,
            t!("common.not_found", locale = ctx.locale.as_str()),
            ctx,
        ),
        None => Problem::from_status(
            StatusCode::NOT_FOUND,
            t!("common.not_found", locale = locale.as_str()),
        ),
    }
    .instance(path)
    .into_response()
}

/// 统一响应契约兜底：提取器拒绝（`Json`/`Query`/`Path` 反序列化失败、媒体类型不对）默认回
/// `text/plain` 原文，这里改写成 problem+json。serde 的技术细节只进日志，对外给按
/// `Accept-Language` 本地化的可读文案；状态码保持拒绝本来的语义。
async fn problem_rejection(req: Request, next: middleware::Next) -> Response {
    let path = req.uri().path().to_string();
    let ctx = req.extensions().get::<RequestContext>().cloned();
    let locale = match &ctx {
        Some(c) => c.locale.clone(),
        None => crate::i18n::negotiate_headers(req.headers()).to_string(),
    };

    let response = next.run(req).await;
    let status = response.status();
    if !status.is_client_error() && !status.is_server_error() {
        return response;
    }
    let is_plain = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/plain"));
    if !is_plain {
        return response;
    }

    let raw = response
        .into_body()
        .collect()
        .await
        .map(|b| b.to_bytes())
        .unwrap_or_default();
    tracing::debug!(path = %path, status = %status, rejection = %String::from_utf8_lossy(&raw).trim(), "提取器拒绝");

    let (code, detail) = if status.is_server_error() {
        (
            BizCode::Internal,
            t!("common.internal_error", locale = locale.as_str()).into_owned(),
        )
    } else if status == StatusCode::UNSUPPORTED_MEDIA_TYPE {
        (
            BizCode::ValidationPayload,
            t!("common.unsupported_media_type", locale = locale.as_str()).into_owned(),
        )
    } else {
        let code = if status == StatusCode::BAD_REQUEST {
            BizCode::ValidationPayload
        } else {
            BizCode::from_status(status)
        };
        (
            code,
            t!("common.invalid_payload", locale = locale.as_str()).into_owned(),
        )
    };
    let mut problem = Problem::new(code, detail).instance(path);
    if let Some(ctx) = &ctx {
        problem = problem.request_id(ctx.request_id.0.clone());
    }
    // 业务码目录里 415 没有独立业务码，挂在 `…_4002` 上但保留原状态码，不改变对外语义。
    problem.with_status(status).into_response()
}

/// 组装根入口：内置模块 + 业务模块一起装配。
///
/// 使用者工程（`cicada new` 生成的 `my-app`）只交出业务模块，框架模块与中间件链
/// 由本函数负责；业务模块同样受权限点双向对账与重名检测约束，对账失败即 panic
/// 在启动期，不留半个可用服务。
pub fn router_with(state: AppState, modules: Vec<Box<dyn Module>>) -> Router {
    router_with_menus(state, modules).0
}

/// `router_with` 的完整形态：同时返回模块声明的菜单（尚未落库）。
///
/// 菜单落库是异步的、且必须由菜单表属主完成（模块边界），因此不在这里做，
/// 交给组装根 `cicada::bootstrap` 在启动时一次性 upsert。
pub fn router_with_menus(
    state: AppState,
    modules: Vec<Box<dyn Module>>,
) -> (Router, Vec<cicada_ports::MenuSpec>) {
    let assembled = Assembler::new()
        .with(Box::new(HttpModule))
        .with(Box::new(OnlineModule))
        .with(Box::new(LogModule))
        .with(Box::new(FileModule))
        .with(Box::new(JobModule))
        .with(Box::new(CacheModule))
        .with(Box::new(ServerModule))
        .with(Box::new(DataModule))
        .with(Box::new(CodeModule))
        .with(Box::new(SystemModule))
        .with(Box::new(NotifyModule))
        .with(Box::new(ExportTaskModule))
        .with_all(modules)
        .seal()
        .expect("核心模块装配失败");
    let menus = assembled.declared_menus();
    let api = assembled.mount(state.clone()).expect("路由权限点对账失败");

    // 中间件链：axum 的 `layer` 是「后加者在外」，因此这里按
    // 8→1 的倒序挂，运行时顺序才是 1 request_id → 2 trace → 2.5 metrics → 3 security_headers
    // → 4 cors → 5 compression → 6 body_limit → 7 rate_limit → 8 auth → 路由级 perm/audit → handler。
    // （2.5 是可观测性要求的 HTTP 指标，不属于请求生命周期的 12 层语义，因此不占整数编号。）
    let api = api
        .merge(SwaggerUi::new("/docs").url("/api-docs/openapi.json", ApiDoc::openapi()))
        // 统一响应：未知路径也要有 problem+json 错误体（方法不匹配仍由 axum 回 405 空体，
        // 业务码目录未给 405 分配业务码）。
        .fallback(not_found)
        // 最内层：把提取器默认的 text/plain 拒绝体改写成 problem+json（统一响应）。
        .layer(middleware::from_fn(problem_rejection))
        // 审计 sink 以应用级 Extension 注入，供路由级审计中间件读取。
        .layer(Extension(state.audit.clone()))
        // 7.5 幂等：在认证之后、路由之前，只拦带 Idempotency-Key 的写请求。
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::idempotency::idempotency_middleware,
        ))
        // 8 认证：Bearer / X-API-Key → Principal + RequestContext。
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        // 7 限流：在鉴权之前挡住未认证的洪水请求。
        .layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit_middleware,
        ))
        // 6 请求体上限（上传/导入路由例外）。
        .layer(middleware::from_fn_with_state(
            layers::BodyLimits::from_config(&state.config),
            layers::body_limit,
        ))
        // 5 压缩。
        .layer(CompressionLayer::new());
    // 4 跨域：`cors.enabled` 为假时不挂层。
    let api = layers::with_cors(api, &state.config);
    let router = api
        // 3 安全响应头。
        .layer(middleware::from_fn(layers::security_headers))
        // 2.5 HTTP 观测（可观测性：请求 span + QPS / 延迟分位）。在 trace 之内，安全头与 CORS 之外，
        // 因此预检请求与短路响应都计入，链路也不依赖日志级别。
        .layer(middleware::from_fn(layers::http_observability))
        // 2 访问日志 span（方法/路径/耗时）。
        .layer(TraceLayer::new_for_http())
        // 1 请求 ID：生成并回传 `X-Request-Id`（`auth_middleware` 从头上读取，保持同一 ID）。
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .with_state(state);
    (router, menus)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::extract::Json;
    use axum::http::{Request as HttpRequest, StatusCode};
    use axum::middleware;
    use axum::routing::post;
    use serde::Deserialize;
    use tower::ServiceExt;

    #[derive(Deserialize)]
    struct Payload {
        #[allow(dead_code)]
        name: String,
    }

    async fn create(Json(_payload): Json<Payload>) -> StatusCode {
        StatusCode::CREATED
    }

    /// 统一响应：提取器拒绝默认是 `text/plain`，挂上兜底层后必须是 problem+json。
    fn rejecting_router() -> Router {
        Router::new()
            .route("/x", post(create))
            .layer(middleware::from_fn(problem_rejection))
    }

    async fn problem_of(request: HttpRequest<Body>) -> (StatusCode, String, serde_json::Value) {
        let response = rejecting_router().oneshot(request).await.unwrap();
        let status = response.status();
        let content_type = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            content_type,
            serde_json::from_slice(&bytes).unwrap(),
        )
    }

    #[tokio::test]
    async fn json_type_mismatch_becomes_validation_problem() {
        let (status, content_type, body) = problem_of(
            HttpRequest::builder()
                .method("POST")
                .uri("/x")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"other":1}"#))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(content_type, "application/problem+json");
        assert_eq!(body["code"], "CICADA_VALIDATION_4001");
        assert_eq!(body["status"], 422);
        assert_eq!(body["instance"], "/x");
        assert!(body["detail"].as_str().unwrap().contains("格式不正确"));
    }

    #[tokio::test]
    async fn malformed_json_keeps_400_with_payload_code() {
        let (status, content_type, body) = problem_of(
            HttpRequest::builder()
                .method("POST")
                .uri("/x")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name": "#))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(content_type, "application/problem+json");
        assert_eq!(body["code"], "CICADA_VALIDATION_4002");
    }

    #[tokio::test]
    async fn missing_json_content_type_keeps_415() {
        let (status, content_type, body) = problem_of(
            HttpRequest::builder()
                .method("POST")
                .uri("/x")
                .header("content-type", "text/plain")
                .body(Body::from(r#"{"name":"n"}"#))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(content_type, "application/problem+json");
        assert_eq!(body["code"], "CICADA_VALIDATION_4002");
        assert_eq!(body["status"], 415);
    }

    #[tokio::test]
    async fn successful_response_is_untouched() {
        let response = rejecting_router()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/x")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"n"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            None
        );
    }
}
