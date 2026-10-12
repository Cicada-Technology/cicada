//! 可观测性指标导出：HTTP 层的计数/直方图与业务码错误率必须出现在 Prometheus 文本里。
//!
//! 这个测试文件是独立进程，所以它拥有自己的 recorder（`install` 只在进程里装一次）。

use axum::Router;
use axum::body::Body;
use axum::extract::Path;
use axum::http::{Request, StatusCode};
use axum::middleware;
use axum::response::IntoResponse;
use axum::routing::get;
use tower::ServiceExt;

use cicada_web::layers::http_observability;
use cicada_web::problem::{BizCode, Problem};

async fn user(_: Path<i64>) -> &'static str {
    "ok"
}

async fn missing() -> axum::response::Response {
    Problem::new(BizCode::SysNotFound, "路径不存在").into_response()
}

#[tokio::test]
async fn http_observability_and_error_codes_are_exposed() {
    // 同进程重复安装会被拒（OnceLock），这里只关心 recorder 在不在。
    let _ = cicada_infra::metrics::install();

    let app = Router::new()
        .route("/api/v1/system/users/{id}", get(user))
        .route("/boom", get(missing))
        .layer(middleware::from_fn(http_observability));

    let resp = app
        .clone()
        .oneshot(
            Request::get("/api/v1/system/users/42")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let _ = axum::body::to_bytes(resp.into_body(), 1024).await;

    let resp = app
        .oneshot(Request::get("/boom").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let _ = axum::body::to_bytes(resp.into_body(), 1024).await;

    let text = cicada_infra::metrics::render();
    assert!(text.contains("cicada_up 1"), "{text}");
    assert!(
        text.contains(r#"route="/api/v1/system/users/{id}""#),
        "路径里的实例段应被压成模板：{text}"
    );
    assert!(text.contains(r#"status="200""#), "{text}");
    assert!(text.contains(r#"status="404""#), "{text}");
    assert!(text.contains("http_requests_total"), "{text}");
    assert!(
        text.contains("http_request_duration_seconds_bucket"),
        "延迟要有分桶才有分位：{text}"
    );
    assert!(
        text.contains(r#"app_errors_total{code="CICADA_SYS_3001"}"#),
        "{text}"
    );
}
