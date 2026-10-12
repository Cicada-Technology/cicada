//! 认证/授权中间件：验 JWT → 装载授权 → 计算数据范围与权限 → 组装 `RequestContext`。
//!
//! 无令牌按匿名通过（`principal = None`），由具体 handler 决定是否拒绝；
//! 令牌无效直接 401。受保护接口用 `RequestContextExt` 抽取后自行检查。
//!
//! 授权计算经 `AuthzCache`：命中则不查库，未命中才装载快照并回填。

use std::net::{IpAddr, Ipv4Addr};

use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use cicada_auth::TokenClaims;
use cicada_kernel::{
    DataScope, Permission, Principal, RequestContext, RequestId, SessionId, TenantId, TraceId,
    UserId,
};
use cicada_rbac::authorize;
use cicada_system::{ApiKeyError, ApiKeyService, load_grant_snapshot};
use rust_i18n::t;

use crate::problem::{BizCode, Problem};
use crate::state::AppState;

/// 组装 `RequestContext` 需要的传输层事实（谁、从哪来、用什么语言应答）。
#[derive(Clone)]
struct ContextBase {
    request_id: RequestId,
    ip: IpAddr,
    user_agent: Option<String>,
    trace_id: TraceId,
    locale: String,
}

pub async fn auth_middleware(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let request_id = request_id(&req);
    let trace_id = TraceId::new(uuid::Uuid::new_v4().to_string());
    let ip = client_ip(&req);
    let user_agent = req
        .headers()
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    // 响应语言：中间件自身的错误体与下游 handler 共用同一协商结果。
    let locale = crate::i18n::negotiate_headers(req.headers());
    let base = ContextBase {
        request_id,
        ip,
        user_agent,
        trace_id,
        locale,
    };

    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").map(str::to_owned));
    // 开放接口凭证：`X-API-Key` 与 Bearer 二选一，Bearer 优先。
    let api_key = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    let ctx = match token {
        None => match api_key {
            Some(secret) => match api_key_context(&state, &secret, &base).await {
                Ok(ctx) => ctx,
                Err(ApiKeyAuthError::Unauthorized(msg)) => {
                    return error_response(&base, StatusCode::UNAUTHORIZED, &msg);
                }
                Err(ApiKeyAuthError::Unavailable) => {
                    return unavailable_response(
                        &base,
                        &t!("auth.api_key_unavailable", locale = base.locale.as_str()),
                    );
                }
            },
            None => RequestContext {
                principal: None,
                tenant_id: TenantId::new(state.config.tenant.default_id),
                data_scope: DataScope::none(),
                permissions: Vec::new(),
                roles: Vec::new(),
                is_super_admin: false,
                request_id: base.request_id,
                ip: base.ip,
                user_agent: base.user_agent,
                trace_id: base.trace_id,
                locale: base.locale,
            },
        },
        Some(token) => match state.tokens.verify_access(&token) {
            Ok(claims) => {
                let Ok(user_id) = claims.sub.parse::<i64>() else {
                    return error_response(
                        &base,
                        StatusCode::UNAUTHORIZED,
                        &t!("auth.invalid_token", locale = base.locale.as_str()),
                    );
                };
                // 会话以 Redis 为权威源：登出/强退后未过期的 access_token 立即失效。
                match state.sessions.get(&claims.jti).await {
                    Ok(Some(session)) => {
                        // 安全设计会话指纹：`user_agent` 变了说明拿这个令牌的人换了环境
                        // （被盗用、被复制到别的设备），撤销会话并要
                        // 求重新登录；IP 变化不算——移动网络下本来就一直在变。
                        if fingerprint::ua_changed(
                            Some(session.user_agent.as_str()),
                            base.user_agent.as_deref(),
                        ) {
                            tracing::warn!(
                                jti = %claims.jti,
                                user_id,
                                session_ua = %session.user_agent,
                                request_ua = %base.user_agent.clone().unwrap_or_default(),
                                "会话指纹不符，撤销会话并要求重新登录"
                            );
                            let _ = state.sessions.revoke(&claims.jti).await;
                            return reauth_response(&base);
                        }
                        let request_ip = base.ip.to_string();
                        if fingerprint::ip_changed(
                            Some(session.ip.as_str()),
                            Some(request_ip.as_str()),
                        ) {
                            // 只记录不重绑：重绑要拿剩余 TTL 重写会话，而存储侧没有
                            // 「读改写 + 保持 TTL」的原语；登录 IP 留在快照里也正是审计要的那个数。
                            tracing::info!(
                                jti = %claims.jti,
                                session_ip = %session.ip,
                                request_ip = %base.ip,
                                "会话 IP 变化，按移动网络常态放行"
                            );
                        }
                    }
                    Ok(None) => {
                        return error_response(
                            &base,
                            StatusCode::UNAUTHORIZED,
                            &t!("auth.session_expired", locale = base.locale.as_str()),
                        );
                    }
                    Err(e) => {
                        return unavailable_response(
                            &base,
                            &t!(
                                "auth.session_store_unavailable",
                                locale = base.locale.as_str(),
                                detail = e
                            ),
                        );
                    }
                }
                authorize_context(&state, &claims, UserId::new(user_id), base).await
            }
            Err(_) => {
                return error_response(
                    &base,
                    StatusCode::UNAUTHORIZED,
                    &t!("auth.invalid_token", locale = base.locale.as_str()),
                );
            }
        },
    };

    // 租户维度限流（套餐 `api_qps_limit`）：需要身份，故放在算完上下文之后；
    // 探测端点豁免（与 IP 维度一致）。
    if !crate::ratelimit::is_exempt(req.uri().path())
        && let Err(limited) = state
            .rate_limit
            .check_tenant(ctx.tenant_id, &state.quota)
            .await
    {
        return crate::ratelimit::limited_response(limited, &ctx.locale);
    }

    req.extensions_mut().insert(ctx);
    next.run(req).await
}

/// `X-API-Key` 认证的失败原因（保持 `Err` 小尺寸，不把整个 `Response` 塞进 `Result`）。
enum ApiKeyAuthError {
    Unauthorized(String),
    Unavailable,
}

/// `X-API-Key` 认证：凭证自带租户与权限集合，**不建立会话**（无状态）。
///
/// 凭证主体是租户级主体（`user_id = 0`），数据范围固定为「租户内全部」；
/// 权限集合取凭证的 `scopes`，因此受同样的路由权限守卫约束。
async fn api_key_context(
    state: &AppState,
    secret: &str,
    base: &ContextBase,
) -> Result<RequestContext, ApiKeyAuthError> {
    let locale = &base.locale;
    let key = match state.api_keys.verify(secret).await {
        Ok(Some(key)) => key,
        Ok(None) => {
            return Err(ApiKeyAuthError::Unauthorized(
                t!("auth.api_key_invalid", locale = base.locale.as_str()).into_owned(),
            ));
        }
        Err(ApiKeyError::Disabled(name)) => {
            return Err(ApiKeyAuthError::Unauthorized(
                t!(
                    "auth.api_key_disabled",
                    locale = base.locale.as_str(),
                    name = name
                )
                .into_owned(),
            ));
        }
        Err(ApiKeyError::Expired(name)) => {
            return Err(ApiKeyAuthError::Unauthorized(
                t!(
                    "auth.api_key_expired",
                    locale = base.locale.as_str(),
                    name = name
                )
                .into_owned(),
            ));
        }
        Err(e) => {
            tracing::error!(error = %e, "API Key 校验失败");
            return Err(ApiKeyAuthError::Unavailable);
        }
    };

    // 最近使用时间异步回写，失败不影响请求（touch_used 内部只记日志）。
    state.api_keys.touch_used(key.id).await;

    let tenant_id = TenantId::new(key.tenant_id);
    let permissions: Vec<Permission> = ApiKeyService::scopes_of(&key)
        .into_iter()
        .map(Permission::new)
        .collect();

    Ok(RequestContext {
        request_id: base.request_id.clone(),
        principal: Some(Principal {
            user_id: UserId::new(ApiKeyService::SUBJECT_USER_ID),
            tenant_id,
            session_id: SessionId(format!("apikey:{}", key.id)),
            user_name: key.name.clone(),
        }),
        tenant_id,
        data_scope: DataScope::all(),
        permissions,
        roles: Vec::new(),
        is_super_admin: false,
        ip: base.ip,
        user_agent: base.user_agent.clone(),
        trace_id: base.trace_id.clone(),
        locale: locale.clone(),
    })
}

async fn authorize_context(
    state: &AppState,
    claims: &TokenClaims,
    user_id: UserId,
    base: ContextBase,
) -> RequestContext {
    let ContextBase {
        request_id,
        ip,
        user_agent,
        trace_id,
        locale,
    } = base;
    let tenant_id = TenantId::new(claims.tid);

    let authz = state
        .authz
        .load_or_compute(tenant_id, user_id, || async {
            let snapshot = load_grant_snapshot(state.db.conn(), tenant_id, user_id).await;
            authorize(
                tenant_id,
                user_id,
                snapshot.user_dept,
                &snapshot.roles,
                snapshot.permissions,
                state.dept_directory.as_ref(),
            )
            .await
        })
        .await;

    RequestContext {
        request_id,
        principal: Some(Principal {
            user_id,
            tenant_id,
            session_id: SessionId(claims.jti.clone()),
            user_name: claims.username.clone(),
        }),
        tenant_id,
        data_scope: authz.data_scope,
        permissions: authz.permissions.into_vec(),
        roles: authz.role_keys,
        is_super_admin: authz.is_super_admin,
        ip,
        user_agent,
        trace_id,
        locale,
    }
}

fn request_id(req: &Request) -> RequestId {
    if let Some(v) = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
    {
        RequestId::new(v)
    } else {
        RequestId::new(uuid::Uuid::new_v4().to_string())
    }
}

/// 客户端 IP：优先 TCP 对端（`ConnectInfo`），其次反代注入的 `X-Forwarded-For` 首个地址与
/// `X-Real-IP`；都没有时退化为未指定地址（此时全站共享一个 IP 限流桶）。
///
/// 部署在可信反代之后时按上述头取用；直连公网且未挂 `ConnectInfo` 的场景下这些头可被伪造，
/// 需由网关改写（ADR 0005）。
pub(crate) fn client_ip(req: &Request) -> IpAddr {
    if let Some(connect) = req
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
    {
        return connect.0.ip();
    }
    if let Some(ip) = header_ip(req, "x-forwarded-for", true) {
        return ip;
    }
    if let Some(ip) = header_ip(req, "x-real-ip", false) {
        return ip;
    }
    IpAddr::V4(Ipv4Addr::UNSPECIFIED)
}

/// 逗号分隔的头（`X-Forwarded-For`）取第一个地址。
fn header_ip(req: &Request, name: &'static str, first_of_list: bool) -> Option<IpAddr> {
    let raw = req.headers().get(name)?.to_str().ok()?;
    let raw = if first_of_list {
        raw.split(',').next()?.trim()
    } else {
        raw.trim()
    };
    raw.parse().ok()
}

/// 会话指纹判定：会话绑定 `(ip, user_agent)`，指纹异常变化时要求重新认证，
/// 而不是直接拒绝。
///
/// 两个函数都吃 `&str` 而不是 `&Session`：中间件里拿到的是已经解出来的字段，
/// 让判定只看字符串才留得住单测（不必造会话与存储）。
mod fingerprint {
    /// 浏览器/设备换了：两边都非空且不相等才算。任一边为空就**不判**——
    /// 有些客户端不发 UA，缺信息不能当成攻击证据。
    pub fn ua_changed(session_ua: Option<&str>, request_ua: Option<&str>) -> bool {
        match (session_ua, request_ua) {
            (Some(a), Some(b)) => !a.is_empty() && !b.is_empty() && a != b,
            _ => false,
        }
    }

    /// IP 换了：同样要求两边都有值。移动网络下这是常态，只用于记日志。
    pub fn ip_changed(session_ip: Option<&str>, request_ip: Option<&str>) -> bool {
        match (session_ip, request_ip) {
            (Some(a), Some(b)) => !a.is_empty() && !b.is_empty() && a != b,
            _ => false,
        }
    }
}

/// 指纹不符：撤销会话后回 401 + `CICADA_AUTH_1011`，前端据此跳登录页（统一响应形状）。
fn reauth_response(base: &ContextBase) -> Response {
    with_request_id(
        base,
        Problem::new(
            BizCode::AuthReauthRequired,
            rust_i18n::t!("auth.reauth_required", locale = base.locale.as_str()).into_owned(),
        ),
    )
    .into_response()
}

fn error_response(base: &ContextBase, status: StatusCode, msg: impl std::fmt::Display) -> Response {
    with_request_id(base, Problem::from_status(status, msg.to_string())).into_response()
}

/// 依赖故障（会话存储 / 键值存储不可用）：业务码目录归 `CICADA_INTERNAL_9003`，
/// 不是数据库故障（9002），也不能按请求错误处理。
fn unavailable_response(base: &ContextBase, msg: impl std::fmt::Display) -> Response {
    with_request_id(
        base,
        Problem::new(BizCode::CacheUnavailable, msg.to_string()),
    )
    .into_response()
}

/// 接入层自产的错误体一律带上本次请求的 `request_id`，前端可与日志对齐。
fn with_request_id(base: &ContextBase, problem: Problem) -> Problem {
    problem.request_id(base.request_id.0.clone())
}

#[cfg(test)]
mod fingerprint_tests {
    use super::fingerprint::*;

    #[test]
    fn different_user_agent_is_a_mismatch() {
        assert!(ua_changed(
            Some("Mozilla/5.0 Firefox"),
            Some("Mozilla/5.0 Chrome")
        ));
        assert!(!ua_changed(Some("Same UA"), Some("Same UA")));
    }

    #[test]
    fn missing_information_is_never_evidence() {
        // 有些客户端不发 UA；把「缺少可比信息」当成攻击信号就是安全设计警告的硬绑定。
        assert!(!ua_changed(Some("Firefox"), None));
        assert!(!ua_changed(Some("Firefox"), Some("")));
        assert!(!ua_changed(None, Some("Firefox")));
        assert!(!ip_changed(Some("1.1.1.1"), None));
        assert!(!ip_changed(Some(""), Some("1.1.1.1")));
    }

    #[test]
    fn ip_change_is_detected_but_only_for_logging() {
        assert!(ip_changed(Some("10.0.0.1"), Some("10.0.0.9")));
        assert!(!ip_changed(Some("10.0.0.1"), Some("10.0.0.1")));
    }
}
