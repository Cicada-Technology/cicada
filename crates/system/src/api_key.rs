//! 开放接口凭证 `sys_api_key`：签发 / 校验 / 停用。
//!
//! 密钥形如 `ck_<64 hex>`：明文只在签发时返回一次，库里只留 `secret_hash`
//! （Argon2id）与 `key_prefix`（前 16 字符，用于检索与展示）。校验走
//! 「前缀命中候选 → 逐个验哈希 → 状态与有效期检查」，因此前缀重复不影响正确性。

use argon2::password_hash::phc::PasswordHash;
use argon2::{Argon2, PasswordHasher, PasswordVerifier};
use chrono::{DateTime, Utc};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};

use cicada_infra::Db;
use cicada_kernel::TenantId;

use crate::entity::{ApiKeyActiveModel, ApiKeyColumn, ApiKeyEntity, ApiKeyModel};

/// 明文前缀长度（含 `ck_`），与 `sys_api_key.key_prefix` 一致。
pub const KEY_PREFIX_LEN: usize = 16;

/// 凭证状态：0 生效 / 1 停用。
pub const STATUS_ACTIVE: i16 = 0;
pub const STATUS_DISABLED: i16 = 1;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ApiKeyError {
    #[error("凭证格式非法")]
    Malformed,
    #[error("凭证已停用：{0}")]
    Disabled(String),
    #[error("凭证已过期：{0}")]
    Expired(String),
    #[error("哈希失败：{0}")]
    Hash(String),
    #[error("数据库错误：{0}")]
    Db(String),
}

/// 签发结果：明文密钥只在此刻出现一次。
#[derive(Debug, Clone)]
pub struct IssuedKey {
    pub model: ApiKeyModel,
    pub secret: String,
}

/// 生成明文密钥：`ck_` + 32 字节（64 hex）。用 uuid v4 拼两段，避免引入随机数依赖。
pub fn generate_secret() -> String {
    format!(
        "ck_{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn hash_secret(secret: &str) -> Result<String, ApiKeyError> {
    Argon2::default()
        .hash_password(secret.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| ApiKeyError::Hash(e.to_string()))
}

fn verify_secret(secret: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|parsed| {
            Argon2::default()
                .verify_password(secret.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

/// 凭证的读写入口（属主 `system`）。
#[derive(Clone)]
pub struct ApiKeyService {
    db: Db,
}

impl ApiKeyService {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// 签发：返回库里那行 + 只出现一次的明文密钥。
    pub async fn issue(
        &self,
        tenant: TenantId,
        name: &str,
        scopes: Vec<String>,
        expires_at: Option<DateTime<Utc>>,
        operator: Option<String>,
    ) -> Result<IssuedKey, ApiKeyError> {
        let secret = generate_secret();
        let prefix = secret[..KEY_PREFIX_LEN].to_string();
        let scopes_json =
            serde_json::to_value(scopes).map_err(|e| ApiKeyError::Hash(e.to_string()))?;

        let model = ApiKeyActiveModel {
            tenant_id: Set(tenant.get()),
            name: Set(name.to_string()),
            key_prefix: Set(prefix),
            secret_hash: Set(hash_secret(&secret)?),
            scopes: Set(scopes_json),
            status: Set(STATUS_ACTIVE),
            expires_at: Set(expires_at),
            create_by: Set(operator.clone()),
            update_by: Set(operator),
            ..Default::default()
        }
        .insert(self.db.conn())
        .await
        .map_err(|e| ApiKeyError::Db(e.to_string()))?;

        Ok(IssuedKey { model, secret })
    }

    /// 校验明文密钥。未知/停用/过期分别返回 `Ok(None)` / `Err(Disabled)` / `Err(Expired)`。
    pub async fn verify(&self, secret: &str) -> Result<Option<ApiKeyModel>, ApiKeyError> {
        if secret.len() <= KEY_PREFIX_LEN || !secret.starts_with("ck_") {
            return Ok(None);
        }
        let prefix = &secret[..KEY_PREFIX_LEN];
        let candidates = ApiKeyEntity::find()
            .filter(ApiKeyColumn::KeyPrefix.eq(prefix))
            .all(self.db.conn())
            .await
            .map_err(|e| ApiKeyError::Db(e.to_string()))?;

        for key in candidates {
            if !verify_secret(secret, &key.secret_hash) {
                continue;
            }
            if key.status != STATUS_ACTIVE {
                return Err(ApiKeyError::Disabled(key.name));
            }
            if let Some(expires) = key.expires_at
                && expires <= Utc::now()
            {
                return Err(ApiKeyError::Expired(key.name));
            }
            return Ok(Some(key));
        }
        Ok(None)
    }

    /// 记录最近使用时间（失败只记日志：不阻断业务请求）。
    pub async fn touch_used(&self, id: i64) {
        let update = ApiKeyEntity::update_many()
            .col_expr(
                ApiKeyColumn::LastUsedAt,
                sea_orm::sea_query::Expr::value(Some(Utc::now())),
            )
            .filter(ApiKeyColumn::Id.eq(id))
            .exec(self.db.conn())
            .await;
        if let Err(e) = update {
            tracing::warn!(error = %e, "更新 API Key 最近使用时间失败");
        }
    }

    pub async fn list(&self, tenant: TenantId) -> Result<Vec<ApiKeyModel>, ApiKeyError> {
        ApiKeyEntity::find()
            .filter(ApiKeyColumn::TenantId.eq(tenant.get()))
            .order_by_desc(ApiKeyColumn::Id)
            .all(self.db.conn())
            .await
            .map_err(|e| ApiKeyError::Db(e.to_string()))
    }

    /// 停用（软状态，不可恢复明文密钥，但可再启用）。
    pub async fn set_status(
        &self,
        tenant: TenantId,
        id: i64,
        status: i16,
    ) -> Result<bool, ApiKeyError> {
        let res = ApiKeyEntity::update_many()
            .col_expr(
                ApiKeyColumn::Status,
                sea_orm::sea_query::Expr::value(status),
            )
            .filter(ApiKeyColumn::Id.eq(id))
            .filter(ApiKeyColumn::TenantId.eq(tenant.get()))
            .exec(self.db.conn())
            .await
            .map_err(|e| ApiKeyError::Db(e.to_string()))?;
        Ok(res.rows_affected > 0)
    }

    /// 删除（凭证列表里彻底移除）。
    pub async fn revoke(&self, tenant: TenantId, id: i64) -> Result<bool, ApiKeyError> {
        let res = ApiKeyEntity::delete_many()
            .filter(ApiKeyColumn::Id.eq(id))
            .filter(ApiKeyColumn::TenantId.eq(tenant.get()))
            .exec(self.db.conn())
            .await
            .map_err(|e| ApiKeyError::Db(e.to_string()))?;
        Ok(res.rows_affected > 0)
    }

    /// 凭证的权限集合（精确匹配的权限标识列表）。
    pub fn scopes_of(model: &ApiKeyModel) -> Vec<String> {
        serde_json::from_value(model.scopes.clone()).unwrap_or_default()
    }

    /// 凭证主体的用户 id：凭证不是自然人账号，固定 0（租户级主体，不参与数据范围）。
    pub const SUBJECT_USER_ID: i64 = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_secrets_are_prefixed_and_unique() {
        let a = generate_secret();
        let b = generate_secret();
        assert!(a.starts_with("ck_"));
        assert_eq!(a.len(), 3 + 64);
        assert_ne!(a, b);
    }

    #[test]
    fn hash_verifies_only_the_right_secret() {
        let s = generate_secret();
        let h = hash_secret(&s).unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_secret(&s, &h));
        assert!(!verify_secret("ck_deadbeef", &h));
    }
}
