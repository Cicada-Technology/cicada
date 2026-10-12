//! OAuth2 / OIDC 登录（微信扫码登录、「通用 OIDC 放二期」）。
//!
//! 结构遵循  的缝纪律：**提供方协议在端口（`OAuthClient`）后面**，
//! 生产用 [`HttpOAuthClient`]，测试用 [`RecordingOAuthClient`]，编排（[`OAuthService`]）
//! 只依赖端口，因此整条「授权地址 → 回调换身份 → 已绑定发令牌 / 未绑定发票据 → 票据绑定」
//! 的流程都可以在没有真实 IdP 的情况下测通。
//!
//! 安全要点（ADR 0007）：
//! - `state` 服务端生成、写入 [`OAuthStateStore`]、**单次消费**并绑定到具体提供方（CSRF 防护）；
//! - 外部身份**不按邮箱自动合并**到已有账号：没绑定就只发短时效「绑定票据」，必须由已登录用户
//!   主动绑定，避免「邮箱相同即接管账号」；

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::Value;

use cicada_infra::Cache;
use cicada_infra::config::OAuthProviderConfig;
use cicada_kernel::{TenantId, UserId};

use crate::error::AuthError;
use crate::identity::{Identity, IdentityType, bind_identity};
use crate::login::{LoginResult, SessionPolicy};
use crate::session::{Session, SessionStore, hash_refresh};
use crate::token::TokenService;

/// 提供方协议类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// 微信开放平台网站应用（PC 扫码）：授权码 → `openid`/`unionid`。
    WeChat,
    /// 通用 OAuth2 / OIDC：授权码 → 访问令牌 → UserInfo。
    Oidc,
}

impl ProviderKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "wechat" => Some(Self::WeChat),
            "oidc" => Some(Self::Oidc),
            _ => None,
        }
    }

    fn default_scope(self) -> &'static str {
        match self {
            Self::WeChat => "snsapi_login",
            Self::Oidc => "openid profile email",
        }
    }

    fn default_identity_type(self) -> IdentityType {
        match self {
            Self::WeChat => IdentityType::WeChatOpen,
            Self::Oidc => IdentityType::Oidc,
        }
    }
}

/// 一次外部登录解析出的身份（尚未落到 `sys_user_identity`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalIdentity {
    pub identity_type: IdentityType,
    /// 外部标识：微信取 `unionid`（缺失回退 `openid`），OIDC 取 `{provider}:{sub}`。
    pub identifier: String,
    /// 提供方标识（配置里的 `code`）。
    pub provider: String,
    pub nickname: Option<String>,
    pub avatar: Option<String>,
    pub email: Option<String>,
}

/// 提供方协议端口。
#[async_trait]
pub trait OAuthClient: Send + Sync + 'static {
    /// 用授权码换外部身份（内部完成「换令牌 + 拉用户信息」）。
    async fn exchange(
        &self,
        provider: &OAuthProviderConfig,
        code: &str,
    ) -> Result<ExternalIdentity, AuthError>;
}

/// `state` 的一次性存储端口（生产 Redis，测试内存）。
#[async_trait]
pub trait OAuthStateStore: Send + Sync + 'static {
    /// 登记 `state` → 提供方，带 TTL。
    async fn issue(&self, state: &str, provider: &str, ttl_secs: u64) -> Result<(), AuthError>;
    /// 原子消费：`state` 存在且属于该提供方时返回 `true` 并删除（重放返回 `false`）。
    async fn consume(&self, state: &str, provider: &str) -> Result<bool, AuthError>;
}

// ---- Redis / 内存两个 adapter ----

pub struct RedisOAuthStateStore {
    cache: Cache,
}

impl RedisOAuthStateStore {
    pub fn new(cache: Cache) -> Self {
        Self { cache }
    }
}

fn state_key(state: &str) -> String {
    format!("oauth:state:{state}")
}

#[async_trait]
impl OAuthStateStore for RedisOAuthStateStore {
    async fn issue(&self, state: &str, provider: &str, ttl_secs: u64) -> Result<(), AuthError> {
        self.cache
            .set_ex(&state_key(state), provider, ttl_secs)
            .await
            .map_err(|e| AuthError::Session(e.to_string()))
    }

    async fn consume(&self, state: &str, provider: &str) -> Result<bool, AuthError> {
        self.cache
            .compare_and_delete(&state_key(state), provider)
            .await
            .map_err(|e| AuthError::Session(e.to_string()))
    }
}

/// 内存实现：单进程有效，供测试与单机开发使用。
#[derive(Default)]
pub struct InMemoryOAuthStateStore {
    states: Mutex<HashMap<String, (String, std::time::Instant)>>,
}

impl InMemoryOAuthStateStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl OAuthStateStore for InMemoryOAuthStateStore {
    async fn issue(&self, state: &str, provider: &str, ttl_secs: u64) -> Result<(), AuthError> {
        let expires = std::time::Instant::now() + Duration::from_secs(ttl_secs);
        self.states
            .lock()
            .expect("state 存储锁中毒")
            .insert(state.to_string(), (provider.to_string(), expires));
        Ok(())
    }

    async fn consume(&self, state: &str, provider: &str) -> Result<bool, AuthError> {
        let mut map = self.states.lock().expect("state 存储锁中毒");
        let Some((bound, expires)) = map.get(state).cloned() else {
            return Ok(false);
        };
        // 与 Redis adapter 的 `compare_and_delete` 语义一致：只在匹配时消费。
        if bound != provider {
            return Ok(false);
        }
        map.remove(state);
        Ok(expires > std::time::Instant::now())
    }
}

// ---- 生产 adapter：HTTP ----

/// 真实提供方调用（`reqwest`，rustls）。
pub struct HttpOAuthClient {
    http: reqwest::Client,
}

impl Default for HttpOAuthClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpOAuthClient {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("构建 OAuth HTTP 客户端失败"),
        }
    }

    /// 微信两步：`sns/oauth2/access_token` 换令牌 → `sns/userinfo` 取昵称。
    async fn exchange_wechat(
        &self,
        provider: &OAuthProviderConfig,
        code: &str,
    ) -> Result<ExternalIdentity, AuthError> {
        let token: Value = self
            .http
            .get(&provider.token_url)
            .query(&[
                ("appid", provider.client_id.as_str()),
                ("secret", provider.client_secret.expose_secret().as_str()),
                ("code", code),
                ("grant_type", "authorization_code"),
            ])
            .send()
            .await
            .map_err(|e| AuthError::OAuthUpstream(e.to_string()))?
            .json()
            .await
            .map_err(|e| AuthError::OAuthUpstream(e.to_string()))?;
        // 微信的错误是 HTTP 200 + errcode，必须显式判断。
        if let Some(err) = wechat_error(&token) {
            return Err(AuthError::OAuthUpstream(err));
        }
        let access_token = str_field(&token, "access_token")
            .ok_or_else(|| AuthError::OAuthUpstream("微信未返回 access_token".to_string()))?;
        let openid = str_field(&token, "openid")
            .ok_or_else(|| AuthError::OAuthUpstream("微信未返回 openid".to_string()))?;
        // unionid 优先：同一开放平台账号下多应用共用一个自然人标识。
        let identifier = str_field(&token, "unionid").unwrap_or_else(|| openid.clone());

        let userinfo: Value = self
            .http
            .get(&provider.userinfo_url)
            .query(&[
                ("access_token", access_token.as_str()),
                ("openid", openid.as_str()),
                ("lang", "zh_CN"),
            ])
            .send()
            .await
            .map_err(|e| AuthError::OAuthUpstream(e.to_string()))?
            .json()
            .await
            .map_err(|e| AuthError::OAuthUpstream(e.to_string()))?;
        if let Some(err) = wechat_error(&userinfo) {
            // 用户信息拉取失败不算致命：仍然可以按 openid 建立身份。
            tracing::warn!(error = %err, "微信 userinfo 拉取失败，仅用 openid 建身份");
        }
        Ok(ExternalIdentity {
            identity_type: self.identity_type(provider, ProviderKind::WeChat)?,
            identifier,
            provider: provider.code.clone(),
            nickname: str_field(&userinfo, "nickname"),
            avatar: str_field(&userinfo, "headimgurl"),
            email: None,
        })
    }

    /// 通用授权码流程：`token_url` 表单换令牌 → `userinfo_url` 带 Bearer 取用户信息。
    async fn exchange_oidc(
        &self,
        provider: &OAuthProviderConfig,
        code: &str,
    ) -> Result<ExternalIdentity, AuthError> {
        let token: Value = self
            .http
            .post(&provider.token_url)
            .form(&[
                ("grant_type", "authorization_code".to_string()),
                ("code", code.to_string()),
                ("redirect_uri", redirect_uri(provider)),
                ("client_id", provider.client_id.clone()),
                (
                    "client_secret",
                    provider.client_secret.expose_secret().clone(),
                ),
            ])
            .send()
            .await
            .map_err(|e| AuthError::OAuthUpstream(e.to_string()))?
            .json()
            .await
            .map_err(|e| AuthError::OAuthUpstream(e.to_string()))?;
        let access_token = str_field(&token, "access_token").ok_or_else(|| {
            AuthError::OAuthUpstream(format!(
                "令牌端点未返回 access_token：{}",
                error_hint(&token)
            ))
        })?;

        let userinfo: Value = self
            .http
            .get(&provider.userinfo_url)
            .bearer_auth(&access_token)
            .send()
            .await
            .map_err(|e| AuthError::OAuthUpstream(e.to_string()))?
            .json()
            .await
            .map_err(|e| AuthError::OAuthUpstream(e.to_string()))?;
        let sub = str_field(&userinfo, "sub")
            .ok_or_else(|| AuthError::OAuthUpstream("UserInfo 未返回 sub".to_string()))?;
        Ok(ExternalIdentity {
            identity_type: self.identity_type(provider, ProviderKind::Oidc)?,
            // 唯一索引是 `(tenant, identity_type, identifier)`，不含 provider，
            // 因此 OIDC 的标识要带提供方前缀（ADR 0007 的代价）。
            identifier: format!("{}:{}", provider.code, sub),
            provider: provider.code.clone(),
            nickname: str_field(&userinfo, "name")
                .or_else(|| str_field(&userinfo, "preferred_username"))
                .or_else(|| str_field(&userinfo, "nickname")),
            avatar: str_field(&userinfo, "picture"),
            email: str_field(&userinfo, "email"),
        })
    }

    fn identity_type(
        &self,
        provider: &OAuthProviderConfig,
        kind: ProviderKind,
    ) -> Result<IdentityType, AuthError> {
        match &provider.identity_type {
            Some(raw) => IdentityType::parse(raw)
                .ok_or_else(|| AuthError::OAuthUpstream(format!("未知身份类型：{raw}"))),
            None => Ok(kind.default_identity_type()),
        }
    }
}

#[async_trait]
impl OAuthClient for HttpOAuthClient {
    async fn exchange(
        &self,
        provider: &OAuthProviderConfig,
        code: &str,
    ) -> Result<ExternalIdentity, AuthError> {
        match ProviderKind::parse(&provider.kind) {
            Some(ProviderKind::WeChat) => self.exchange_wechat(provider, code).await,
            Some(ProviderKind::Oidc) => self.exchange_oidc(provider, code).await,
            None => Err(AuthError::OAuthProviderNotFound(provider.code.clone())),
        }
    }
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn error_hint(v: &Value) -> String {
    for key in ["error_description", "error", "errcode", "errmsg", "message"] {
        if let Some(s) = str_field(v, key) {
            return s;
        }
    }
    "未知错误".to_string()
}

fn wechat_error(v: &Value) -> Option<String> {
    match v.get("errcode").and_then(Value::as_i64) {
        Some(0) | None => None,
        Some(code) => Some(format!(
            "微信错误 {code}：{}",
            str_field(v, "errmsg").unwrap_or_default()
        )),
    }
}

// ---- 测试 adapter ----

/// 录制型 adapter：按授权码返回预置身份，并记录调用，供集成测试断言。
#[derive(Default)]
pub struct RecordingOAuthClient {
    identities: Vec<(String, ExternalIdentity)>,
    calls: Mutex<Vec<(String, String)>>,
}

impl RecordingOAuthClient {
    pub fn new(identities: Vec<(String, ExternalIdentity)>) -> Self {
        Self {
            identities,
            calls: Mutex::new(Vec::new()),
        }
    }

    /// 已发生的调用：(provider code, code)。
    pub fn calls(&self) -> Vec<(String, String)> {
        self.calls.lock().expect("调用记录锁中毒").clone()
    }
}

#[async_trait]
impl OAuthClient for RecordingOAuthClient {
    async fn exchange(
        &self,
        provider: &OAuthProviderConfig,
        code: &str,
    ) -> Result<ExternalIdentity, AuthError> {
        self.calls
            .lock()
            .expect("调用记录锁中毒")
            .push((provider.code.clone(), code.to_string()));
        self.identities
            .iter()
            .find(|(c, _)| c == code)
            .map(|(_, id)| id.clone())
            .ok_or_else(|| AuthError::OAuthUpstream(format!("未预置授权码 {code}")))
    }
}

// ---- 编排 ----

/// 授权地址 + 本次 `state`。
pub struct AuthorizeRequest {
    pub authorize_url: String,
    pub state: String,
}

/// 回调结果：已绑定直接发令牌，未绑定只发短时效绑定票据。
pub enum OAuthLoginOutcome {
    Bound {
        result: Box<LoginResult>,
        identity: ExternalIdentity,
    },
    Unbound {
        ticket: String,
        identity: ExternalIdentity,
    },
}

/// 第三方登录编排：解析提供方 → 校验 `state` → 换身份 → 查绑定 → 发令牌 / 发票据。
pub struct OAuthService {
    client: std::sync::Arc<dyn OAuthClient>,
    states: std::sync::Arc<dyn OAuthStateStore>,
    providers: Vec<OAuthProviderConfig>,
    state_ttl_secs: u64,
    ticket_ttl_secs: u64,
}

impl OAuthService {
    pub fn new(
        client: std::sync::Arc<dyn OAuthClient>,
        states: std::sync::Arc<dyn OAuthStateStore>,
        providers: Vec<OAuthProviderConfig>,
        state_ttl_secs: u64,
        ticket_ttl_secs: u64,
    ) -> Self {
        Self {
            client,
            states,
            providers,
            state_ttl_secs,
            ticket_ttl_secs,
        }
    }

    /// 未配置任何提供方：第三方登录接口一律 404（默认部署形态）。
    pub fn unconfigured() -> Self {
        Self::new(
            std::sync::Arc::new(RecordingOAuthClient::default()),
            std::sync::Arc::new(InMemoryOAuthStateStore::new()),
            Vec::new(),
            300,
            600,
        )
    }

    pub fn provider(&self, code: &str) -> Option<&OAuthProviderConfig> {
        self.providers.iter().find(|p| p.code == code)
    }

    pub fn ticket_ttl_secs(&self) -> u64 {
        self.ticket_ttl_secs
    }

    /// 生成授权地址并登记一次性 `state`。
    pub async fn authorize_url(&self, provider_code: &str) -> Result<AuthorizeRequest, AuthError> {
        let provider = self
            .provider(provider_code)
            .ok_or_else(|| AuthError::OAuthProviderNotFound(provider_code.to_string()))?;
        let state = uuid::Uuid::new_v4().simple().to_string();
        self.states
            .issue(&state, provider_code, self.state_ttl_secs)
            .await?;
        Ok(AuthorizeRequest {
            authorize_url: authorize_url(provider, &state)?,
            state,
        })
    }

    /// 回调：`state` 校验 → 换身份 → 已绑定发令牌 / 未绑定发票据。
    #[allow(clippy::too_many_arguments)]
    pub async fn login(
        &self,
        db: &DatabaseConnection,
        tokens: &TokenService,
        sessions: &dyn SessionStore,
        policy: SessionPolicy,
        tenant: TenantId,
        provider_code: &str,
        code: &str,
        state: &str,
        ip: &str,
        user_agent: &str,
    ) -> Result<OAuthLoginOutcome, AuthError> {
        let provider = self
            .provider(provider_code)
            .ok_or_else(|| AuthError::OAuthProviderNotFound(provider_code.to_string()))?;
        if !self.states.consume(state, provider_code).await? {
            return Err(AuthError::OAuthStateInvalid);
        }
        let identity = self.client.exchange(provider, code).await?;

        let Some(user_id) =
            find_identity_user(db, tenant, identity.identity_type, &identity.identifier).await?
        else {
            let ticket = tokens.sign_binding_ticket(
                tenant.get(),
                identity.identity_type.as_str(),
                &identity.provider,
                &identity.identifier,
                identity.nickname.as_deref(),
                self.ticket_ttl_secs,
            )?;
            return Ok(OAuthLoginOutcome::Unbound { ticket, identity });
        };

        let result = issue_session(
            db, tokens, sessions, policy, tenant, user_id, ip, user_agent,
        )
        .await?;
        Ok(OAuthLoginOutcome::Bound {
            result: Box::new(result),
            identity,
        })
    }

    /// 把绑定票据落到当前账号（已登录用户的自助绑定）。
    pub async fn bind(
        &self,
        db: &DatabaseConnection,
        tokens: &TokenService,
        tenant: TenantId,
        user_id: UserId,
        ticket: &str,
    ) -> Result<Identity, AuthError> {
        let claims = tokens.verify_binding_ticket(ticket)?;
        if claims.tid != tenant.get() {
            // 票据只能用在签发它的租户里（唯一键含 tenant）。
            return Err(AuthError::OAuthStateInvalid);
        }
        let identity_type = IdentityType::parse(&claims.itype)
            .ok_or_else(|| AuthError::OAuthUpstream(format!("未知身份类型：{}", claims.itype)))?;
        if identity_type == IdentityType::Username || identity_type.needs_verification() {
            return Err(AuthError::UnsupportedIdentityType);
        }
        bind_identity(
            db,
            tenant,
            user_id,
            identity_type,
            &claims.identifier,
            Some(&claims.provider),
            None,
            &crate::identity::RejectAllVerifier,
        )
        .await
    }
}

/// 回调地址：配置里显式给出的公开地址（必须是提供方控制台登记的那个）；
/// 留空时退化为相对路径，仅同源开发场景可用。
pub fn redirect_uri(provider: &OAuthProviderConfig) -> String {
    if provider.redirect_uri.is_empty() {
        format!("/auth/login/{}/callback", provider.code)
    } else {
        provider.redirect_uri.clone()
    }
}

/// 构造授权地址（`state` 必带，供提供方原样回传）。
pub fn authorize_url(provider: &OAuthProviderConfig, state: &str) -> Result<String, AuthError> {
    let kind = ProviderKind::parse(&provider.kind)
        .ok_or_else(|| AuthError::OAuthProviderNotFound(provider.code.clone()))?;
    let scope = if provider.scopes.is_empty() {
        kind.default_scope().to_string()
    } else {
        provider.scopes.join(" ")
    };
    // 微信的参数名是 `appid`，通用 OAuth2/OIDC 是 `client_id`。
    let id_param = match kind {
        ProviderKind::WeChat => "appid",
        ProviderKind::Oidc => "client_id",
    };
    let sep = if provider.authorize_url.contains('?') {
        '&'
    } else {
        '?'
    };
    let mut url = format!(
        "{}{sep}{id_param}={}&redirect_uri={}&response_type=code&scope={}&state={}",
        provider.authorize_url,
        urlencode(&provider.client_id),
        urlencode(&redirect_uri(provider)),
        urlencode(&scope),
        urlencode(state),
    );
    if kind == ProviderKind::WeChat {
        // 微信要求以该锚点结尾（扫码登录）。
        url.push_str("#wechat_redirect");
    }
    Ok(url)
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

async fn find_identity_user(
    db: &DatabaseConnection,
    tenant: TenantId,
    identity_type: IdentityType,
    identifier: &str,
) -> Result<Option<UserId>, AuthError> {
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT user_id FROM sys_user_identity \
         WHERE tenant_id = $1 AND identity_type = $2 AND identifier = $3 LIMIT 1",
        [
            tenant.get().into(),
            identity_type.as_str().into(),
            identifier.into(),
        ],
    );
    let rows = db
        .query_all_raw(stmt)
        .await
        .map_err(|e| AuthError::Db(e.to_string()))?;
    let Some(row) = rows.into_iter().next() else {
        return Ok(None);
    };
    let user_id: i64 = row
        .try_get_by_index(0)
        .map_err(|e| AuthError::Db(e.to_string()))?;
    Ok(Some(UserId::new(user_id)))
}

/// 与密码登录共用同一套会话/令牌逻辑（最大会话数、Redis 会话）。
#[allow(clippy::too_many_arguments)]
async fn issue_session(
    db: &DatabaseConnection,
    tokens: &TokenService,
    sessions: &dyn SessionStore,
    policy: SessionPolicy,
    tenant: TenantId,
    user_id: UserId,
    ip: &str,
    user_agent: &str,
) -> Result<LoginResult, AuthError> {
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT user_name, status FROM sys_user \
         WHERE tenant_id = $1 AND user_id = $2 AND deleted_at IS NULL LIMIT 1",
        [tenant.get().into(), user_id.get().into()],
    );
    let rows = db
        .query_all_raw(stmt)
        .await
        .map_err(|e| AuthError::Db(e.to_string()))?;
    let Some(row) = rows.into_iter().next() else {
        return Err(AuthError::IdentityNotFound);
    };
    let username: String = row
        .try_get_by_index(0)
        .map_err(|e| AuthError::Db(e.to_string()))?;
    let status: i16 = row
        .try_get_by_index(1)
        .map_err(|e| AuthError::Db(e.to_string()))?;
    if status != 0 {
        return Err(AuthError::Disabled);
    }

    if policy.max_sessions > 0 {
        let mut existing = sessions.list_user(user_id.get()).await?;
        if existing.len() as i64 >= policy.max_sessions {
            existing.sort_by_key(|(_, s)| s.login_time);
            let kick = existing.len() as i64 - policy.max_sessions + 1;
            for (jti, _) in existing.into_iter().take(kick as usize) {
                sessions.revoke(&jti).await?;
            }
        }
    }

    let session_id = uuid::Uuid::new_v4().to_string();
    let refresh_token = tokens.new_refresh_token(&session_id);
    let now = chrono::Utc::now().timestamp();
    sessions
        .create(
            &session_id,
            &Session {
                user_id: user_id.get(),
                tenant_id: tenant.get(),
                username: username.clone(),
                refresh_hash: hash_refresh(&refresh_token),
                ip: ip.to_string(),
                user_agent: user_agent.to_string(),
                login_time: now,
                last_seen: now,
            },
            policy.refresh_ttl_secs,
        )
        .await?;
    let access_token = tokens.sign_access(user_id.get(), tenant.get(), &session_id, &username)?;
    Ok(LoginResult {
        access_token,
        refresh_token,
        principal: cicada_kernel::Principal {
            user_id,
            tenant_id: tenant,
            session_id: cicada_kernel::SessionId(session_id),
            user_name: username,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cicada_infra::config::Secret;

    fn provider(kind: &str, code: &str) -> OAuthProviderConfig {
        OAuthProviderConfig {
            code: code.to_string(),
            kind: kind.to_string(),
            client_id: "cid".to_string(),
            client_secret: Secret::new("secret".to_string()),
            authorize_url: "https://idp.example/authorize".to_string(),
            token_url: "https://idp.example/token".to_string(),
            userinfo_url: "https://idp.example/userinfo".to_string(),
            redirect_uri: "https://api.example/auth/login/x/callback".to_string(),
            scopes: Vec::new(),
            identity_type: None,
        }
    }

    #[test]
    fn oidc_authorize_url_carries_state_and_scope() {
        let url = authorize_url(&provider("oidc", "keycloak"), "s-123").unwrap();
        assert!(url.starts_with("https://idp.example/authorize?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=cid"));
        assert!(url.contains("state=s-123"));
        assert!(url.contains("scope=openid%20profile%20email"));
        // redirect_uri 里的分隔符必须被编码。
        assert!(url.contains("redirect_uri="));
        assert!(!url.ends_with("#wechat_redirect"));
    }

    #[test]
    fn wechat_authorize_url_uses_appid_and_anchor() {
        let url = authorize_url(&provider("wechat", "wechat"), "s-1").unwrap();
        assert!(url.contains("appid=cid"));
        assert!(url.contains("scope=snsapi_login"));
        assert!(url.ends_with("#wechat_redirect"));
    }

    #[test]
    fn unknown_kind_is_rejected() {
        assert!(authorize_url(&provider("saml", "x"), "s").is_err());
    }

    #[tokio::test]
    async fn state_is_single_use_and_provider_bound() {
        let store = InMemoryOAuthStateStore::new();
        store.issue("s1", "wechat", 60).await.unwrap();
        // 换个提供方消费 → 失败，且不消费掉。
        assert!(!store.consume("s1", "keycloak").await.unwrap());
        assert!(store.consume("s1", "wechat").await.unwrap());
        // 重放 → 失败。
        assert!(!store.consume("s1", "wechat").await.unwrap());
    }

    #[tokio::test]
    async fn state_expires() {
        let store = InMemoryOAuthStateStore::new();
        store.issue("s2", "wechat", 0).await.unwrap();
        assert!(!store.consume("s2", "wechat").await.unwrap());
    }

    #[tokio::test]
    async fn authorize_url_registers_state_for_the_provider() {
        let client = std::sync::Arc::new(RecordingOAuthClient::default());
        let states = std::sync::Arc::new(InMemoryOAuthStateStore::new());
        let svc = OAuthService::new(
            client,
            states.clone(),
            vec![provider("oidc", "keycloak")],
            60,
            60,
        );
        let req = svc.authorize_url("keycloak").await.unwrap();
        assert!(req.authorize_url.contains(&req.state));
        // 未配置的提供方 → 404 语义的错误。
        assert!(matches!(
            svc.authorize_url("wechat").await,
            Err(AuthError::OAuthProviderNotFound(_))
        ));
    }

    #[test]
    fn urlencode_escapes_reserved_characters() {
        assert_eq!(urlencode("a b/c:d"), "a%20b%2Fc%3Ad");
        assert_eq!(urlencode("Az0-_.~"), "Az0-_.~");
    }
}
