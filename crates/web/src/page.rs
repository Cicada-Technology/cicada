//! 分页参数解析：`page` 从 1 开始，`page_size` 默认 20、上限 200；
//! **超限报错而不是静默截断**。
//!
//! 为什么不是 clamp：静默把 `page_size=5000` 改成 200 之后，调用方以为自己拿到了
//! 5000 条——它拿到的其实是前 200 条，而 `total` 又是对的，于是「翻页翻不完」这种
//! 现象没有任何一处报错可以指。参数越界属于客户端错误，按统一错误响应出 422 +
//! `CICADA_VALIDATION_4004`，让调用方自己修正。

use crate::problem::{ApiErr, BizCode, FieldError, Problem};

/// 默认每页条数（分页约定）。
pub const DEFAULT_PAGE_SIZE: u64 = 20;

/// 每页条数上限（分页约定）：列表接口一律用它，不再各写各的数字。
pub const MAX_PAGE_SIZE: u64 = 200;

/// 解析并校验 `page` / `page_size`；返回已校正过的 `(page, page_size)`。
///
/// 缺失取默认值；`page < 1` 与 `page_size` 越界都算调用方错误。
pub(crate) fn resolve(page: Option<u64>, page_size: Option<u64>) -> Result<(u64, u64), ApiErr> {
    let size = page_size.unwrap_or(DEFAULT_PAGE_SIZE);
    if !(1..=MAX_PAGE_SIZE).contains(&size) {
        return Err(Problem::new(
            BizCode::ValidationPageLimit,
            format!("page_size 必须在 1..={MAX_PAGE_SIZE} 之间（收到 {size}）"),
        )
        .fields(vec![FieldError::new(
            "page_size",
            format!("上限 {MAX_PAGE_SIZE}；需要更多数据请翻页"),
        )]));
    }
    let number = page.unwrap_or(1);
    if number < 1 {
        return Err(Problem::new(
            BizCode::ValidationPageLimit,
            format!("page 从 1 开始（收到 {number}）"),
        )
        .fields(vec![FieldError::new("page", "最小为 1")]));
    }
    Ok((number, size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    #[test]
    fn defaults_match_the_documented_contract() {
        assert_eq!(resolve(None, None).unwrap(), (1, DEFAULT_PAGE_SIZE));
        assert_eq!(resolve(Some(3), Some(50)).unwrap(), (3, 50));
        // 边界：正好 200 是合法的，越界一格就报错。
        assert_eq!(
            resolve(Some(1), Some(MAX_PAGE_SIZE)).unwrap().1,
            MAX_PAGE_SIZE
        );
    }

    /// 「静默截断」是这一条要消灭的行为：报错必须点名参数与上限。
    #[tokio::test]
    async fn oversized_page_size_is_rejected_not_clamped() {
        for bad in [MAX_PAGE_SIZE + 1, 5_000, u64::MAX] {
            let resp = resolve(None, Some(bad)).unwrap_err().into_response();
            assert_eq!(resp.status(), axum::http::StatusCode::UNPROCESSABLE_ENTITY);
            let text = body_text(resp).await;
            assert!(text.contains("page_size"), "{text}");
            assert!(text.contains("200"), "上限要写在消息里：{text}");
        }
        let zero = resolve(None, Some(0)).unwrap_err().into_response();
        assert!(body_text(zero).await.contains("CICADA_VALIDATION_4004"));
        let page0 = resolve(Some(0), None).unwrap_err().into_response();
        assert!(body_text(page0).await.contains("page"));
    }

    /// 业务码必须是 4004（分页上限），不是兜底的 4001。
    #[tokio::test]
    async fn the_error_carries_the_appendix_c_code() {
        let resp = resolve(None, Some(9999)).unwrap_err().into_response();
        let text = body_text(resp).await;
        assert!(text.contains("CICADA_VALIDATION_4004"), "{text}");
        // RFC 9457 的形状：`type` 是 URN，`title` 与业务码一一对应。
        assert!(
            text.contains("\"type\":\"https://cicada.dev/errors/validation/4004\""),
            "{text}"
        );
        assert_eq!(
            BizCode::ValidationPageLimit.code(),
            "CICADA_VALIDATION_4004"
        );
    }

    async fn body_text(resp: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .expect("读响应体");
        String::from_utf8_lossy(&bytes).to_string()
    }
}
