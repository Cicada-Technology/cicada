//! 第三方登录接口（微信扫码登录 / 通用 OIDC）。
//!
//! - `GET  /auth/login/{provider}/authorize-url`：取授权地址（同时登记一次性 `state`）；
//! - `POST /auth/login/{provider}/callback`：回调换身份 —— 已绑定直接发令牌，
//!   未绑定只发**短时效绑定票据**（不自动建号，也不按邮箱合并到已有账号）；
//! - `POST /auth/identities/oauth`：登录用户拿票据把外部身份绑到自己的账号。
//!
//! 三个接口都不声明权限点：前两个是匿名登录入口，第三个是「当前账号自助」。
//! 未配置任何提供方时（默认部署）一律 404。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use cicada_audit::LoginLog;
use cicada_auth::{AuthError, ExternalIdentity, OAuthLoginOutcome};

use crate::auth::{LoginResponse, map_auth_error, session_policy};
use crate::extractor::RequestContextExt;
use crate::i18n::Locale;
use crate::response::created;
use crate::routes::Routes;
use crate::state::AppState;

use crate::problem::{ApiErr, Problem};

fn err(locale: &str, e: &AuthError) -> ApiErr {
    map_auth_error(locale, e)
}

#[derive(Serialize, ToSchema)]
pub struct AuthorizeUrlResponse {
    /// 浏览器/内嵌页直接打开的授权地址。
    pub authorize_url: String,
    /// 服务端生成的一次性 `state`，回调时必须原样带回。
    pub state: String,
}

#[derive(Deserialize, ToSchema)]
pub struct CallbackRequest {
    pub code: String,
    pub state: String,
}

/// 回调结果：`bound` 带令牌，`unbound` 带绑定票据。
#[derive(Serialize, ToSchema)]
pub struct CallbackResponse {
    /// `bound`（已绑定，直接可用）或 `unbound`（需先绑定）。
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login: Option<LoginResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding_ticket: Option<String>,
    /// 外部身份的展示信息（昵称/头像），便于绑定页展示「你要绑定的是谁」。
    pub external: ExternalIdentityVo,
}

#[derive(Serialize, ToSchema)]
pub struct ExternalIdentityVo {
    pub identity_type: String,
    pub provider: String,
    pub identifier: String,
    pub nickname: Option<String>,
    pub avatar: Option<String>,
    pub email: Option<String>,
}

impl From<ExternalIdentity> for ExternalIdentityVo {
    fn from(i: ExternalIdentity) -> Self {
        Self {
            identity_type: i.identity_type.as_str().to_string(),
            provider: i.provider,
            identifier: i.identifier,
            nickname: i.nickname,
            avatar: i.avatar,
            email: i.email,
        }
    }
}

#[derive(Deserialize, ToSchema)]
pub struct BindOAuthRequest {
    /// 回调返回的绑定票据。
    pub binding_ticket: String,
}

#[derive(Serialize, ToSchema)]
pub struct BindOAuthResponse {
    pub identity_id: i64,
    pub identity_type: String,
    pub provider: Option<String>,
    pub identifier: String,
}

#[utoipa::path(
    get,
    path = "/auth/login/{provider}/authorize-url",
    params(("provider" = String, Path, description = "提供方标识，如 wechat / keycloak")),
    responses(
        (status = 200, description = "授权地址与 state", body = AuthorizeUrlResponse),
        (status = 404, description = "未配置该提供方", body = Problem)
    )
)]
pub async fn authorize_url(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    locale: Locale,
) -> Result<Json<AuthorizeUrlResponse>, ApiErr> {
    let req = state
        .oauth
        .authorize_url(&provider)
        .await
        .map_err(|e| err(&locale.0, &e))?;
    Ok(Json(AuthorizeUrlResponse {
        authorize_url: req.authorize_url,
        state: req.state,
    }))
}

#[utoipa::path(
    post,
    path = "/auth/login/{provider}/callback",
    params(("provider" = String, Path, description = "提供方标识")),
    request_body = CallbackRequest,
    responses(
        (status = 200, description = "已绑定：直接发令牌；未绑定：返回绑定票据", body = CallbackResponse),
        (status = 401, description = "state 无效或已过期", body = Problem),
        (status = 404, description = "未配置该提供方", body = Problem),
        (status = 502, description = "上游提供方不可用或返回错误", body = Problem)
    )
)]
pub async fn callback(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    ctx: RequestContextExt,
    Json(req): Json<CallbackRequest>,
) -> Result<Json<CallbackResponse>, ApiErr> {
    let ctx = ctx.into_inner();
    let ip = ctx.ip.to_string();
    let ua = ctx.user_agent.clone().unwrap_or_default();
    let tenant_id = ctx.tenant_id.get();
    match state
        .oauth
        .login(
            state.db.conn(),
            &state.tokens,
            state.sessions.as_ref(),
            session_policy(&state),
            ctx.tenant_id,
            &provider,
            &req.code,
            &req.state,
            &ip,
            &ua,
        )
        .await
    {
        Ok(OAuthLoginOutcome::Bound { result, identity }) => {
            state
                .audit
                .record_login(LoginLog::success(
                    tenant_id,
                    &result.principal.user_name,
                    Some(ip.clone()).filter(|s| s != "0.0.0.0"),
                    Some(ua.clone()).filter(|s| !s.is_empty()),
                ))
                .await;
            Ok(Json(CallbackResponse {
                status: "bound".to_string(),
                login: Some(LoginResponse {
                    access_token: result.access_token,
                    refresh_token: result.refresh_token,
                    user_id: result.principal.user_id.get(),
                    tenant_id: result.principal.tenant_id.get(),
                    username: result.principal.user_name,
                }),
                binding_ticket: None,
                external: identity.into(),
            }))
        }
        Ok(OAuthLoginOutcome::Unbound { ticket, identity }) => {
            state
                .audit
                .record_login(LoginLog::failure(
                    tenant_id,
                    format!("oauth:{provider}"),
                    Some(ip.clone()).filter(|s| s != "0.0.0.0"),
                    Some(ua.clone()).filter(|s| !s.is_empty()),
                    "外部身份尚未绑定任何账号".to_string(),
                ))
                .await;
            Ok(Json(CallbackResponse {
                status: "unbound".to_string(),
                login: None,
                binding_ticket: Some(ticket),
                external: identity.into(),
            }))
        }
        Err(e) => {
            state
                .audit
                .record_login(LoginLog::failure(
                    tenant_id,
                    format!("oauth:{provider}"),
                    Some(ip.clone()).filter(|s| s != "0.0.0.0"),
                    Some(ua.clone()).filter(|s| !s.is_empty()),
                    e.to_string(),
                ))
                .await;
            Err(err(&ctx.locale, &e))
        }
    }
}

#[utoipa::path(
    post,
    path = "/auth/identities/oauth",
    request_body = BindOAuthRequest,
    responses(
        (status = 201, description = "绑定成功", body = BindOAuthResponse),
        (status = 401, description = "未认证 / 票据无效", body = Problem),
        (status = 422, description = "该身份已被其他账号绑定", body = Problem)
    )
)]
#[cicada_macros::audit(module = "auth", action = "identity:bindOAuth", kind = "insert")]
pub async fn bind_oauth(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<BindOAuthRequest>,
) -> Result<(StatusCode, HeaderMap, Json<BindOAuthResponse>), ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(err(&ctx.locale, &AuthError::InvalidToken));
    };
    let identity = state
        .oauth
        .bind(
            state.db.conn(),
            &state.tokens,
            principal.tenant_id,
            principal.user_id,
            &req.binding_ticket,
        )
        .await
        .map_err(|e| err(&ctx.locale, &e))?;
    let identity_id = identity.identity_id;
    Ok(created(
        format!("/auth/identities/{identity_id}"),
        BindOAuthResponse {
            identity_id: identity.identity_id,
            identity_type: identity.identity_type,
            provider: identity.provider,
            identifier: identity.identifier,
        },
    ))
}

pub fn routes(_state: AppState) -> Routes {
    Routes::new()
        .route(
            "/auth/login/{provider}/authorize-url",
            get(authorize_url),
            &[],
        )
        .route("/auth/login/{provider}/callback", post(callback), &[])
        .route_audited(
            "/auth/identities/oauth",
            post(bind_oauth),
            &[],
            bind_oauth__audit(),
        )
}
