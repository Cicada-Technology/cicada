//! 服务端国际化：`rust-i18n` 消息目录 + `Accept-Language` 协商。
//!
//! 设计取舍：
//! - rust-i18n 4.x 的当前语言是**进程级**全局（`set_locale`），在 tokio 多线程任务间
//!   共享会互相串语言；所以这里从不调用 `set_locale`，一律用 `t!(..., locale = ...)`
//!   显式传入由请求协商出的语言（存于 `RequestContext::locale`）。
//! - 消息键与参考实现 `messages.properties` 同名映射（`user.*`、`user.jcaptcha.error` 等），
//!   cicada 独有语义放 `auth.* / common.* / idempotency.*`；目录在 `locales/*.yml`。
//! - 审计日志与 tracing 不做翻译（服务端事实记录，保持稳定中文）。

use axum::http::HeaderMap;

/// 兜底语言：`Accept-Language` 缺失或全部不支持时使用。
pub const DEFAULT_LOCALE: &str = "zh-CN";

/// 协商 `Accept-Language` 到一个受支持语言的标识（`zh-CN` / `en-US`）。
///
/// 规则：按 q 权重从高到低扫描（默认 q=1，缺权重/非法权重按 0 处理，稳定保序），
/// 主语言子标签 `zh*` → `zh-CN`、`en*` → `en-US`，其余跳过；无命中回退默认语言。
#[must_use]
pub fn negotiate(accept_language: Option<&str>) -> String {
    let Some(raw) = accept_language.filter(|s| !s.trim().is_empty()) else {
        return DEFAULT_LOCALE.to_string();
    };
    // (序号, q, 命中的语言)，先按 q 降序、q 相同按出现顺序。
    let mut candidates: Vec<(usize, f32, &'static str)> = raw
        .split(',')
        .enumerate()
        .filter_map(|(idx, entry)| {
            let (tag, params) = entry.split_once(';').unwrap_or((entry, ""));
            let q = params
                .split(';')
                .find_map(|p| p.trim().strip_prefix("q="))
                .map_or(1.0, |v| v.trim().parse::<f32>().unwrap_or(0.0));
            let base = tag
                .trim()
                .split('-')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            let locale: &'static str = match base.as_str() {
                "zh" => "zh-CN",
                "en" => "en-US",
                _ => return None,
            };
            Some((idx, q, locale))
        })
        .collect();
    candidates.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    candidates
        .first()
        .map_or_else(|| DEFAULT_LOCALE.to_string(), |c| c.2.to_string())
}

/// 从请求头直接协商（中间件在 `RequestContext` 组装前也要产错误消息，走这条路）。
#[must_use]
pub fn negotiate_headers(headers: &HeaderMap) -> String {
    negotiate(
        headers
            .get(axum::http::header::ACCEPT_LANGUAGE)
            .and_then(|v| v.to_str().ok()),
    )
}

/// 语言抽取器：用于拿不到（或不该依赖）`RequestContext` 的 handler。
pub struct Locale(pub String);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for Locale {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(negotiate_headers(&parts.headers)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_i18n::t;

    #[test]
    fn negotiate_defaults_to_zh() {
        assert_eq!(negotiate(None), "zh-CN");
        assert_eq!(negotiate(Some("")), "zh-CN");
        assert_eq!(negotiate(Some("fr,de;q=0.8")), "zh-CN");
    }

    #[test]
    fn negotiate_matches_base_tag() {
        assert_eq!(negotiate(Some("en-US,en;q=0.9")), "en-US");
        assert_eq!(negotiate(Some("zh-Hans-CN,zh;q=0.9")), "zh-CN");
        assert_eq!(negotiate(Some("en")), "en-US");
    }

    #[test]
    fn negotiate_respects_quality_order() {
        assert_eq!(negotiate(Some("zh;q=0.8,en;q=0.9")), "en-US");
        // q 相同时按出现顺序取先者。
        assert_eq!(negotiate(Some("en;q=0.3,zh;q=0.3")), "en-US");
        // 缺省 q=1 优先于显式 q<1；非法 q 按 0 处理。
        assert_eq!(negotiate(Some("zh;q=0.5,en")), "en-US");
        assert_eq!(negotiate(Some("en;q=abc,zh")), "zh-CN");
    }

    #[test]
    fn messages_resolve_in_both_locales() {
        assert_eq!(t!("auth.unauthorized", locale = "zh-CN"), "未认证");
        assert_eq!(t!("auth.unauthorized", locale = "en-US"), "Unauthenticated");
        // 未知语言由 fallback（zh-CN）兜底。
        assert_eq!(
            t!("user.jcaptcha.error", locale = "fr"),
            "图形验证码错误或已过期"
        );
    }

    #[test]
    fn messages_interpolate_named_args() {
        let s = t!(
            "user.password.retry.limit.exceed",
            locale = "en-US",
            seconds = 42u64
        );
        assert_eq!(
            s,
            "Account locked due to failed sign-ins, retry in 42 seconds"
        );
    }
}
