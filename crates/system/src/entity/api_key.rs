//! `sys_api_key`：开放接口凭证。
//!
//! 只存 `secret_hash`（Argon2id），明文仅在创建时返回一次；`key_prefix` 用于展示与检索。

use sea_orm::entity::prelude::*;

use cicada_macros::DataScoped;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior, DataScoped)]
#[sea_orm(table_name = "sys_api_key")]
#[data_scope(tenant = Column::TenantId, no_ownership)]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub tenant_id: i64,
    pub name: String,
    /// 明文前缀（`ck_xxxxxxxx`），不足以还原密钥。
    pub key_prefix: String,
    pub secret_hash: String,
    /// 允许的权限标识集合（`["system:user:list", ...]`；`["*"]` = 租户内全部）。
    pub scopes: Json,
    /// 0 生效 / 1 停用（`Active` / `Disabled`）。
    pub status: i16,
    pub expires_at: Option<DateTimeUtc>,
    pub last_used_at: Option<DateTimeUtc>,
    pub remark: Option<String>,
    pub create_by: Option<String>,
    pub create_time: DateTimeUtc,
    pub update_by: Option<String>,
    pub update_time: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
