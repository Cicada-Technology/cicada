//! `sys_file`：文件元数据。有 `tenant_id` 与软删，
//! 无部门/归属列（`NoOwnership`）。

use sea_orm::entity::prelude::*;

use cicada_macros::DataScoped;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior, DataScoped)]
#[sea_orm(table_name = "sys_file")]
#[data_scope(tenant = Column::TenantId, deleted = Column::DeletedAt, no_ownership)]
pub struct Model {
    #[sea_orm(primary_key)]
    pub file_id: i64,
    pub tenant_id: i64,
    /// 驱动：`local` / `s3` / `oss` / `minio`。
    pub storage: String,
    /// 不可猜测路径 `tenant/uuid`，对外不暴露。
    pub path: String,
    /// 仅用于展示，不参与路径拼接。
    pub original_name: String,
    /// 字节数。
    pub size: i64,
    /// 由 magic number 判定，不信扩展名。
    pub mime: String,
    /// 内容 sha256（去重与完整性校验）。
    pub sha256: String,
    pub biz_type: Option<String>,
    pub biz_id: Option<i64>,
    pub create_by: Option<String>,
    pub create_time: DateTimeUtc,
    pub update_by: Option<String>,
    pub update_time: Option<DateTimeUtc>,
    pub deleted_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
