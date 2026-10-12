//! `biz_goods`：商品
//!
//! 由 cicada 代码生成器产出。归属形态：`DeptAndOwner`。

use cicada_macros::DataScoped;
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior, DataScoped)]
#[sea_orm(table_name = "biz_goods")]
#[data_scope(tenant = Column::TenantId, deleted = Column::DeletedAt, dept = Column::DeptId, owner = Column::CreateBy)]
pub struct Model {
    /// 商品 id
    #[sea_orm(primary_key)]
    pub goods_id: i64,
    /// 租户
    pub tenant_id: i64,
    /// 归属部门
    pub dept_id: i64,
    /// 商品名称
    pub goods_name: String,
    /// 商品类型
    #[sea_orm(column_name = "type")]
    pub type_col: String,
    /// 价格
    pub price: Option<Decimal>,
    /// 库存
    pub quantity: i32,
    /// 是否上架
    pub online: bool,
    /// 上架日期
    pub publish_date: Option<Date>,
    /// 上架时间
    pub publish_at: Option<DateTimeUtc>,
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
