//! 认证错误（映射到 的 `CICADA_AUTH_*`）。

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("账号或密码错误")]
    InvalidCredentials,
    #[error("账号已停用")]
    Disabled,
    #[error("账号已锁定，请 {retry_after_secs} 秒后重试")]
    AccountLocked { retry_after_secs: u64 },
    #[error("令牌无效或过期")]
    InvalidToken,
    #[error("会话已失效")]
    SessionExpired,
    #[error("刷新令牌已被使用，会话链已撤销")]
    RefreshReplay,
    #[error("会话存储错误：{0}")]
    Session(String),
    #[error("密码错误：{0}")]
    Password(String),
    #[error("JWT 错误：{0}")]
    Jwt(#[from] jsonwebtoken::errors::Error),
    /// 密钥配置本身不合法（PEM 解析不了、算法不认识）。这是**部署错误**，
    /// 与「令牌无效」必须分开：前者该在启动时炸掉，后者是每次请求的正常拒绝。
    #[error("JWT 配置错误：{0}")]
    Config(String),
    #[error("数据库错误：{0}")]
    Db(String),
    #[error("身份不存在")]
    IdentityNotFound,
    #[error("必须保留至少一个可登录身份")]
    LastIdentity,
    #[error("该身份已被绑定")]
    DuplicateIdentity,
    #[error("该身份类型不支持自助绑定")]
    UnsupportedIdentityType,
    #[error("身份标识非法")]
    InvalidIdentifier,
    #[error("验证码错误或已过期")]
    VerificationFailed,
    #[error("需要二次验证码")]
    TotpRequired,
    #[error("二次验证码错误")]
    TotpInvalid,
    #[error("未启用二次验证")]
    TotpNotEnabled,
    #[error("二次验证已启用")]
    TotpAlreadyEnabled,
    #[error("未知的登录提供方：{0}")]
    OAuthProviderNotFound(String),
    #[error("授权 state 无效或已过期")]
    OAuthStateInvalid,
    #[error("第三方登录上游失败：{0}")]
    OAuthUpstream(String),
}
