//! 登录身份绑定（一账号多身份）。
//!
//! `sys_user` 是账号，`sys_user_identity` 是它可用来登录的身份，一对多；
//! 唯一键 `(tenant_id, identity_type, identifier)` 保证租户内一个手机/邮箱/微信
//! 只能绑到一个账号。`sys_user.user_name` / `phonenumber` / `email` 是同类型
//! **主身份**（`is_primary`）的镜像列，在同一事务语义内同步。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

use cicada_kernel::{TenantId, UserId};

use crate::error::AuthError;

/// 身份类型（与 对齐）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityType {
    /// 账号名，与参考实现 `user_name` 对齐；建号时写入，不开放自助绑定。
    Username,
    Phone,
    Email,
    /// 微信开放平台扫码（PC）。
    WeChatOpen,
    /// 微信小程序。
    WeChatMp,
    /// 通用 OIDC / OAuth2 提供方（通用 OIDC 放二期）。
    Oidc,
}

impl IdentityType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Username => "Username",
            Self::Phone => "Phone",
            Self::Email => "Email",
            Self::WeChatOpen => "WeChatOpen",
            Self::WeChatMp => "WeChatMp",
            Self::Oidc => "Oidc",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "Username" => Self::Username,
            "Phone" => Self::Phone,
            "Email" => Self::Email,
            "WeChatOpen" => Self::WeChatOpen,
            "WeChatMp" => Self::WeChatMp,
            "Oidc" => Self::Oidc,
            _ => return None,
        })
    }

    /// 是否需要验证码 / OAuth 回调证明归属。
    pub fn needs_verification(self) -> bool {
        matches!(self, Self::Phone | Self::Email)
    }

    /// 是否有镜像列需要同步。
    fn mirror_column(self) -> Option<&'static str> {
        match self {
            Self::Phone => Some("phonenumber"),
            Self::Email => Some("email"),
            _ => None,
        }
    }

    pub fn normalize(self, identifier: &str) -> String {
        let t = identifier.trim();
        match self {
            Self::Phone => t.chars().filter(|c| !matches!(c, ' ' | '-')).collect(),
            Self::Email => t.to_lowercase(),
            _ => t.to_string(),
        }
    }
}

/// 一条已绑定的身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub identity_id: i64,
    pub tenant_id: i64,
    pub user_id: i64,
    pub identity_type: String,
    pub identifier: String,
    pub provider: Option<String>,
    pub verified_at: Option<DateTime<Utc>>,
    pub is_primary: bool,
}

/// 绑定新身份时的归属校验缝。短信/邮件通道是可选扩展，
/// 未接入时用 [`RejectAllVerifier`] 失败关闭。
#[async_trait]
pub trait IdentityVerifier: Send + Sync + 'static {
    async fn verify(&self, identity_type: IdentityType, identifier: &str, code: &str) -> bool;
}

/// 生产默认：未配置通道，一律拒绝绑定。
#[derive(Default, Clone, Copy)]
pub struct RejectAllVerifier;

#[async_trait]
impl IdentityVerifier for RejectAllVerifier {
    async fn verify(&self, _t: IdentityType, _identifier: &str, _code: &str) -> bool {
        false
    }
}

/// 开发 / 测试用固定验证码校验器（上线前替换为真实通道）。
#[derive(Clone)]
pub struct StaticCodeVerifier {
    code: String,
}

impl StaticCodeVerifier {
    pub fn new(code: impl Into<String>) -> Self {
        Self { code: code.into() }
    }
}

#[async_trait]
impl IdentityVerifier for StaticCodeVerifier {
    async fn verify(&self, _t: IdentityType, _identifier: &str, code: &str) -> bool {
        !self.code.is_empty() && code == self.code
    }
}

fn db_err(e: sea_orm::DbErr) -> AuthError {
    AuthError::Db(e.to_string())
}

fn row_identity(row: &sea_orm::QueryResult) -> Result<Identity, AuthError> {
    Ok(Identity {
        identity_id: row.try_get_by_index(0).map_err(db_err)?,
        tenant_id: row.try_get_by_index(1).map_err(db_err)?,
        user_id: row.try_get_by_index(2).map_err(db_err)?,
        identity_type: row.try_get_by_index(3).map_err(db_err)?,
        identifier: row.try_get_by_index(4).map_err(db_err)?,
        provider: row.try_get_by_index(5).map_err(db_err)?,
        verified_at: row.try_get_by_index(6).map_err(db_err)?,
        is_primary: row.try_get_by_index(7).map_err(db_err)?,
    })
}

const SELECT_COLS: &str = "identity_id, tenant_id, user_id, identity_type, identifier, \
                            provider, verified_at, is_primary";

/// 列出某账号的全部已绑定身份。
pub async fn list_identities(
    db: &DatabaseConnection,
    tenant: TenantId,
    user_id: UserId,
) -> Result<Vec<Identity>, AuthError> {
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!(
            "SELECT {SELECT_COLS} FROM sys_user_identity \
             WHERE tenant_id = $1 AND user_id = $2 ORDER BY identity_id"
        ),
        [tenant.get().into(), user_id.get().into()],
    );
    db.query_all_raw(stmt)
        .await
        .map_err(db_err)?
        .iter()
        .map(row_identity)
        .collect()
}

async fn sync_mirror(
    db: &DatabaseConnection,
    tenant: TenantId,
    user_id: UserId,
    identity_type: IdentityType,
    identifier: Option<&str>,
) -> Result<(), AuthError> {
    let Some(column) = identity_type.mirror_column() else {
        return Ok(());
    };
    let value = match identifier {
        Some(v) => sea_orm::Value::String(Some(v.to_string())),
        None => sea_orm::Value::String(None),
    };
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!(
            "UPDATE sys_user SET {column} = $1, update_time = now() \
             WHERE tenant_id = $2 AND user_id = $3"
        ),
        [value, tenant.get().into(), user_id.get().into()],
    );
    db.execute_raw(stmt).await.map_err(db_err)?;
    Ok(())
}

/// 绑定一个新身份。手机/邮箱需通过 `verifier`；已存在的同一身份返回
/// [`AuthError::DuplicateIdentity`]；首次绑定的类型成为主身份并同步镜像列。
#[allow(clippy::too_many_arguments)]
pub async fn bind_identity(
    db: &DatabaseConnection,
    tenant: TenantId,
    user_id: UserId,
    identity_type: IdentityType,
    identifier: &str,
    provider: Option<&str>,
    code: Option<&str>,
    verifier: &dyn IdentityVerifier,
) -> Result<Identity, AuthError> {
    if identity_type == IdentityType::Username {
        return Err(AuthError::UnsupportedIdentityType);
    }
    let identifier = identity_type.normalize(identifier);
    if identifier.is_empty() {
        return Err(AuthError::InvalidIdentifier);
    }
    if identity_type.needs_verification() {
        let ok = matches!(code, Some(c) if verifier.verify(identity_type, &identifier, c).await);
        if !ok {
            return Err(AuthError::VerificationFailed);
        }
    }

    let exists = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT 1 FROM sys_user_identity \
         WHERE tenant_id = $1 AND identity_type = $2 AND identifier = $3",
        [
            tenant.get().into(),
            identity_type.as_str().into(),
            identifier.clone().into(),
        ],
    );
    if !db.query_all_raw(exists).await.map_err(db_err)?.is_empty() {
        return Err(AuthError::DuplicateIdentity);
    }

    let count_stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT count(*) FROM sys_user_identity \
         WHERE tenant_id = $1 AND user_id = $2 AND identity_type = $3",
        [
            tenant.get().into(),
            user_id.get().into(),
            identity_type.as_str().into(),
        ],
    );
    let rows = db.query_all_raw(count_stmt).await.map_err(db_err)?;
    let same_type: i64 = rows[0].try_get_by_index(0).map_err(db_err)?;
    let is_primary = same_type == 0;

    let insert = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!(
            "INSERT INTO sys_user_identity \
             (tenant_id, user_id, identity_type, identifier, provider, verified_at, is_primary, create_time) \
             VALUES ($1, $2, $3, $4, $5, now(), $6, now()) RETURNING {SELECT_COLS}"
        ),
        [
            tenant.get().into(),
            user_id.get().into(),
            identity_type.as_str().into(),
            identifier.clone().into(),
            provider.map(|p| p.to_string()).into(),
            is_primary.into(),
        ],
    );
    let row = db
        .query_all_raw(insert)
        .await
        .map_err(db_err)?
        .into_iter()
        .next()
        .ok_or(AuthError::IdentityNotFound)?;
    let identity = row_identity(&row)?;

    if is_primary {
        sync_mirror(db, tenant, user_id, identity_type, Some(&identifier)).await?;
    }
    Ok(identity)
}

/// 解绑身份；必须为账号保留至少一个可登录身份。解绑主身份时把同类型
/// 最早的一条提升为主身份并同步镜像列。
pub async fn unbind_identity(
    db: &DatabaseConnection,
    tenant: TenantId,
    user_id: UserId,
    identity_id: i64,
) -> Result<(), AuthError> {
    let find = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT identity_type, is_primary FROM sys_user_identity \
         WHERE identity_id = $1 AND tenant_id = $2 AND user_id = $3",
        [
            identity_id.into(),
            tenant.get().into(),
            user_id.get().into(),
        ],
    );
    let Some(row) = db
        .query_all_raw(find)
        .await
        .map_err(db_err)?
        .into_iter()
        .next()
    else {
        return Err(AuthError::IdentityNotFound);
    };
    let identity_type_raw: String = row.try_get_by_index(0).map_err(db_err)?;
    let was_primary: bool = row.try_get_by_index(1).map_err(db_err)?;
    let identity_type =
        IdentityType::parse(&identity_type_raw).ok_or(AuthError::UnsupportedIdentityType)?;

    let count_stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT count(*) FROM sys_user_identity WHERE tenant_id = $1 AND user_id = $2",
        [tenant.get().into(), user_id.get().into()],
    );
    let total: i64 = db.query_all_raw(count_stmt).await.map_err(db_err)?[0]
        .try_get_by_index(0)
        .map_err(db_err)?;
    if total <= 1 {
        return Err(AuthError::LastIdentity);
    }

    let delete = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "DELETE FROM sys_user_identity WHERE identity_id = $1 AND tenant_id = $2 AND user_id = $3",
        [
            identity_id.into(),
            tenant.get().into(),
            user_id.get().into(),
        ],
    );
    db.execute_raw(delete).await.map_err(db_err)?;

    if was_primary {
        let promote = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            format!(
                "UPDATE sys_user_identity SET is_primary = true \
                 WHERE identity_id = (SELECT identity_id FROM sys_user_identity \
                    WHERE tenant_id = $1 AND user_id = $2 AND identity_type = $3 \
                    ORDER BY identity_id LIMIT 1) RETURNING {SELECT_COLS}"
            ),
            [
                tenant.get().into(),
                user_id.get().into(),
                identity_type.as_str().into(),
            ],
        );
        match db
            .query_all_raw(promote)
            .await
            .map_err(db_err)?
            .into_iter()
            .next()
        {
            Some(row) => {
                let promoted = row_identity(&row)?;
                sync_mirror(
                    db,
                    tenant,
                    user_id,
                    identity_type,
                    Some(&promoted.identifier),
                )
                .await?;
            }
            None => sync_mirror(db, tenant, user_id, identity_type, None).await?,
        }
    }
    Ok(())
}
