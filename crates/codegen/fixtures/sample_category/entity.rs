//! `biz_category`：商品分类
//!
//! 由 cicada 代码生成器产出。归属形态：`DeptAndOwner`。

use cicada_macros::DataScoped;
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior, DataScoped)]
#[sea_orm(table_name = "biz_category")]
#[data_scope(tenant = Column::TenantId, deleted = Column::DeletedAt, dept = Column::DeptId, owner = Column::CreateBy)]
pub struct Model {
    /// 分类 id
    #[sea_orm(primary_key)]
    pub cat_id: i64,
    /// 租户
    pub tenant_id: i64,
    /// 归属部门
    pub dept_id: i64,
    /// 上级分类
    pub parent_id: i64,
    /// 分类名称
    pub cat_name: String,
    /// 显示顺序
    pub sort_order: i32,
    /// 是否启用
    pub online: bool,
    /// 备注
    pub remark: Option<String>,
    /// 创建人
    pub create_by: Option<String>,
    /// 创建时间
    pub create_time: DateTimeUtc,
    /// 更新人
    pub update_by: Option<String>,
    /// 更新时间
    pub update_time: Option<DateTimeUtc>,
    /// 软删
    pub deleted_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
