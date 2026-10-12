//! 引用归属校验（所有按 ID 取数的方法强制带租户条件，并在返回前做归属校验）。
//!
//! 写接口会收到一批**引用**：给用户挂哪些角色、把用户放在哪个部门、给角色授哪些菜单与
//! 数据范围部门。这些 id 是调用方给的，不能假定它们属于调用者的租户——一旦放进
//! `sys_user_role` / `sys_role_dept`，授权计算就会读到别的租户的行。
//! 校验统一按「本租户、未软删」再查一遍，用业务码目录的 `CICADA_RBAC_2002` 拒绝；
//! 不复用 404：调用方确实提交了这么一行，问题不是「不存在」，是「不是你的」。
//! 也不复用 403「无权限标识」：那种情况是缺 `system:user:edit`，补救动作完全不同。

use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};

use cicada_system::entity::{
    DeptColumn, DeptEntity, MenuColumn, MenuEntity, RoleColumn, RoleEntity,
};

use super::{ApiErr, db_error, tenant_mismatch};

/// 去重并保序，报错时指认第一个不合格 id（一次报一个比一口气列出全部更好定位）。
fn wanted(ids: &[i64]) -> Vec<i64> {
    let mut seen: Vec<i64> = Vec::with_capacity(ids.len());
    for id in ids {
        if !seen.contains(id) {
            seen.push(*id);
        }
    }
    seen
}

/// 角色必须全部是「本租户、未软删」。
pub(crate) async fn assert_roles<C: ConnectionTrait>(
    db: &C,
    tenant: i64,
    role_ids: &[i64],
) -> Result<(), ApiErr> {
    let ids = wanted(role_ids);
    if ids.is_empty() {
        return Ok(());
    }
    let owned = RoleEntity::find()
        .filter(RoleColumn::RoleId.is_in(ids.clone()))
        .filter(RoleColumn::TenantId.eq(tenant))
        .filter(RoleColumn::DeletedAt.is_null())
        .all(db)
        .await
        .map_err(db_error)?
        .into_iter()
        .map(|r| r.role_id)
        .collect::<Vec<_>>();
    match ids.into_iter().find(|id| !owned.contains(id)) {
        Some(id) => Err(tenant_mismatch(format!("角色 {id} 不属于当前租户或已删除"))),
        None => Ok(()),
    }
}

/// 部门必须全部是「本租户、未软删」（用户的 `dept_id` 与角色的数据范围部门都走这里）。
pub(crate) async fn assert_depts<C: ConnectionTrait>(
    db: &C,
    tenant: i64,
    dept_ids: &[i64],
) -> Result<(), ApiErr> {
    let ids = wanted(dept_ids);
    if ids.is_empty() {
        return Ok(());
    }
    let owned = DeptEntity::find()
        .filter(DeptColumn::DeptId.is_in(ids.clone()))
        .filter(DeptColumn::TenantId.eq(tenant))
        .filter(DeptColumn::DeletedAt.is_null())
        .all(db)
        .await
        .map_err(db_error)?
        .into_iter()
        .map(|d| d.dept_id)
        .collect::<Vec<_>>();
    match ids.into_iter().find(|id| !owned.contains(id)) {
        Some(id) => Err(tenant_mismatch(format!("部门 {id} 不属于当前租户或已删除"))),
        None => Ok(()),
    }
}

/// 用户的 `dept_id` 是可空的单值。
pub(crate) async fn assert_dept<C: ConnectionTrait>(
    db: &C,
    tenant: i64,
    dept_id: Option<i64>,
) -> Result<(), ApiErr> {
    assert_depts(db, tenant, &dept_id.map(|d| vec![d]).unwrap_or_default()).await
}

/// 菜单是全局字典（`sys_menu` 无 `tenant_id`），因此只校验存在：
/// 挂一个不存在的菜单 id 到角色上，等于授出一条永远解析不出权限点的引用。
pub(crate) async fn assert_menus<C: ConnectionTrait>(
    db: &C,
    menu_ids: &[i64],
) -> Result<(), ApiErr> {
    let ids = wanted(menu_ids);
    if ids.is_empty() {
        return Ok(());
    }
    let found = MenuEntity::find()
        .filter(MenuColumn::MenuId.is_in(ids.clone()))
        .all(db)
        .await
        .map_err(db_error)?
        .into_iter()
        .map(|m| m.menu_id)
        .collect::<Vec<_>>();
    match ids.into_iter().find(|id| !found.contains(id)) {
        Some(id) => Err(tenant_mismatch(format!("菜单 {id} 不存在"))),
        None => Ok(()),
    }
}
