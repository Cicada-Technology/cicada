//! `sys_user_identity`：登录身份绑定。一人一账号、多身份绑定。
//!
//! 带 `tenant_id` 与唯一键 `(tenant_id, identity_type, identifier)`，因此不是纯
//! 关联表；不实现 `DataScoped`（授权装载按 `user_id` 直取，属授权输入而非受控数据）。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior)]
#[sea_orm(table_name = "sys_user_identity")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub identity_id: i64,
    pub tenant_id: i64,
    pub user_id: i64,
    /// `Username` / `Phone` / `Email` / `WeChatOpen` / `WeChatMp`。
    pub identity_type: String,
    pub identifier: String,
    pub provider: Option<String>,
    pub verified_at: Option<DateTimeUtc>,
    pub is_primary: bool,
    pub create_time: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
