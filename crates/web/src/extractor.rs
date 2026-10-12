//! `RequestContextExt`：从请求扩展抽取 `RequestContext`（由 auth 中间件写入）。

use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};

use cicada_kernel::RequestContext;

use crate::problem::Problem;
use crate::state::AppState;

/// 对 `RequestContext` 的薄包装，作为 handler 参数即完成抽取。
pub struct RequestContextExt(pub RequestContext);

impl RequestContextExt {
    pub fn into_inner(self) -> RequestContext {
        self.0
    }
}

#[derive(Debug)]
pub struct MissingRequestContext;

impl IntoResponse for MissingRequestContext {
    fn into_response(self) -> Response {
        // 拿不到上下文就没有协商语言，只能回兜底文案（正常链路上第 3 层已注入上下文）。
        Problem::from_status(StatusCode::UNAUTHORIZED, "未认证").into_response()
    }
}

impl FromRequestParts<AppState> for RequestContextExt {
    type Rejection = MissingRequestContext;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<RequestContext>()
            .cloned()
            .map(RequestContextExt)
            .ok_or(MissingRequestContext)
    }
}
