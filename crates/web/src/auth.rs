//! 认证接口：登录、刷新、登出、当前用户、路由。

use axum::{Json, extract::State, http::StatusCode};
use rust_i18n::t;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use cicada_audit::LoginLog;
use cicada_auth::{AuthError, LockoutPolicy, SessionPolicy, lockout_key};
use cicada_kernel::RoleId;
use cicada_system::{load_grant_snapshot, load_menu_tree};

use crate::extractor::RequestContextExt;
use crate::problem::{ApiErr, BizCode, Problem};
use crate::state::AppState;

#[derive(Deserialize, ToSchema)]
pub struct LoginRequest {
    /// 登录标识符：账号名 / 手机 / 邮箱。
    pub username: String,
    pub password: String,
    /// 启用 TOTP 的账号在此携带 6 位动态码。
    #[serde(default)]
    pub totp: Option<String>,
    /// `GET /auth/captcha` 发放的验证码 id（连续失败后必带）。
    #[serde(default)]
    pub captcha_id: Option<String>,
    /// 用户输入的验证码字符。
    #[serde(default)]
    pub captcha_code: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct LoginResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub user_id: i64,
    pub tenant_id: i64,
    pub username: String,
}

/// 刷新请求：只带刷新令牌（不透明，内含会话 id）。
#[derive(Deserialize, ToSchema)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Serialize, ToSchema)]
pub struct RefreshResponse {
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Serialize, ToSchema)]
pub struct ProfileResponse {
    pub user_id: i64,
    pub user_name: String,
    pub tenant_id: i64,
    pub roles: Vec<String>,
    pub permissions: Vec<String>,
}

/// 密码失败锁定策略（配置 `auth.password`）。
fn lockout_policy(state: &AppState) -> LockoutPolicy {
    LockoutPolicy {
        max_retry: state.config.auth.password.max_retry,
        lock_secs: state.config.auth.password.lock_minutes * 60,
    }
}

pub(crate) fn session_policy(state: &AppState) -> SessionPolicy {
    SessionPolicy {
        refresh_ttl_secs: state.config.auth.refresh_ttl_days * 24 * 60 * 60,
        max_sessions: state.config.auth.max_sessions,
    }
}

#[utoipa::path(
    post,
    path = "/auth/login",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "登录成功", body = LoginResponse),
        (status = 422, description = "账号或密码错误", body = Problem),
        (status = 403, description = "账号已停用", body = Problem)
    )
)]
pub async fn login(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<LoginRequest>,
) -> Result<(StatusCode, Json<LoginResponse>), ApiErr> {
    let ctx = ctx.into_inner();
    let ip = ctx.ip.to_string();
    let ua = ctx.user_agent.unwrap_or_default();
    let tenant_id = ctx.tenant_id.get();

    // 图形验证码闸门：连续失败达到阈值后强制；前端主动带了也校验。
    let failures = state
        .lockout
        .failures(&lockout_key(tenant_id, &req.username))
        .await
        .unwrap_or(0);
    let provided = req.captcha_id.as_deref().is_some_and(|s| !s.is_empty())
        || req.captcha_code.as_deref().is_some_and(|s| !s.is_empty());
    if state.captcha.is_required(failures) || (state.captcha.params().enabled && provided) {
        let passed = state
            .captcha
            .verify(req.captcha_id.as_deref(), req.captcha_code.as_deref())
            .await
            .map_err(|e| map_auth_error(&ctx.locale, &e))?;
        if !passed {
            state
                .audit
                .record_login(LoginLog::failure(
                    tenant_id,
                    &req.username,
                    Some(ip.clone()).filter(|s| s != "0.0.0.0"),
                    Some(ua.clone()).filter(|s| !s.is_empty()),
                    "图形验证码错误或已过期",
                ))
                .await;
            return Err(Problem::new(
                BizCode::AuthCaptchaInvalid,
                t!("user.jcaptcha.error", locale = ctx.locale.as_str()).into_owned(),
            ));
        }
    }

    match cicada_auth::login(
        state.db.conn(),
        &state.tokens,
        state.sessions.as_ref(),
        state.lockout.as_ref(),
        session_policy(&state),
        lockout_policy(&state),
        ctx.tenant_id,
        &req.username,
        &req.password,
        req.totp.as_deref(),
        &ip,
        &ua,
    )
    .await
    {
        Ok(r) => {
            state
                .audit
                .record_login(LoginLog::success(
                    tenant_id,
                    &r.principal.user_name,
                    Some(ip.clone()).filter(|s| s != "0.0.0.0"),
                    Some(ua.clone()).filter(|s| !s.is_empty()),
                ))
                .await;
            Ok((
                StatusCode::OK,
                Json(LoginResponse {
                    access_token: r.access_token,
                    refresh_token: r.refresh_token,
                    user_id: r.principal.user_id.get(),
                    tenant_id: r.principal.tenant_id.get(),
                    username: r.principal.user_name,
                }),
            ))
        }
        Err(e) => {
            state
                .audit
                .record_login(LoginLog::failure(
                    tenant_id,
                    &req.username,
                    Some(ip.clone()).filter(|s| s != "0.0.0.0"),
                    Some(ua.clone()).filter(|s| !s.is_empty()),
                    e.to_string(),
                ))
                .await;
            Err(map_auth_error(&ctx.locale, &e))
        }
    }
}

/// 图形验证码响应：`img` 为不带 data URI 前缀的 base64 PNG；
/// `enabled=false` 时 `captcha_id`/`img` 为空串（兼容既有客户端的 `captchaImage` 形态）。
#[derive(Serialize, ToSchema)]
pub struct CaptchaVo {
    pub captcha_id: String,
    pub img: String,
    pub enabled: bool,
}

#[utoipa::path(
    get,
    path = "/auth/captcha",
    responses((status = 200, description = "图形验证码", body = CaptchaVo))
)]
pub async fn captcha_image(
    State(state): State<AppState>,
    ctx: RequestContextExt,
) -> Result<(StatusCode, Json<CaptchaVo>), ApiErr> {
    let ctx = ctx.into_inner();
    if !state.captcha.params().enabled {
        return Ok((
            StatusCode::OK,
            Json(CaptchaVo {
                captcha_id: String::new(),
                img: String::new(),
                enabled: false,
            }),
        ));
    }
    let (captcha_id, img) = state
        .captcha
        .generate()
        .await
        .map_err(|e| map_auth_error(&ctx.locale, &e))?;
    Ok((
        StatusCode::OK,
        Json(CaptchaVo {
            captcha_id,
            img,
            enabled: true,
        }),
    ))
}

#[utoipa::path(
    post,
    path = "/auth/refresh",
    request_body = RefreshRequest,
    responses(
        (status = 200, description = "刷新成功", body = RefreshResponse),
        (status = 401, description = "刷新令牌无效或已被轮换", body = Problem)
    )
)]
pub async fn refresh(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<RefreshRequest>,
) -> Result<(StatusCode, Json<RefreshResponse>), ApiErr> {
    let ctx = ctx.into_inner();
    let ip = ctx.ip.to_string();
    let ua = ctx.user_agent.unwrap_or_default();
    match cicada_auth::refresh(
        &state.tokens,
        state.sessions.as_ref(),
        session_policy(&state),
        &req.refresh_token,
        &ip,
        &ua,
    )
    .await
    {
        Ok(r) => Ok((
            StatusCode::OK,
            Json(RefreshResponse {
                access_token: r.access_token,
                refresh_token: r.refresh_token,
            }),
        )),
        Err(e) => Err(map_auth_error(&ctx.locale, &e)),
    }
}

#[utoipa::path(
    post,
    path = "/auth/logout",
    responses(
        (status = 204, description = "已登出"),
        (status = 401, description = "未认证", body = Problem)
    )
)]
pub async fn logout(
    State(state): State<AppState>,
    ctx: RequestContextExt,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized(&ctx.locale));
    };
    match cicada_auth::logout(state.sessions.as_ref(), principal.session_id.as_str()).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(e) => Err(map_auth_error(&ctx.locale, &e)),
    }
}

/// `AuthError` → 业务码目录业务码 + 已本地化 `detail`。
///
/// 状态码由业务码给出，调用点只需 `Err(map_auth_error(..))`。相对旧的简化错误体，
/// 两处状态按业务码目录归位：TOTP 校验失败 401→422（`CICADA_AUTH_1006`）、身份重复绑定
/// 422→409（`CICADA_AUTH_1008`）。
pub(crate) fn map_auth_error(locale: &str, e: &AuthError) -> ApiErr {
    // 面向用户的消息走 rust-i18n；技术错误（Db/Session/Password）细节只进日志，
    // 对外给 `common.internal_error`——500 级不泄露内部信息。
    let (code, detail) = match e {
        AuthError::InvalidCredentials => (
            BizCode::AuthBadCredentials,
            t!("user.password.not.match", locale = locale).into_owned(),
        ),
        AuthError::Disabled => (
            BizCode::AuthDisabled,
            t!("user.blocked", locale = locale).into_owned(),
        ),
        AuthError::AccountLocked { retry_after_secs } => (
            BizCode::AuthLocked,
            t!(
                "user.password.retry.limit.exceed",
                locale = locale,
                seconds = *retry_after_secs
            )
            .into_owned(),
        ),
        AuthError::InvalidToken => (
            BizCode::AuthUnauthorized,
            t!("auth.invalid_token", locale = locale).into_owned(),
        ),
        AuthError::SessionExpired => (
            BizCode::AuthUnauthorized,
            t!("auth.session_expired", locale = locale).into_owned(),
        ),
        AuthError::RefreshReplay => (
            BizCode::AuthRefreshInvalid,
            t!("auth.refresh_replay", locale = locale).into_owned(),
        ),
        AuthError::TotpRequired => (
            BizCode::AuthUnauthorized,
            t!("auth.totp.required", locale = locale).into_owned(),
        ),
        AuthError::TotpInvalid => (
            BizCode::AuthTotpFailed,
            t!("auth.totp.invalid", locale = locale).into_owned(),
        ),
        AuthError::IdentityNotFound => (
            BizCode::SysNotFound,
            t!("auth.identity.not_found", locale = locale).into_owned(),
        ),
        AuthError::LastIdentity => (
            BizCode::AuthLastIdentity,
            t!("auth.identity.last", locale = locale).into_owned(),
        ),
        // 未启用 / 已启用属于状态冲突：业务码目录没有专用码，挂 1006 并保留 409 语义。
        AuthError::TotpNotEnabled => (
            BizCode::AuthTotpFailed,
            t!("auth.totp.not_enabled", locale = locale).into_owned(),
        ),
        AuthError::TotpAlreadyEnabled => (
            BizCode::AuthTotpFailed,
            t!("auth.totp.already_enabled", locale = locale).into_owned(),
        ),
        AuthError::DuplicateIdentity => (
            BizCode::AuthIdentityBound,
            t!("auth.identity.duplicate", locale = locale).into_owned(),
        ),
        AuthError::UnsupportedIdentityType => (
            BizCode::ValidationFields,
            t!("auth.identity.unsupported", locale = locale).into_owned(),
        ),
        AuthError::InvalidIdentifier => (
            BizCode::ValidationFields,
            t!("auth.identity.invalid", locale = locale).into_owned(),
        ),
        AuthError::VerificationFailed => (
            BizCode::ValidationFields,
            t!("auth.verification_failed", locale = locale).into_owned(),
        ),
        AuthError::Jwt(_) => (
            BizCode::AuthUnauthorized,
            t!("auth.invalid_token", locale = locale).into_owned(),
        ),
        AuthError::OAuthProviderNotFound(provider) => (
            BizCode::SysNotFound,
            t!(
                "auth.oauth.provider_not_found",
                locale = locale,
                provider = provider
            )
            .into_owned(),
        ),
        // state 失效可能是 CSRF 或过期，统一按未认证处理（401 语义）。
        AuthError::OAuthStateInvalid => (
            BizCode::AuthUnauthorized,
            t!("auth.oauth.state_invalid", locale = locale).into_owned(),
        ),
        AuthError::OAuthUpstream(detail) => (
            BizCode::AuthUpstreamUnavailable,
            t!("auth.oauth.upstream", locale = locale, detail = detail).into_owned(),
        ),
        other => {
            tracing::error!(error = %other, "认证内部错误");
            (
                BizCode::Internal,
                t!("common.internal_error", locale = locale).into_owned(),
            )
        }
    };
    let problem = Problem::new(code, detail);
    if matches!(e, AuthError::TotpNotEnabled | AuthError::TotpAlreadyEnabled) {
        problem.with_status(StatusCode::CONFLICT)
    } else {
        problem
    }
}

#[utoipa::path(
    get,
    path = "/auth/profile",
    responses(
        (status = 200, description = "当前用户信息", body = ProfileResponse),
        (status = 401, description = "未认证", body = Problem)
    )
)]
pub async fn profile(
    State(_state): State<AppState>,
    ctx: RequestContextExt,
) -> Result<Json<ProfileResponse>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized(&ctx.locale));
    };
    Ok(Json(ProfileResponse {
        user_id: principal.user_id.get(),
        user_name: principal.user_name.clone(),
        tenant_id: principal.tenant_id.get(),
        roles: ctx.roles.clone(),
        permissions: ctx
            .permissions
            .iter()
            .map(|p| p.as_str().to_string())
            .collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/auth/routers",
    responses(
        (status = 200, description = "当前用户可见的菜单树"),
        (status = 401, description = "未认证", body = Problem)
    )
)]
pub async fn routers(
    State(state): State<AppState>,
    ctx: RequestContextExt,
) -> Result<Json<Vec<cicada_system::MenuTreeNode>>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized(&ctx.locale));
    };
    let snapshot =
        load_grant_snapshot(state.db.conn(), principal.tenant_id, principal.user_id).await;
    let role_ids: Vec<RoleId> = snapshot.roles.iter().map(|r| r.role_id).collect();
    // 菜单树按「角色集合」缓存（缓存对象之一）：一次登录一份，翻页与刷新都复用。
    // 键里带着域版本号，所以菜单或角色变更时整域的版本一跳就够，不需要删键。
    let ident = cicada_system::roles_ident(&role_ids.iter().map(|r| r.get()).collect::<Vec<_>>());
    let tree = cicada_system::cached_json(
        &state.sys_cache,
        cicada_system::CacheDomain::Menu,
        None,
        &ident,
        || async { Some(load_menu_tree(state.db.conn(), &role_ids).await) },
    )
    .await
    .unwrap_or_default();
    Ok(Json(tree))
}

fn unauthorized(locale: &str) -> ApiErr {
    Problem::new(
        BizCode::AuthUnauthorized,
        t!("auth.unauthorized", locale = locale).into_owned(),
    )
}
