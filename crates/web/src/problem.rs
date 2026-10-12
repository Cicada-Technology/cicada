//! 统一错误响应：RFC 9457 `application/problem+json`。
//!
//! 契约要点：
//! - HTTP 状态码表达结果语义，成功直接返回资源，**错误是这个包体**，没有 `{code,msg}` 包装；
//! - `type` 每个业务码一个稳定 URI（`https://cicada.dev/errors/<域>/<序号>`），便于检索与文档跳转；
//! - `title` 面向人且**按问题类型固定**（不随每次 occurrence 变化，也不翻译）；
//!   每次 occurrence 的可读细节放 `detail`，那一层才走横切能力的服务端国际化；
//! - `code` 是机器可读业务码（业务码目录全集，命名 `CICADA_<域>_<四位>`，一经发布不复用）；
//! - `errors` 只在字段级校验失败时出现，前端表单据此把消息贴回具体字段。
//!
//! 为什么状态码挂在 [`BizCode`] 上而不是调用点：业务码目录的每行都是「码 ↔ HTTP ↔ 何时返回」的
//! 三元组，把状态码交给码可以消除「同一个语义在两处返回不同状态」的漂移；确实需要携带
//! `request_id` / `instance` 时由 [`Problem::from_context`] 从 `RequestContext` 补齐。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;

use cicada_infra::metrics::APP_ERRORS;
use cicada_kernel::RequestContext;

/// 业务码目录。
///
/// 变体名按「域 + 序号」读；`code()` 给出线上标识，`status()` 给出附录约定的 HTTP 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BizCode {
    // ---- AUTH（1xxx）----
    /// 401 未携带令牌，或令牌无效/过期。
    AuthUnauthorized,
    /// 422 账号或密码错误（统一码，不区分身份是否存在，避免枚举）。
    AuthBadCredentials,
    /// 423 账号因连续失败被临时锁定。
    AuthLocked,
    /// 403 账号已停用。
    AuthDisabled,
    /// 422 图形验证码错误或过期。
    AuthCaptchaInvalid,
    /// 422 二次验证失败（含「当前状态不适用二次验证操作」）。
    AuthTotpFailed,
    /// 401 refresh_token 无效或已被轮换（判定重放，撤销整条会话链）。
    AuthRefreshInvalid,
    /// 409 待绑定的身份已属于其他账号。
    AuthIdentityBound,
    /// 409 解绑被拒：账号须保留至少一个可登录身份。
    AuthLastIdentity,
    /// 502 上游授权码换取失败（上游不可用或凭证无效）。
    AuthUpstreamUnavailable,
    /// 401 会话指纹不符（`user_agent` 变了），需要重新登录。
    AuthReauthRequired,

    // ---- RBAC（2xxx）----
    /// 403 缺少权限标识。
    RbacForbidden,
    /// 403 租户上下文缺失或不匹配。
    RbacTenantMismatch,
    /// 403 目标数据在当前数据范围之外（命中兜底防线）。
    RbacOutOfScope,

    // ---- SYS（3xxx）----
    /// 404 资源不存在。
    SysNotFound,
    /// 409 唯一约束冲突（账号名/手机/邮箱/权限标识/编码）。
    SysUnique,
    /// 409 乐观锁冲突（`version` 不匹配）。
    SysOptimisticLock,
    /// 409 存在关联数据，拒绝删除（部门有下级、角色被引用等）。
    SysReferenced,
    /// 422 树结构非法（父节点指向自身或后代）。
    SysTreeIllegal,

    // ---- VALIDATION（4xxx）----
    /// 422 字段级参数校验失败。
    ValidationFields,
    /// 400 请求体超限、JSON 深度/数组长度超限。
    ValidationPayload,
    /// 422 排序字段不在白名单。
    ValidationSortField,
    /// 422 分页超过上限。
    ValidationPageLimit,

    // ---- RATE（5xxx）----
    /// 429 触发限流。
    RateLimited,

    // ---- INTERNAL（9xxx）----
    /// 500 未预期错误（对外只给可读 `detail`，细节只进日志与 trace）。
    Internal,
    /// 503 数据库不可用。
    DbUnavailable,
    /// 503 Redis 不可用。
    CacheUnavailable,
    /// 500 存储/对象存储、邮件等外部依赖失败。
    ExternalDependency,
}

impl BizCode {
    /// 线上标识，出现在包体的 `code` 字段。
    pub const fn code(self) -> &'static str {
        match self {
            Self::AuthUnauthorized => "CICADA_AUTH_1001",
            Self::AuthBadCredentials => "CICADA_AUTH_1002",
            Self::AuthLocked => "CICADA_AUTH_1003",
            Self::AuthDisabled => "CICADA_AUTH_1004",
            Self::AuthCaptchaInvalid => "CICADA_AUTH_1005",
            Self::AuthTotpFailed => "CICADA_AUTH_1006",
            Self::AuthRefreshInvalid => "CICADA_AUTH_1007",
            Self::AuthIdentityBound => "CICADA_AUTH_1008",
            Self::AuthLastIdentity => "CICADA_AUTH_1009",
            Self::AuthUpstreamUnavailable => "CICADA_AUTH_1010",
            Self::AuthReauthRequired => "CICADA_AUTH_1011",
            Self::RbacForbidden => "CICADA_RBAC_2001",
            Self::RbacTenantMismatch => "CICADA_RBAC_2002",
            Self::RbacOutOfScope => "CICADA_RBAC_2003",
            Self::SysNotFound => "CICADA_SYS_3001",
            Self::SysUnique => "CICADA_SYS_3002",
            Self::SysOptimisticLock => "CICADA_SYS_3003",
            Self::SysReferenced => "CICADA_SYS_3004",
            Self::SysTreeIllegal => "CICADA_SYS_3005",
            Self::ValidationFields => "CICADA_VALIDATION_4001",
            Self::ValidationPayload => "CICADA_VALIDATION_4002",
            Self::ValidationSortField => "CICADA_VALIDATION_4003",
            Self::ValidationPageLimit => "CICADA_VALIDATION_4004",
            Self::RateLimited => "CICADA_RATE_5001",
            Self::Internal => "CICADA_INTERNAL_9001",
            Self::DbUnavailable => "CICADA_INTERNAL_9002",
            Self::CacheUnavailable => "CICADA_INTERNAL_9003",
            Self::ExternalDependency => "CICADA_INTERNAL_9004",
        }
    }

    /// 业务码目录约定的 HTTP 状态。
    pub const fn status(self) -> StatusCode {
        match self {
            Self::AuthUnauthorized | Self::AuthRefreshInvalid | Self::AuthReauthRequired => {
                StatusCode::UNAUTHORIZED
            }
            Self::AuthBadCredentials
            | Self::AuthCaptchaInvalid
            | Self::AuthTotpFailed
            | Self::SysTreeIllegal
            | Self::ValidationFields
            | Self::ValidationSortField
            | Self::ValidationPageLimit => StatusCode::UNPROCESSABLE_ENTITY,
            Self::AuthLocked => StatusCode::LOCKED,
            Self::AuthDisabled
            | Self::RbacForbidden
            | Self::RbacTenantMismatch
            | Self::RbacOutOfScope => StatusCode::FORBIDDEN,
            Self::AuthIdentityBound
            | Self::AuthLastIdentity
            | Self::SysUnique
            | Self::SysOptimisticLock
            | Self::SysReferenced => StatusCode::CONFLICT,
            Self::AuthUpstreamUnavailable => StatusCode::BAD_GATEWAY,
            Self::SysNotFound => StatusCode::NOT_FOUND,
            Self::ValidationPayload => StatusCode::BAD_REQUEST,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::Internal | Self::ExternalDependency => StatusCode::INTERNAL_SERVER_ERROR,
            Self::DbUnavailable | Self::CacheUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    /// `type` 的稳定 URI 片段：`<域小写>/<四位序号>`。
    pub const fn slug(self) -> &'static str {
        match self {
            Self::AuthUnauthorized => "auth/1001",
            Self::AuthBadCredentials => "auth/1002",
            Self::AuthLocked => "auth/1003",
            Self::AuthDisabled => "auth/1004",
            Self::AuthCaptchaInvalid => "auth/1005",
            Self::AuthTotpFailed => "auth/1006",
            Self::AuthRefreshInvalid => "auth/1007",
            Self::AuthIdentityBound => "auth/1008",
            Self::AuthLastIdentity => "auth/1009",
            Self::AuthUpstreamUnavailable => "auth/1010",
            Self::AuthReauthRequired => "auth/1011",
            Self::RbacForbidden => "rbac/2001",
            Self::RbacTenantMismatch => "rbac/2002",
            Self::RbacOutOfScope => "rbac/2003",
            Self::SysNotFound => "sys/3001",
            Self::SysUnique => "sys/3002",
            Self::SysOptimisticLock => "sys/3003",
            Self::SysReferenced => "sys/3004",
            Self::SysTreeIllegal => "sys/3005",
            Self::ValidationFields => "validation/4001",
            Self::ValidationPayload => "validation/4002",
            Self::ValidationSortField => "validation/4003",
            Self::ValidationPageLimit => "validation/4004",
            Self::RateLimited => "rate/5001",
            Self::Internal => "internal/9001",
            Self::DbUnavailable => "internal/9002",
            Self::CacheUnavailable => "internal/9003",
            Self::ExternalDependency => "internal/9004",
        }
    }

    /// 面向人的短标题：按问题类型固定，不随 occurrence 变化（RFC 9457 的要求）。
    pub const fn title(self) -> &'static str {
        match self {
            Self::AuthUnauthorized => "未认证",
            Self::AuthBadCredentials => "账号或密码错误",
            Self::AuthLocked => "账号已临时锁定",
            Self::AuthDisabled => "账号已停用",
            Self::AuthCaptchaInvalid => "验证码错误或已过期",
            Self::AuthTotpFailed => "二次验证未通过",
            Self::AuthRefreshInvalid => "刷新令牌无效",
            Self::AuthIdentityBound => "该身份已属于其他账号",
            Self::AuthLastIdentity => "须保留至少一个可登录身份",
            Self::AuthUpstreamUnavailable => "上游认证服务不可用",
            Self::AuthReauthRequired => "登录环境已变化，请重新登录",
            Self::RbacForbidden => "无权限",
            Self::RbacTenantMismatch => "租户上下文不匹配",
            Self::RbacOutOfScope => "数据范围之外",
            Self::SysNotFound => "资源不存在",
            Self::SysUnique => "唯一约束冲突",
            Self::SysOptimisticLock => "数据已被他人修改",
            Self::SysReferenced => "存在关联数据",
            Self::SysTreeIllegal => "树结构非法",
            Self::ValidationFields => "参数校验失败",
            Self::ValidationPayload => "请求体不合法",
            Self::ValidationSortField => "排序字段不合法",
            Self::ValidationPageLimit => "分页超过上限",
            Self::RateLimited => "请求过于频繁",
            Self::Internal => "服务内部错误",
            Self::DbUnavailable => "数据库不可用",
            Self::CacheUnavailable => "缓存不可用",
            Self::ExternalDependency => "外部依赖失败",
        }
    }

    /// 状态码 → 该状态下语义最中立的业务码。
    ///
    /// 只服务于「没有更精确业务码」的通用错误产出（各模块的 `err(status, msg)` 帮助函数）。
    /// 有明确业务语义的分支（密码错误 / TOTP / 幂等冲突 / 字段校验 / 缓存不可用…）必须直接
    /// 构造对应变体，否则会把 `CICADA_VALIDATION_4001` 之类当成兜底码滥用。
    /// 返回值保证 `status()` 与入参一致；业务码目录里没有对应码的状态（如 413）兜底为
    /// [`BizCode::Internal`]（500）——真要返回那种状态，调用点应直接构造业务码。
    pub const fn from_status(status: StatusCode) -> Self {
        match status.as_u16() {
            401 => Self::AuthUnauthorized,
            403 => Self::RbacForbidden,
            404 => Self::SysNotFound,
            409 => Self::SysUnique,
            422 => Self::ValidationFields,
            423 => Self::AuthLocked,
            429 => Self::RateLimited,
            503 => Self::DbUnavailable,
            _ => Self::Internal,
        }
    }
}

/// 字段级校验错误（只在 `ValidationFields` 一类里出现）。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

impl FieldError {
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

/// RFC 9457 问题详情 + cicada 扩展字段（`code` / `request_id` / `errors`）。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Problem {
    /// `type`：本业务码的稳定 URI（序列化时改名，utoipa 跟随 serde）。
    #[serde(rename = "type")]
    pub type_uri: String,
    pub title: &'static str,
    pub status: u16,
    /// 本次 occurrence 的可读细节（已按 `Accept-Language` 本地化）。
    pub detail: String,
    /// 触发问题的请求路径。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    /// 业务码目录的机器可读业务码。
    pub code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<FieldError>,
}

impl Problem {
    /// 业务码 + 已本地化的 `detail`；状态码取业务码目录为该码约定的值。
    ///
    /// 这里是所有失败响应的唯一出口，所以「错误率按业务码分组」（可观测性）的计数也只在这一处。
    #[must_use]
    pub fn new(code: BizCode, detail: impl Into<String>) -> Self {
        metrics::counter!(APP_ERRORS, "code" => code.code()).increment(1);
        Self {
            type_uri: format!("https://cicada.dev/errors/{}", code.slug()),
            title: code.title(),
            status: code.status().as_u16(),
            detail: detail.into(),
            instance: None,
            code: code.code(),
            request_id: None,
            errors: Vec::new(),
        }
    }

    /// 带请求上下文的一构造即补 `request_id`（请求生命周期第 1 层生成的那个）。
    /// `instance` 需要请求路径，`RequestContext` 不携带它，由调用点自行补。
    #[must_use]
    pub fn from_context(code: BizCode, detail: impl Into<String>, ctx: &RequestContext) -> Self {
        Self::new(code, detail).request_id(ctx.request_id.0.clone())
    }

    /// 只有状态码可用时的兜底构造（各模块通用的 `err(status, msg)` 帮助函数走这里）。
    #[must_use]
    pub fn from_status(status: StatusCode, detail: impl Into<String>) -> Self {
        Self::new(BizCode::from_status(status), detail)
    }

    #[must_use]
    pub fn instance(mut self, path: impl Into<String>) -> Self {
        self.instance = Some(path.into());
        self
    }

    #[must_use]
    pub fn request_id(mut self, id: impl Into<String>) -> Self {
        self.request_id = Some(id.into());
        self
    }

    #[must_use]
    pub fn fields(mut self, errors: Vec<FieldError>) -> Self {
        self.errors = errors;
        self
    }

    /// 少数流水线层（体积超限、限流）已自带状态码语义，用它覆盖码里的默认值。
    #[must_use]
    pub fn with_status(mut self, status: StatusCode) -> Self {
        self.status = status.as_u16();
        self
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        // self.status 只可能由 StatusCode::as_u16 产生，from_u16 必回环成功。
        let status = StatusCode::from_u16(self.status).expect("problem status 来自 StatusCode");
        let code = self.code.to_string();
        let mut response = (status, axum::Json(self)).into_response();
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/problem+json"),
        );
        // 业务码同时进一个响应头：审计中间件要的是「哪个码」，而响应体是一次性的流，
        // 为了记一个字段去缓冲整份响应不值得。头也让 curl 调试时一眼看到码。
        response.headers_mut().insert(
            BIZ_CODE_HEADER,
            axum::http::HeaderValue::from_str(&code)
                .unwrap_or_else(|_| axum::http::HeaderValue::from_static("CICADA_INTERNAL_9000")),
        );
        response
    }
}

/// 携带业务码目录业务码的响应头名。
pub const BIZ_CODE_HEADER: axum::http::HeaderName =
    axum::http::HeaderName::from_static("x-cicada-biz-code");

/// 接口模块统一的错误类型：`Result<T, ApiErr>`，错误体就是 problem+json。
///
/// 取代旧的 `(StatusCode, Json<ErrorBody>)` —— 状态码由 [`BizCode`] 决定，不再由调用点
/// 携带（除少数需要覆写的场景，用 [`Problem::with_status`]）。
pub type ApiErr = Problem;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_status_keeps_the_status_and_picks_a_neutral_code() {
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::NOT_FOUND,
            StatusCode::CONFLICT,
            StatusCode::UNPROCESSABLE_ENTITY,
            StatusCode::LOCKED,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            let problem = Problem::from_status(status, "x");
            assert_eq!(problem.status, status.as_u16(), "{status} 被改了状态");
            assert!(problem.code.starts_with("CICADA_"));
        }
        // 请求体超限由 `BizCode::ValidationPayload`（400）表达，不走 413 兜底。
        assert_eq!(
            Problem::new(BizCode::ValidationPayload, "x").status,
            StatusCode::BAD_REQUEST.as_u16()
        );
        // 通用兜底：422 落字段校验码，403 落 RBAC 码。
        assert_eq!(
            Problem::from_status(StatusCode::UNPROCESSABLE_ENTITY, "x").code,
            "CICADA_VALIDATION_4001"
        );
        assert_eq!(
            Problem::from_status(StatusCode::FORBIDDEN, "x").code,
            "CICADA_RBAC_2001"
        );
    }

    #[test]
    fn problem_serializes_rfc9457_with_extensions() {
        let problem = Problem::new(BizCode::RbacForbidden, "缺少权限标识 system:user:add")
            .request_id("b1f0c3")
            .instance("/api/v1/system/users");
        let json = serde_json::to_value(&problem).expect("可序列化");
        assert_eq!(json["type"], "https://cicada.dev/errors/rbac/2001");
        assert_eq!(json["title"], "无权限");
        assert_eq!(json["status"], 403);
        assert_eq!(json["code"], "CICADA_RBAC_2001");
        assert_eq!(json["request_id"], "b1f0c3");
        assert_eq!(json["instance"], "/api/v1/system/users");
        // 空 errors 与缺 instance 不出现（RFC 9457 的可选项不该发 null）。
        assert!(json.get("errors").is_none());
    }

    #[test]
    fn omitted_fields_stay_omitted() {
        let json =
            serde_json::to_value(Problem::new(BizCode::SysNotFound, "用户 7 不存在")).unwrap();
        assert!(json.get("instance").is_none());
        assert!(json.get("request_id").is_none());
        assert!(json.get("errors").is_none());
    }

    #[test]
    fn catalog_codes_are_unique_and_match_appendix_c() {
        // 业务码目录的 27 个码：`CICADA_<域>_<四位>`，码一经发布不复用。
        let all = [
            BizCode::AuthUnauthorized,
            BizCode::AuthBadCredentials,
            BizCode::AuthLocked,
            BizCode::AuthDisabled,
            BizCode::AuthCaptchaInvalid,
            BizCode::AuthTotpFailed,
            BizCode::AuthRefreshInvalid,
            BizCode::AuthIdentityBound,
            BizCode::AuthLastIdentity,
            BizCode::AuthUpstreamUnavailable,
            BizCode::RbacForbidden,
            BizCode::RbacTenantMismatch,
            BizCode::RbacOutOfScope,
            BizCode::SysNotFound,
            BizCode::SysUnique,
            BizCode::SysOptimisticLock,
            BizCode::SysReferenced,
            BizCode::SysTreeIllegal,
            BizCode::ValidationFields,
            BizCode::ValidationPayload,
            BizCode::ValidationSortField,
            BizCode::ValidationPageLimit,
            BizCode::RateLimited,
            BizCode::Internal,
            BizCode::DbUnavailable,
            BizCode::CacheUnavailable,
            BizCode::ExternalDependency,
        ];
        assert_eq!(all.len(), 27);
        for (i, code) in all.iter().enumerate() {
            assert!(!all[..i].contains(code), "业务码重复：{}", code.code());
            // 命名规则 `CICADA_<域>_<四位>`：域段非空、末段恰为 4 位数字。
            let code_str = code.code();
            let rest = code_str
                .strip_prefix("CICADA_")
                .unwrap_or_else(|| panic!("业务码缺前缀：{code_str}"));
            let (domain, number) = rest
                .rsplit_once('_')
                .unwrap_or_else(|| panic!("业务码缺域段：{code_str}"));
            assert!(
                !domain.is_empty()
                    && number.len() == 4
                    && number.bytes().all(|b| b.is_ascii_digit()),
                "业务码命名不合规则：{code_str}"
            );
        }
        // 抽查业务码目录的三元组。
        assert_eq!(BizCode::AuthLocked.status(), StatusCode::LOCKED);
        assert_eq!(BizCode::ValidationPayload.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            BizCode::DbUnavailable.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
