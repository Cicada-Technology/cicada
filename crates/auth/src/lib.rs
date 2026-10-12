//! cicada-auth（L3 能力）
//!
//! 认证：账号/手机/邮箱/微信身份解析与 JWT 签发校验。

pub mod captcha;
pub mod error;
pub mod identity;
pub mod lockout;
pub mod login;
pub mod oauth;
pub mod online;
pub mod password;
pub mod session;
pub mod token;
pub mod totp;

pub use captcha::{
    CaptchaParams, CaptchaService, CaptchaStore, InMemoryCaptchaStore, RedisCaptchaStore,
};
pub use error::AuthError;
pub use identity::{
    Identity, IdentityType, IdentityVerifier, RejectAllVerifier, StaticCodeVerifier, bind_identity,
    list_identities, unbind_identity,
};
pub use lockout::{
    InMemoryLockoutStore, LockState, LockoutPolicy, LockoutStore, RedisLockoutStore, lockout_key,
};
pub use login::{
    LoginResult, RefreshResult, SessionPolicy, canonical_identifier, login, logout, refresh,
};
pub use oauth::{
    ExternalIdentity, HttpOAuthClient, InMemoryOAuthStateStore, OAuthClient, OAuthLoginOutcome,
    OAuthService, OAuthStateStore, ProviderKind, RecordingOAuthClient, RedisOAuthStateStore,
    authorize_url, redirect_uri,
};
pub use online::{
    Model as UserOnline, OnlineSnapshot, ReconcileReport, Relation as UserOnlineRelation,
    SnapshottedSessionStore,
};
pub use password::{hash_password, verify_password};
pub use session::{InMemorySessionStore, RedisSessionStore, Session, SessionStore, hash_refresh};
pub use token::{BindingTicketClaims, TokenClaims, TokenService};
pub use totp::{TotpSetup, generate_setup as generate_totp_setup, verify as verify_totp};
