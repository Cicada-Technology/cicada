//! 登录 / 刷新 / 登出。
//!
//! 会话的权威在 Redis（`SessionStore`）：登录写入、刷新轮换、登出撤销。
//! 刷新令牌一次一换，旧令牌复用即判定重放并撤销该用户整条会话链。

use std::time::{SystemTime, UNIX_EPOCH};

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

use cicada_kernel::{Principal, SessionId, TenantId, UserId};

use crate::error::AuthError;
use crate::lockout::{LockoutPolicy, LockoutStore};
use crate::password::verify_password;
use crate::session::{Session, SessionStore, hash_refresh};
use crate::token::TokenService;

/// 会话策略（对应配置 `auth.refresh_ttl_days` / `auth.max_sessions`）。
#[derive(Clone, Copy, Debug)]
pub struct SessionPolicy {
    pub refresh_ttl_secs: u64,
    /// 单账号最大会话数，`< 0` 表示不限。
    pub max_sessions: i64,
}

pub struct LoginResult {
    pub access_token: String,
    pub refresh_token: String,
    pub principal: Principal,
}

pub struct RefreshResult {
    pub access_token: String,
    pub refresh_token: String,
    pub principal: Principal,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 规范化「登录标识符」：邮箱转小写，手机去掉分隔符，其余原样。
pub fn canonical_identifier(raw: &str) -> String {
    let t = raw.trim();
    if t.contains('@') {
        t.to_lowercase()
    } else if t.chars().any(|c| c.is_ascii_digit())
        && t.chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '+' | ' ' | '-'))
    {
        t.chars()
            .filter(|c| c.is_ascii_digit() || *c == '+')
            .collect()
    } else {
        t.to_string()
    }
}

/// 按标识符登录（账号名 / 手机 / 邮箱）。查不到与密码错返回同一个错误，避免账号枚举。
///
/// 查找以 `sys_user_identity` 为准，`user_name` 作为兼容回退；一律限定在 `tenant` 内。
/// 启用 TOTP 的账号在密码校验通过后必须携带正确的 `totp`。
#[allow(clippy::too_many_arguments)]
pub async fn login(
    db: &DatabaseConnection,
    tokens: &TokenService,
    sessions: &dyn SessionStore,
    lockout: &dyn LockoutStore,
    policy: SessionPolicy,
    lockout_policy: LockoutPolicy,
    tenant: TenantId,
    identifier: &str,
    password: &str,
    totp_code: Option<&str>,
    ip: &str,
    user_agent: &str,
) -> Result<LoginResult, AuthError> {
    let canonical = canonical_identifier(identifier);
    let lock_key = crate::lockout::lockout_key(tenant.get(), &canonical);
    if let Some(retry_after_secs) = lockout.locked_secs(&lock_key).await? {
        return Err(AuthError::AccountLocked { retry_after_secs });
    }
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT u.user_id, u.tenant_id, u.user_name, u.password, u.status, \
                u.totp_enabled, u.totp_secret \
         FROM sys_user u \
         WHERE u.deleted_at IS NULL AND u.tenant_id = $2 \
           AND (u.user_name = $1 OR EXISTS ( \
                 SELECT 1 FROM sys_user_identity i \
                 WHERE i.user_id = u.user_id AND i.tenant_id = u.tenant_id \
                   AND i.identifier = $1 \
                   AND i.identity_type IN ('Username','Phone','Email'))) \
         LIMIT 1",
        [canonical.into(), tenant.get().into()],
    );
    let rows = db
        .query_all_raw(stmt)
        .await
        .map_err(|e| AuthError::Db(e.to_string()))?;
    let Some(row) = rows.into_iter().next() else {
        return Err(reject_credentials(lockout, &lock_key, lockout_policy).await);
    };

    let user_id: i64 = row
        .try_get_by_index(0)
        .map_err(|e| AuthError::Db(e.to_string()))?;
    let tenant_id: i64 = row
        .try_get_by_index(1)
        .map_err(|e| AuthError::Db(e.to_string()))?;
    let db_username: String = row
        .try_get_by_index(2)
        .map_err(|e| AuthError::Db(e.to_string()))?;
    let pw_hash: String = row
        .try_get_by_index(3)
        .map_err(|e| AuthError::Db(e.to_string()))?;
    let status: i16 = row
        .try_get_by_index(4)
        .map_err(|e| AuthError::Db(e.to_string()))?;
    let totp_enabled: bool = row
        .try_get_by_index(5)
        .map_err(|e| AuthError::Db(e.to_string()))?;
    let totp_secret: Option<String> = row
        .try_get_by_index(6)
        .map_err(|e| AuthError::Db(e.to_string()))?;

    if status != 0 {
        return Err(AuthError::Disabled);
    }
    if !verify_password(password, &pw_hash)? {
        return Err(reject_credentials(lockout, &lock_key, lockout_policy).await);
    }
    // 哈希升级：参考实现搬迁来的 BCrypt 在登录成功后按需升级为 Argon2id。
    if crate::password::needs_rehash(&pw_hash) {
        upgrade_password(db, user_id, password).await;
    }
    // 二次验证：密码正确后才检查，避免向未认证者泄露 TOTP 状态。
    if totp_enabled {
        let secret = totp_secret.ok_or(AuthError::TotpInvalid)?;
        match totp_code {
            Some(code) if crate::totp::verify(&secret, code) => {}
            Some(_) => return Err(AuthError::TotpInvalid),
            None => return Err(AuthError::TotpRequired),
        }
    }

    // 登录成功：清空失败计数。
    let _ = lockout.clear(&lock_key).await;

    // 最大会话数：超出时先踢掉最旧的会话。
    if policy.max_sessions > 0 {
        let mut existing = sessions.list_user(user_id).await?;
        if existing.len() as i64 >= policy.max_sessions {
            existing.sort_by_key(|(_, s)| s.login_time);
            let kick = existing.len() as i64 - policy.max_sessions + 1;
            for (jti, _) in existing.into_iter().take(kick as usize) {
                sessions.revoke(&jti).await?;
            }
        }
    }

    let session_id = Uuid::new_v4().to_string();
    let refresh_token = tokens.new_refresh_token(&session_id);
    let ts = now_secs();
    let session = Session {
        user_id,
        tenant_id,
        username: db_username.clone(),
        refresh_hash: hash_refresh(&refresh_token),
        ip: ip.to_string(),
        user_agent: user_agent.to_string(),
        login_time: ts,
        last_seen: ts,
    };
    sessions
        .create(&session_id, &session, policy.refresh_ttl_secs)
        .await?;

    let access_token = tokens.sign_access(user_id, tenant_id, &session_id, &db_username)?;

    Ok(LoginResult {
        access_token,
        refresh_token,
        principal: Principal {
            user_id: UserId::new(user_id),
            tenant_id: TenantId::new(tenant_id),
            session_id: SessionId(session_id),
            user_name: db_username,
        },
    })
}

/// 记一次失败；达到阈值时把「账号或密码错误」升级为「账号已锁定」。
async fn reject_credentials(
    lockout: &dyn LockoutStore,
    key: &str,
    policy: LockoutPolicy,
) -> AuthError {
    match lockout.record_failure(key, policy).await {
        Ok(state) if state.locked => AuthError::AccountLocked {
            retry_after_secs: state.retry_after_secs,
        },
        _ => AuthError::InvalidCredentials,
    }
}

/// 登录成功后把非 Argon2 哈希升级为 Argon2id（失败仅告警，不影响登录）。
async fn upgrade_password(db: &DatabaseConnection, user_id: i64, password: &str) {
    let Ok(hash) = crate::password::hash_password(password) else {
        return;
    };
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE sys_user SET password = $1, update_time = now() WHERE user_id = $2",
        [hash.into(), user_id.into()],
    );
    if let Err(e) = db.execute_raw(stmt).await {
        tracing::warn!(error = %e, user_id, "密码哈希升级失败");
    }
}

/// 用刷新令牌换新令牌并轮换刷新令牌。
///
/// 旧刷新令牌复用 → 判定泄露，撤销该用户整条会话链。
pub async fn refresh(
    tokens: &TokenService,
    sessions: &dyn SessionStore,
    policy: SessionPolicy,
    refresh_token: &str,
    ip: &str,
    user_agent: &str,
) -> Result<RefreshResult, AuthError> {
    let jti = TokenService::refresh_session_id(refresh_token)
        .ok_or(AuthError::InvalidToken)?
        .to_string();
    let Some(mut session) = sessions.get(&jti).await? else {
        return Err(AuthError::SessionExpired);
    };

    if !session.accepts_refresh(refresh_token) {
        sessions.revoke_user(session.user_id).await?;
        return Err(AuthError::RefreshReplay);
    }

    let new_refresh = tokens.new_refresh_token(&jti);
    session.refresh_hash = hash_refresh(&new_refresh);
    session.last_seen = now_secs();
    if !ip.is_empty() {
        session.ip = ip.to_string();
    }
    if !user_agent.is_empty() {
        session.user_agent = user_agent.to_string();
    }
    sessions
        .create(&jti, &session, policy.refresh_ttl_secs)
        .await?;

    let access_token =
        tokens.sign_access(session.user_id, session.tenant_id, &jti, &session.username)?;

    Ok(RefreshResult {
        access_token,
        refresh_token: new_refresh,
        principal: Principal {
            user_id: UserId::new(session.user_id),
            tenant_id: TenantId::new(session.tenant_id),
            session_id: SessionId(jti),
            user_name: session.username,
        },
    })
}

/// 撤销当前会话（登出）。
pub async fn logout(sessions: &dyn SessionStore, jti: &str) -> Result<(), AuthError> {
    sessions.revoke(jti).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_identifier_normalizes_by_shape() {
        assert_eq!(canonical_identifier(" Admin "), "Admin");
        assert_eq!(
            canonical_identifier("Admin@Example.COM"),
            "admin@example.com"
        );
        assert_eq!(canonical_identifier("+86 138-0000-0000"), "+8613800000000");
        assert_eq!(canonical_identifier("user_1"), "user_1");
    }
}
