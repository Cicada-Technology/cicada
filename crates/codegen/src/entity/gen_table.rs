//! `gen_table`：代码生成的表级元数据。

use sea_orm::entity::prelude::*;

use cicada_macros::DataScoped;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior, DataScoped)]
#[sea_orm(table_name = "gen_table")]
#[data_scope(tenant = Column::TenantId, deleted = Column::DeletedAt, no_ownership)]
pub struct Model {
    #[sea_orm(primary_key)]
    pub table_id: i64,
    pub tenant_id: i64,
    pub table_name: String,
    pub table_comment: String,
    /// Rust 类型名（PascalCase）。
    pub class_name: String,
    /// 生成代码所在的模块目录。
    pub module_name: String,
    /// 业务名（snake_case），用于路由段与文件名。
    pub business_name: String,
    pub function_name: String,
    pub function_author: String,
    /// 主子表的子表名（`tpl_category = sub` 时使用）。
    pub sub_table_name: Option<String>,
    pub sub_table_fk_name: Option<String>,
    /// `crud` / `tree` / `sub`。
    pub tpl_category: String,
    /// 表单一行显示的列数。
    pub form_col_num: i16,
    /// `0` = 打包 zip（默认）；`1` = 写入源码树。
    pub gen_type: String,
    pub gen_path: Option<String>,
    /// 生成器扩展开关（JSON 文本）。
    pub options: Option<String>,
    pub remark: Option<String>,
    pub create_by: Option<String>,
    pub create_time: DateTimeUtc,
    pub update_by: Option<String>,
    pub update_time: Option<DateTimeUtc>,
    pub deleted_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
