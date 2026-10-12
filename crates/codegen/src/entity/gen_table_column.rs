//! `gen_table_column`：代码生成的列级元数据。
//!
//! 与参考实现的差异：`is_*` 用真正的 `bool`（参考实现用 `char '1'`），
//! `java_type` / `java_field` 换成 `rust_type` / `rust_field`。

use sea_orm::entity::prelude::*;

use cicada_macros::DataScoped;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior, DataScoped)]
#[sea_orm(table_name = "gen_table_column")]
#[data_scope(tenant = Column::TenantId, no_ownership)]
pub struct Model {
    #[sea_orm(primary_key)]
    pub column_id: i64,
    pub tenant_id: i64,
    pub table_id: i64,
    pub column_name: String,
    pub column_comment: String,
    /// 数据库原始类型（`udt_name`，如 `varchar` / `int8` / `timestamptz`）。
    pub column_type: String,
    /// Rust 类型字面量（`i64` / `Option<String>` / `DateTimeUtc`）。
    pub rust_type: String,
    /// Rust 字段名（snake_case，关键字已转义）。
    pub rust_field: String,
    pub is_pk: bool,
    pub is_increment: bool,
    pub is_required: bool,
    pub is_insert: bool,
    pub is_edit: bool,
    pub is_list: bool,
    pub is_query: bool,
    /// `EQ` / `LIKE` / `BETWEEN`。
    pub query_type: String,
    /// 前端控件：`input` / `textarea` / `select` / `datetime` / `date` / `switch` / `image` …
    pub html_type: String,
    pub dict_type: Option<String>,
    /// 列序号（`ordinal_position`）。
    pub sort: i32,
    pub create_time: DateTimeUtc,
    pub update_time: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
