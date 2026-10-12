//! `sys_dict_type`：字典类型（全局表，无 `tenant_id`、无软删）。
//!
//! 因此**不**实现 `DataScoped`：字典是全局字典，不参与租户/数据范围。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior)]
#[sea_orm(table_name = "sys_dict_type")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub dict_id: i64,
    pub dict_name: String,
    pub dict_type: String,
    pub status: i16,
    pub create_by: Option<String>,
    pub create_time: DateTimeUtc,
    pub update_by: Option<String>,
    pub update_time: Option<DateTimeUtc>,
    pub remark: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
