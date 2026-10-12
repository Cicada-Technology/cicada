//! 可选的 TOTP 二次验证（可选增强：TOTP 二次验证）。
//!
//! 只做两件事：生成新密钥（含 otpauth URL），以及校验用户提交的 6 位码。
//! 密钥由账号级运营者持有（`sys_user.totp_secret`），启用状态存 `totp_enabled`。

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use totp_rs::{Algorithm, Builder, Secret, Totp};

use cicada_kernel::{TenantId, UserId};

use crate::error::AuthError;

/// TOTP 签发方名（otpauth URL 的 issuer；客户端展示用）。
pub const DEFAULT_ISSUER: &str = "cicada";

/// 新密钥的签发结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TotpSetup {
    /// Base32 编码的共享密钥（也写进 `sys_user.totp_secret`）。
    pub secret_base32: String,
    /// `otpauth://totp/...` URL，交给前端生成二维码。
    pub otpauth_url: String,
}

fn build(secret: Secret, account: &str) -> Option<Totp> {
    Builder::new()
        .with_secret(secret)
        .with_algorithm(Algorithm::SHA1)
        .with_account_name(account)
        .with_issuer(Some(DEFAULT_ISSUER))
        .build()
        .ok()
}

/// 为某个账号生成新的 TOTP 密钥。`account` 通常是用户名。
pub fn generate_setup(account: &str) -> Option<TotpSetup> {
    let secret = Secret::generate();
    let secret_base32 = secret.to_base32();
    let totp = build(Secret::try_from_base32(&secret_base32).ok()?, account)?;
    let otpauth_url = totp.to_url().ok()?;
    Some(TotpSetup {
        secret_base32,
        otpauth_url,
    })
}

/// 校验 `code` 是否匹配 `secret_base32`（含默认 ±1 步的时钟漂移容忍）。
pub fn verify(secret_base32: &str, code: &str) -> bool {
    let Ok(secret) = Secret::try_from_base32(secret_base32) else {
        return false;
    };
    let Some(totp) = build(secret, "verify") else {
        return false;
    };
    totp.check_current(code.trim()).is_some()
}

/// 生成当前时间窗口的验证码（测试 / 客户端自检用）。
pub fn current_code(secret_base32: &str) -> Option<String> {
    let secret = Secret::try_from_base32(secret_base32).ok()?;
    let totp = build(secret, "verify")?;
    Some(totp.generate_current().to_string())
}

/// 读取账号的 TOTP 状态：`(enabled, secret)`。
pub async fn load_state(
    db: &DatabaseConnection,
    tenant: TenantId,
    user_id: UserId,
) -> Result<(bool, Option<String>), AuthError> {
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT totp_enabled, totp_secret FROM sys_user \
         WHERE tenant_id = $1 AND user_id = $2 AND deleted_at IS NULL",
        [tenant.get().into(), user_id.get().into()],
    );
    let Some(row) = db
        .query_all_raw(stmt)
        .await
        .map_err(|e| AuthError::Db(e.to_string()))?
        .into_iter()
        .next()
    else {
        return Err(AuthError::IdentityNotFound);
    };
    let enabled: bool = row
        .try_get_by_index(0)
        .map_err(|e| AuthError::Db(e.to_string()))?;
    let secret: Option<String> = row
        .try_get_by_index(1)
        .map_err(|e| AuthError::Db(e.to_string()))?;
    Ok((enabled, secret))
}

async fn update(
    db: &DatabaseConnection,
    tenant: TenantId,
    user_id: UserId,
    enabled: bool,
    secret: Option<String>,
) -> Result<(), AuthError> {
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE sys_user SET totp_enabled = $1, totp_secret = $2, update_time = now() \
         WHERE tenant_id = $3 AND user_id = $4",
        [
            enabled.into(),
            secret.into(),
            tenant.get().into(),
            user_id.get().into(),
        ],
    );
    db.execute_raw(stmt)
        .await
        .map_err(|e| AuthError::Db(e.to_string()))?;
    Ok(())
}

/// 写入新的待启用的密钥（`totp_enabled` 保持 false）。
pub async fn store_pending_secret(
    db: &DatabaseConnection,
    tenant: TenantId,
    user_id: UserId,
    secret: &str,
) -> Result<(), AuthError> {
    update(db, tenant, user_id, false, Some(secret.to_string())).await
}

/// 启用 TOTP（调用前须已校验码）。
pub async fn enable(
    db: &DatabaseConnection,
    tenant: TenantId,
    user_id: UserId,
) -> Result<(), AuthError> {
    let (_, secret) = load_state(db, tenant, user_id).await?;
    update(db, tenant, user_id, true, secret).await
}

/// 关闭 TOTP 并清除密钥。
pub async fn disable(
    db: &DatabaseConnection,
    tenant: TenantId,
    user_id: UserId,
) -> Result<(), AuthError> {
    update(db, tenant, user_id, false, None).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_then_verify_roundtrips() {
        let setup = generate_setup("admin").expect("生成 TOTP 密钥");
        assert!(!setup.secret_base32.is_empty());
        assert!(setup.otpauth_url.starts_with("otpauth://totp/"));
        assert!(setup.otpauth_url.contains("issuer=cicada"));

        let code = current_code(&setup.secret_base32).unwrap();
        assert!(verify(&setup.secret_base32, &code));
    }

    #[test]
    fn wrong_code_is_rejected() {
        let setup = generate_setup("admin").unwrap();
        assert!(!verify(&setup.secret_base32, "000000"));
        assert!(!verify("not-base32", "123456"));
    }
}
