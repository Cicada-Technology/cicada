//! 授权输入的装载：`user → roles → menus(perms)` 与 `user → dept`。
//!
//! 这里只负责**读自己拥有的表**并把结果组装成纯值（`RoleGrant` / `Permission`），
//! 计算交给 `rbac`。装载用连接池句柄、不经过 `Scope`——角色/菜单是授权计算的
//! 输入，本身不应当被数据范围过滤。

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use cicada_kernel::{DataScopeKind, DeptId, Permission, RoleGrant, RoleId, TenantId, UserId};

use crate::entity::{
    MenuColumn, MenuEntity, RoleColumn, RoleDeptColumn, RoleDeptEntity, RoleEntity, RoleMenuColumn,
    RoleMenuEntity, UserColumn, UserEntity, UserRoleColumn, UserRoleEntity,
};

/// 一次装载产出的纯值快照，直接喂给 `rbac`。
#[derive(Debug, Default, Clone)]
pub struct GrantSnapshot {
    pub user_dept: Option<DeptId>,
    pub roles: Vec<RoleGrant>,
    pub permissions: Vec<Permission>,
}

/// 用户的部门（`Dept` / `DeptAndChild` 范围的「本部门」）。
pub async fn load_user_dept(
    db: &DatabaseConnection,
    tenant: TenantId,
    user_id: UserId,
) -> Option<DeptId> {
    UserEntity::find()
        .filter(UserColumn::UserId.eq(user_id.get()))
        .filter(UserColumn::TenantId.eq(tenant.get()))
        .filter(UserColumn::DeletedAt.is_null())
        .one(db)
        .await
        .ok()
        .flatten()
        .and_then(|u| u.dept_id.map(DeptId::new))
}

/// 用户的角色授权（含 `Custom` 的自定部门）。
pub async fn load_role_grants(
    db: &DatabaseConnection,
    tenant: TenantId,
    user_id: UserId,
) -> Vec<RoleGrant> {
    let role_ids: Vec<i64> = UserRoleEntity::find()
        .filter(UserRoleColumn::UserId.eq(user_id.get()))
        .all(db)
        .await
        .map(|rows| rows.into_iter().map(|r| r.role_id).collect())
        .unwrap_or_default();
    if role_ids.is_empty() {
        return Vec::new();
    }

    let roles = RoleEntity::find()
        .filter(RoleColumn::RoleId.is_in(role_ids.clone()))
        .filter(RoleColumn::TenantId.eq(tenant.get()))
        .filter(RoleColumn::Status.eq(0))
        .filter(RoleColumn::DeletedAt.is_null())
        .all(db)
        .await
        .unwrap_or_default();

    let dept_rows = RoleDeptEntity::find()
        .filter(RoleDeptColumn::RoleId.is_in(role_ids.clone()))
        .all(db)
        .await
        .unwrap_or_default();

    let mut depts_by_role: std::collections::HashMap<i64, Vec<DeptId>> =
        std::collections::HashMap::new();
    for row in dept_rows {
        depts_by_role
            .entry(row.role_id)
            .or_default()
            .push(DeptId::new(row.dept_id));
    }

    roles
        .into_iter()
        .map(|r| RoleGrant {
            role_id: RoleId::new(r.role_id),
            role_key: r.role_key,
            data_scope: DataScopeKind::from_code(r.data_scope).unwrap_or(DataScopeKind::None),
            dept_ids: depts_by_role.remove(&r.role_id).unwrap_or_default(),
        })
        .collect()
}

/// 角色集合对应的权限点（去重在 `rbac` 侧完成）。
pub async fn load_permissions(db: &DatabaseConnection, role_ids: &[RoleId]) -> Vec<Permission> {
    if role_ids.is_empty() {
        return Vec::new();
    }
    let ids: Vec<i64> = role_ids.iter().map(|r| r.get()).collect();

    let menu_ids: Vec<i64> = RoleMenuEntity::find()
        .filter(RoleMenuColumn::RoleId.is_in(ids.clone()))
        .all(db)
        .await
        .map(|rows| rows.into_iter().map(|r| r.menu_id).collect())
        .unwrap_or_default();
    if menu_ids.is_empty() {
        return Vec::new();
    }

    MenuEntity::find()
        .filter(MenuColumn::MenuId.is_in(menu_ids))
        .all(db)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|m| m.perms)
        .filter(|p| !p.is_empty())
        .map(Permission::new)
        .collect()
}

/// 一次性装载完整快照。
pub async fn load_grant_snapshot(
    db: &DatabaseConnection,
    tenant: TenantId,
    user_id: UserId,
) -> GrantSnapshot {
    let user_dept = load_user_dept(db, tenant, user_id).await;
    let roles = load_role_grants(db, tenant, user_id).await;
    let permissions =
        load_permissions(db, &roles.iter().map(|r| r.role_id).collect::<Vec<_>>()).await;
    GrantSnapshot {
        user_dept,
        roles,
        permissions,
    }
}
