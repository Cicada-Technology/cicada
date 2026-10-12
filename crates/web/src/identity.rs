//! 登录身份绑定与可选 TOTP 二次验证接口。
//!
//! - `GET/POST/DELETE /auth/identities`：当前账号已绑定身份的查询 / 绑定 / 解绑。
//! - `GET/POST /auth/totp{,/setup,/enable,/disable}`：二次验证的查看与开关。
//!
//! 均为「当前账号自助」接口，不声明权限点（登录用户即可操作自己的身份）；
//! 绑定 / 解绑 / 开关 TOTP 挂 `#[audit]` 记录操作日志。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use cicada_auth::{AuthError, IdentityType};

use crate::auth::map_auth_error;
use crate::extractor::RequestContextExt;
use crate::response::created;
use crate::routes::Routes;
use crate::state::AppState;
use rust_i18n::t;

use crate::problem::{ApiErr, BizCode, Problem};

fn unauthorized(locale: &str) -> ApiErr {
    Problem::new(
        BizCode::AuthUnauthorized,
        t!("auth.unauthorized", locale = locale).into_owned(),
    )
}

fn err(locale: &str, e: &AuthError) -> ApiErr {
    map_auth_error(locale, e)
}

// ---- 身份 ----

#[derive(Debug, Serialize, ToSchema)]
pub struct IdentityVo {
    pub identity_id: i64,
    pub identity_type: String,
    pub identifier: String,
    pub provider: Option<String>,
    pub is_primary: bool,
    pub verified: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct BindIdentityRequest {
    /// `Phone` / `Email` / `WeChatOpen` / `WeChatMp`（`Username` 不支持自助绑定）。
    pub identity_type: String,
    pub identifier: String,
    pub provider: Option<String>,
    /// 手机/邮箱的验证码（微信走 OAuth2 回调，不用此字段）。
    pub code: Option<String>,
}

#[utoipa::path(
    get,
    path = "/auth/identities",
    responses(
        (status = 200, description = "已绑定身份", body = [IdentityVo]),
        (status = 401, description = "未认证", body = Problem)
    )
)]
pub async fn list_identities(
    State(state): State<AppState>,
    ctx: RequestContextExt,
) -> Result<Json<Vec<IdentityVo>>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized(&ctx.locale));
    };
    let list =
        cicada_auth::list_identities(state.db.conn(), principal.tenant_id, principal.user_id)
            .await
            .map_err(|e| err(&ctx.locale, &e))?;
    Ok(Json(
        list.into_iter()
            .map(|i| IdentityVo {
                identity_id: i.identity_id,
                identity_type: i.identity_type,
                identifier: i.identifier,
                provider: i.provider,
                is_primary: i.is_primary,
                verified: i.verified_at.is_some(),
            })
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/auth/identities",
    request_body = BindIdentityRequest,
    responses(
        (status = 201, description = "绑定成功", body = IdentityVo),
        (status = 401, description = "未认证", body = Problem),
        (status = 422, description = "验证码错误 / 重复绑定 / 标识非法", body = Problem)
    )
)]
#[cicada_macros::audit(module = "auth", action = "identity:bind", kind = "insert")]
pub async fn bind_identity(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<BindIdentityRequest>,
) -> Result<(StatusCode, HeaderMap, Json<IdentityVo>), ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized(&ctx.locale));
    };
    let identity_type = IdentityType::parse(&req.identity_type)
        .ok_or_else(|| err(&ctx.locale, &AuthError::UnsupportedIdentityType))?;
    let identity = cicada_auth::bind_identity(
        state.db.conn(),
        principal.tenant_id,
        principal.user_id,
        identity_type,
        &req.identifier,
        req.provider.as_deref(),
        req.code.as_deref(),
        state.identity_verifier.as_ref(),
    )
    .await
    .map_err(|e| err(&ctx.locale, &e))?;
    let identity_id = identity.identity_id;
    Ok(created(
        format!("/auth/identities/{identity_id}"),
        IdentityVo {
            identity_id: identity.identity_id,
            identity_type: identity.identity_type,
            identifier: identity.identifier,
            provider: identity.provider,
            is_primary: identity.is_primary,
            verified: identity.verified_at.is_some(),
        },
    ))
}

#[utoipa::path(
    delete,
    path = "/auth/identities/{identity_id}",
    params(("identity_id" = i64, Path, description = "身份 id")),
    responses(
        (status = 204, description = "已解绑"),
        (status = 401, description = "未认证", body = Problem),
        (status = 404, description = "身份不存在", body = Problem),
        (status = 409, description = "必须保留至少一个可登录身份", body = Problem)
    )
)]
#[cicada_macros::audit(module = "auth", action = "identity:unbind", kind = "delete")]
pub async fn unbind_identity(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(identity_id): Path<i64>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized(&ctx.locale));
    };
    cicada_auth::unbind_identity(
        state.db.conn(),
        principal.tenant_id,
        principal.user_id,
        identity_id,
    )
    .await
    .map_err(|e| err(&ctx.locale, &e))?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- TOTP ----

#[derive(Debug, Serialize, ToSchema)]
pub struct TotpStatus {
    pub enabled: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TotpSetupResponse {
    pub secret: String,
    pub otpauth_url: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TotpCodeRequest {
    pub code: String,
}

#[utoipa::path(
    get,
    path = "/auth/totp",
    responses((status = 200, description = "TOTP 状态", body = TotpStatus), (status = 401, description = "未认证", body = Problem))
)]
pub async fn totp_status(
    State(state): State<AppState>,
    ctx: RequestContextExt,
) -> Result<Json<TotpStatus>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized(&ctx.locale));
    };
    let (enabled, _) =
        cicada_auth::totp::load_state(state.db.conn(), principal.tenant_id, principal.user_id)
            .await
            .map_err(|e| err(&ctx.locale, &e))?;
    Ok(Json(TotpStatus { enabled }))
}

#[utoipa::path(
    post,
    path = "/auth/totp/setup",
    responses(
        (status = 200, description = "新的待启用密钥", body = TotpSetupResponse),
        (status = 409, description = "已启用二次验证", body = Problem)
    )
)]
pub async fn totp_setup(
    State(state): State<AppState>,
    ctx: RequestContextExt,
) -> Result<Json<TotpSetupResponse>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized(&ctx.locale));
    };
    let (enabled, _) =
        cicada_auth::totp::load_state(state.db.conn(), principal.tenant_id, principal.user_id)
            .await
            .map_err(|e| err(&ctx.locale, &e))?;
    if enabled {
        return Err(err(&ctx.locale, &AuthError::TotpAlreadyEnabled));
    }
    let setup = cicada_auth::generate_totp_setup(&principal.user_name)
        .ok_or_else(|| err(&ctx.locale, &AuthError::TotpInvalid))?;
    cicada_auth::totp::store_pending_secret(
        state.db.conn(),
        principal.tenant_id,
        principal.user_id,
        &setup.secret_base32,
    )
    .await
    .map_err(|e| err(&ctx.locale, &e))?;
    Ok(Json(TotpSetupResponse {
        secret: setup.secret_base32,
        otpauth_url: setup.otpauth_url,
    }))
}

#[utoipa::path(
    post,
    path = "/auth/totp/enable",
    request_body = TotpCodeRequest,
    responses(
        (status = 204, description = "已启用"),
        (status = 422, description = "验证码错误或未先 setup", body = Problem)
    )
)]
#[cicada_macros::audit(module = "auth", action = "totp:enable", kind = "update")]
pub async fn totp_enable(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<TotpCodeRequest>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized(&ctx.locale));
    };
    let (enabled, secret) =
        cicada_auth::totp::load_state(state.db.conn(), principal.tenant_id, principal.user_id)
            .await
            .map_err(|e| err(&ctx.locale, &e))?;
    if enabled {
        return Err(err(&ctx.locale, &AuthError::TotpAlreadyEnabled));
    }
    let Some(secret) = secret else {
        return Err(err(&ctx.locale, &AuthError::TotpNotEnabled));
    };
    if !cicada_auth::verify_totp(&secret, &req.code) {
        return Err(err(&ctx.locale, &AuthError::TotpInvalid));
    }
    cicada_auth::totp::enable(state.db.conn(), principal.tenant_id, principal.user_id)
        .await
        .map_err(|e| err(&ctx.locale, &e))?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/auth/totp/disable",
    request_body = TotpCodeRequest,
    responses(
        (status = 204, description = "已关闭"),
        (status = 409, description = "未启用", body = Problem),
        (status = 422, description = "验证码错误", body = Problem)
    )
)]
#[cicada_macros::audit(module = "auth", action = "totp:disable", kind = "update")]
pub async fn totp_disable(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<TotpCodeRequest>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized(&ctx.locale));
    };
    let (enabled, secret) =
        cicada_auth::totp::load_state(state.db.conn(), principal.tenant_id, principal.user_id)
            .await
            .map_err(|e| err(&ctx.locale, &e))?;
    if !enabled {
        return Err(err(&ctx.locale, &AuthError::TotpNotEnabled));
    }
    let Some(secret) = secret else {
        return Err(err(&ctx.locale, &AuthError::TotpNotEnabled));
    };
    if !cicada_auth::verify_totp(&secret, &req.code) {
        return Err(err(&ctx.locale, &AuthError::TotpInvalid));
    }
    cicada_auth::totp::disable(state.db.conn(), principal.tenant_id, principal.user_id)
        .await
        .map_err(|e| err(&ctx.locale, &e))?;
    Ok(StatusCode::NO_CONTENT)
}

/// 身份 / TOTP 路由声明（并入核心 HTTP 模块）。
pub fn routes(_state: AppState) -> Routes {
    Routes::new()
        .route("/auth/identities", get(list_identities), &[])
        .route_audited(
            "/auth/identities",
            post(bind_identity),
            &[],
            bind_identity__audit(),
        )
        .route_audited(
            "/auth/identities/{identity_id}",
            delete(unbind_identity),
            &[],
            unbind_identity__audit(),
        )
        .route("/auth/totp", get(totp_status), &[])
        .route("/auth/totp/setup", post(totp_setup), &[])
        .route_audited(
            "/auth/totp/enable",
            post(totp_enable),
            &[],
            totp_enable__audit(),
        )
        .route_audited(
            "/auth/totp/disable",
            post(totp_disable),
            &[],
            totp_disable__audit(),
        )
}
