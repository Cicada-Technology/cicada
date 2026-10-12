//! `sys_user`：用户（账号，一人一行）。数据范围：按部门过滤（`DeptOnly`）。

use sea_orm::entity::prelude::*;

use cicada_macros::DataScoped;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior, DataScoped)]
#[sea_orm(table_name = "sys_user")]
#[data_scope(tenant = Column::TenantId, dept = Column::DeptId, deleted = Column::DeletedAt)]
pub struct Model {
    #[sea_orm(primary_key)]
    pub user_id: i64,
    pub tenant_id: i64,
    pub user_name: String,
    pub nick_name: String,
    pub dept_id: Option<i64>,
    pub password: String,
    pub email: Option<String>,
    pub phonenumber: Option<String>,
    pub status: i16,
    /// 可选的 TOTP 共享密钥（Base32；仅启用后校验）。
    pub totp_secret: Option<String>,
    pub totp_enabled: bool,
    pub login_ip: Option<String>,
    pub login_date: Option<DateTimeUtc>,
    pub version: i32,
    pub deleted_at: Option<DateTimeUtc>,
    pub remark: Option<String>,
    pub create_by: Option<String>,
    pub create_time: DateTimeUtc,
    pub update_by: Option<String>,
    pub update_time: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
