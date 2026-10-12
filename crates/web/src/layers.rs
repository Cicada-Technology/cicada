//! 请求生命周期的**外圈 Layer（1–6）**：request_id → trace → security_headers → cors →
//! compression → body_limit。装配顺序（含第 7/8 层）见 `router.rs` 的注释。
//!
//! 这些层与业务无关，全部在 `router()` 里一次性挂上；`from_fn` 型中间件保持「无状态」或只吃
//! 自己那份小配置，方便脱离 `AppState` 单测。

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue, Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http_body_util::Limited;
use rust_i18n::t;
use tower_http::cors::{AllowOrigin, CorsLayer};

use cicada_infra::Config;

use cicada_infra::metrics::{HTTP_LATENCY, HTTP_REQUESTS};

use crate::problem::{BizCode, Problem};
use crate::state::AppState;

/// 请求体不受 `server.body_limit_mb` 约束的路由（上传/导入）：改用 `storage.max_upload_mb`
/// 加一份多部分编码余量。 第 6 层的「上传路由例外」。
const UPLOAD_PATHS: [&str; 2] = ["/api/v1/system/files", "/api/v1/system/users/import"];

/// 多部分/表单编码的边界与头部余量（1 MB）。
const MULTIPART_SLACK_BYTES: usize = 1024 * 1024;

/// Swagger UI 自带内联脚本，套严格 CSP 会白屏；这两个前缀只加通用安全头。
const DOC_PREFIXES: [&str; 2] = ["/docs", "/api-docs"];

/// 第 6 层 `body_limit` 的配额（启动期从 `Config` 算出，作为中间件自己的 state）。
#[derive(Debug, Clone, Copy)]
pub struct BodyLimits {
    default_bytes: usize,
    upload_bytes: usize,
}

impl BodyLimits {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            default_bytes: cfg.server.body_limit_mb * 1024 * 1024,
            upload_bytes: (cfg.storage.max_upload_mb * 1024 * 1024)
                .saturating_add(MULTIPART_SLACK_BYTES),
        }
    }

    /// 上传/导入路由用大额度，其余路由用 `server.body_limit_mb`。
    fn for_path(&self, path: &str) -> usize {
        if UPLOAD_PATHS.contains(&path) {
            self.upload_bytes
        } else {
            self.default_bytes
        }
    }
}

/// 第 6 层：请求体上限。`Content-Length` 先挡一道（早失败、错误信息明确），
/// 再给请求体套 `http_body_util::Limited`，兜住不带长度（chunked）的请求。
///
/// 超限按业务码目录的 `CICADA_VALIDATION_4002` 返回 **400**（不是 413）。
pub async fn body_limit(
    axum::extract::State(limits): axum::extract::State<BodyLimits>,
    req: Request,
    next: Next,
) -> Response {
    let limit = limits.for_path(req.uri().path());
    if let Some(len) = content_length(&req)
        && len > limit as u64
    {
        let locale = crate::i18n::negotiate_headers(req.headers());
        return payload_too_large(limit, &locale);
    }
    let (parts, body) = req.into_parts();
    let req = Request::from_parts(parts, Body::new(Limited::new(body, limit)));
    next.run(req).await
}

/// 第 3 层：安全响应头（请求生命周期：`X-Content-Type-Options` / CSP / HSTS）。
pub async fn security_headers(req: Request, next: Next) -> Response {
    let path = req.uri().path().to_owned();
    let https = req
        .headers()
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        == Some("https");
    let mut resp = next.run(req).await;
    let headers = resp.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    // HSTS 只在确实走 HTTPS 时下发（明文下发了也没有意义，还会污染本地开发）。
    if https {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        );
    }
    if !DOC_PREFIXES
        .iter()
        .any(|p| path == *p || path.starts_with(&format!("{p}/")))
    {
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
        );
    }
    resp
}

/// 第 4 层：跨域。`cors.enabled = false` 时不挂层（浏览器同源或网关反向代理的场景）。
pub fn with_cors(router: Router<AppState>, cfg: &Config) -> Router<AppState> {
    if !cfg.cors.enabled {
        return router;
    }
    let any = cfg.cors.allowed_origins.iter().any(|o| o == "*");
    let origins: Vec<HeaderValue> = cfg
        .cors
        .allowed_origins
        .iter()
        .filter(|o| *o != "*")
        .filter_map(|o| o.parse().ok())
        .collect();
    // 规范禁止 `*` 与凭证同用；这里以「显式源列表」为准并记一条告警，避免线上 panic。
    let credentials = cfg.cors.allow_credentials;
    if any && credentials {
        tracing::warn!("cors: allow_credentials=true 与通配源 `*` 不兼容，已按显式源列表放行");
    }
    let allow_origin = if any && !credentials {
        AllowOrigin::any()
    } else {
        AllowOrigin::list(origins)
    };
    let mut layer = CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::ACCEPT,
            HeaderName::from_static("x-api-key"),
            HeaderName::from_static("x-request-id"),
        ])
        .max_age(Duration::from_secs(cfg.cors.max_age_secs));
    if credentials {
        layer = layer.allow_credentials(true);
    }
    router.layer(layer)
}

fn content_length(req: &Request) -> Option<u64> {
    req.headers()
        .get(header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

fn payload_too_large(limit: usize, locale: &str) -> Response {
    // 业务码目录 `CICADA_VALIDATION_4002`：请求体超限归 400，不是 413（网关常吞 413）。
    Problem::new(
        BizCode::ValidationPayload,
        t!(
            "common.payload_too_large",
            locale = locale,
            limit_mb = limit / (1024 * 1024)
        ),
    )
    .into_response()
}

/// 第 2.5 层：HTTP 观测——请求 span + QPS + 延迟分位。
///
/// 为什么自己开 span 而不复用 `TraceLayer` 的那个：tower-http 的请求 span 是 TRACE 级，
/// 生产日志级别（info）下它根本不存在，链路导出就没东西可导。这里用 INFO 级的
/// `http.request` span（属性名按 OTel 语义约定），于是 `otel.kind=server` 的链路在任何
/// 日志级别都能上报，DB 语句（`record_stmt_in_spans`）也挂得上去。
///
/// 挂在 request_id / trace 之内、业务之外：每个进入应用的请求都记账，
/// 而 Prometheus 抓取自己那条 `/metrics` 也只多一条计数，不会递归。
pub async fn http_observability(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let route = normalized_route(&path);
    let request_id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let span = tracing::info_span!(
        "http.request",
        otel.kind = "server",
        http.request.method = %method.as_str(),
        http.route = %route.as_str(),
        url.path = %path.as_str(),
        http.response.status_code = tracing::field::Empty,
        otel.status_code = tracing::field::Empty,
        cicada.request_id = %request_id,
    );
    let started = std::time::Instant::now();
    let resp = tracing::Instrument::instrument(next.run(req), span.clone()).await;
    span.record("http.response.status_code", resp.status().as_u16() as i64);
    if resp.status().is_server_error() {
        span.record("otel.status_code", "ERROR");
    }
    let labels = [
        ("method", method.as_str().to_owned()),
        ("route", route.clone()),
    ];
    metrics::histogram!(HTTP_LATENCY, &labels).record(started.elapsed().as_secs_f64());
    metrics::counter!(
        HTTP_REQUESTS,
        "method" => method.as_str().to_owned(),
        "route" => route,
        "status" => resp.status().as_u16().to_string(),
    )
    .increment(1);
    resp
}

/// 把路径里的「实例段」压成 `{id}`，让 `route` 标签与路由模板同构。
///
/// 不这么做的话基数会随数据量线性增长（每个 `/api/v1/system/users/123` 一条时间序列），
/// 指标本身就成了攻击面。识别规则：纯数字、UUID、以及长度 ≥ 16 的十六进制串（雪花 ID 等）。
pub fn normalized_route(path: &str) -> String {
    let segments: Vec<&str> = path
        .split('/')
        .map(|seg| {
            if is_instance_segment(seg) {
                "{id}"
            } else {
                seg
            }
        })
        .collect();
    segments.join("/")
}

fn is_instance_segment(seg: &str) -> bool {
    if seg.is_empty() {
        return false;
    }
    // 纯数字 ID。
    if seg.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    if is_uuid(seg) {
        return true;
    }
    // 长十六进制串（雪花/ULID 之类），且至少含一个数字——避免把纯字母的路由名误判成 ID。
    let long_hex = seg.len() >= 16 && seg.bytes().all(|b| b.is_ascii_hexdigit());
    long_hex && seg.bytes().any(|b| b.is_ascii_digit())
}

/// UUID 的 8-4-4-4-12 形态。
fn is_uuid(seg: &str) -> bool {
    let groups: Vec<&str> = seg.split('-').collect();
    let lens = [8usize, 4, 4, 4, 12];
    groups.len() == lens.len()
        && groups
            .iter()
            .zip(lens)
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()))
        && groups.iter().any(|g| g.bytes().any(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn normalized_route_collapses_instance_segments() {
        assert_eq!(
            normalized_route("/api/v1/system/users/123"),
            "/api/v1/system/users/{id}"
        );
        assert_eq!(
            normalized_route("/api/v1/system/users/123/roles/456"),
            "/api/v1/system/users/{id}/roles/{id}"
        );
        // UUID 与雪花 ID 这类「十六进制 + 数字」也收敛。
        assert_eq!(
            normalized_route("/api/v1/files/2f1c9a7e-8b3d-4c6a-9f0e-1d2b3c4d5e6f"),
            "/api/v1/files/{id}"
        );
        // 非实例段保持原样，避免把真实路由名也压掉。
        for path in [
            "/api/v1/monitor/oper-logs/export",
            "/auth/login",
            "/healthz",
            "/api/v1/system/users/import",
        ] {
            assert_eq!(normalized_route(path), path);
        }
        assert_eq!(normalized_route("/"), "/");
    }
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};
    use axum::middleware;
    use axum::routing::{get, post};
    use tower::ServiceExt;

    fn limits() -> BodyLimits {
        BodyLimits {
            default_bytes: 16,
            upload_bytes: 1024,
        }
    }

    async fn echo(body: String) -> String {
        body
    }

    fn limited_router() -> Router {
        Router::new()
            .route(UPLOAD_PATHS[0], post(echo))
            .route("/json", post(echo))
            .layer(middleware::from_fn_with_state(limits(), body_limit))
    }

    #[tokio::test]
    async fn content_length_over_limit_is_rejected() {
        let resp = limited_router()
            .oneshot(
                HttpRequest::post("/json")
                    .header("content-length", "1024")
                    .body(Body::from("x".repeat(1024)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert!(
            String::from_utf8_lossy(&body).contains("请求体超过上限"),
            "应由 body_limit 层拒绝：{}",
            String::from_utf8_lossy(&body)
        );
    }

    #[tokio::test]
    async fn upload_path_uses_the_larger_budget() {
        let resp = limited_router()
            .oneshot(
                HttpRequest::post(UPLOAD_PATHS[0])
                    .header("content-length", "512")
                    .body(Body::from("x".repeat(512)))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    }

    #[tokio::test]
    async fn body_without_content_length_is_cut_by_the_streaming_limit() {
        // 不带 Content-Length：只能靠 `Limited` 兜住（超限后 handler 取体失败）。
        let resp = limited_router()
            .oneshot(
                HttpRequest::post("/json")
                    .body(Body::from(vec![b'x'; 64]))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_client_error() || resp.status().is_server_error(),
            "未声明长度的超限请求体必须被拒，实际 {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn security_headers_are_added_except_on_docs() {
        let app = Router::new()
            .route("/api", get(|| async { "ok" }))
            .route("/docs", get(|| async { "ok" }))
            .layer(middleware::from_fn(security_headers));
        let api = app
            .clone()
            .oneshot(HttpRequest::get("/api").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            api.headers().get("x-content-type-options").unwrap(),
            "nosniff"
        );
        assert!(api.headers().contains_key("content-security-policy"));
        let docs = app
            .oneshot(HttpRequest::get("/docs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(!docs.headers().contains_key("content-security-policy"));
        // 前缀相近的普通路由不能被误判成文档路由。
        let similar = Router::new()
            .route("/docs-like", get(|| async { "ok" }))
            .layer(middleware::from_fn(security_headers))
            .oneshot(HttpRequest::get("/docs-like").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(similar.headers().contains_key("content-security-policy"));
    }
}
