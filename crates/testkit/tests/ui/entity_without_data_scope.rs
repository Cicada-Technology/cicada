//! 编译失败用例：实体未声明归属维度（datascope-typestate.md ）。
//!
//! `Query::scoped` 要求 `E: DataScoped`；没写 `#[derive(DataScoped)]` 的实体进不了受控查询。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior)]
#[sea_orm(table_name = "no_scope_doc")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub tenant_id: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

fn main() {
    let scope = cicada_kernel::Scope {
        tenant_id: cicada_kernel::TenantId::new(1),
        user_id: Some(cicada_kernel::UserId::new(1)),
        data_scope: cicada_kernel::DataScope {
            kind: cicada_kernel::DataScopeKind::All,
            dept_ids: vec![],
            subtree_paths: vec![],
            user_id: cicada_kernel::UserId::new(1),
        },
    };
    let _ = cicada_kernel::scope::Query::<Entity>::open().scoped(&scope);
}
