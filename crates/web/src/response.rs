//! 创建类响应的统一形状（成功直接返回资源：单资源为对象，创建 `201` + `Location`）。
//!
//! 为什么这件事要收在一个出口：`Location` 是 HTTP 语义的一部分，但每个 handler 自己
//! 拼响应头时最容易漏——漏了不报错、不改变状态码，客户端只是拿不到「新资源在哪」。
//! 项目里所有 201 都从这里出，就只有一处需要被 review。
//!
//! 头里的值是**服务端拼出来的规范 URI**（`/api/v1/.../{id}`，id 来自刚插入的行），
//! 不是客户端给的字符串：`Location` 指向别处会变成一类 SSRF 式的误导。
//! 也正因如此，`HeaderValue` 的构造只会因为编程错误而失败，这里让它当场炸出来，
//! 而不是静默丢头——丢了头的 201 看着完全正常，只有客户端会困惑。

use axum::Json;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};

/// 201 + `Location: {location}` + JSON 体。
pub fn created<T>(location: String, body: T) -> (StatusCode, HeaderMap, Json<T>) {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::LOCATION,
        HeaderValue::from_str(&location)
            .unwrap_or_else(|e| panic!("Location 头只能是 ASCII 路径，收到 {location:?}：{e}")),
    );
    (StatusCode::CREATED, headers, Json(body))
}

#[cfg(test)]
mod tests {
    use super::created;
    use axum::Json;
    use axum::http::StatusCode;
    use axum::http::header::LOCATION;

    fn body() -> serde_json::Value {
        serde_json::json!({ "user_id": 7 })
    }

    #[test]
    fn created_carries_status_location_and_body() {
        let (status, headers, Json(body)) = created("/api/v1/system/users/7".to_string(), body());
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(headers[LOCATION], "/api/v1/system/users/7");
        assert_eq!(body["user_id"], 7);
    }
}
