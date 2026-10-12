//! 数据范围计算：角色合并 → 声明式 `DataScope`。
//!
//! `DeptAndChild` 的后代在 `rbac` 侧展开进 `dept_ids`，消费侧只做 `IN` 判断，
//! 不需要跨模块 JOIN 部门树。

use std::collections::BTreeSet;

use cicada_kernel::{DataScope, DataScopeKind, DeptId, RoleGrant, TenantId, UserId};
use cicada_ports::DeptDirectory;

/// 超级管理员角色键（与 `migrations/init_admin.rs` 一致）。命中即短路为 `All`。
pub const SUPER_ADMIN_ROLE_KEY: &str = "admin";

/// 是否命中超级管理员（任一角色的 role_key 为 `admin`）。
pub fn is_super_admin(roles: &[RoleGrant]) -> bool {
    roles.iter().any(|r| r.role_key == SUPER_ADMIN_ROLE_KEY)
}

/// 计算一个用户的合并数据范围（多角色取并集，宽者优先）。
///
/// 合并规则：
/// 1. 无角色 → `None`（失败关闭）；
/// 2. 超级管理员或任一角色为 `All` → `All`；
/// 3. 否则并集各角色的可见部门：`Custom` 的自定部门 ∪ `Dept` 的本部门
///    ∪ `DeptAndChild` 的本部门及其后代（经 `DeptDirectory` 展开）；
/// 4. 有可见部门 → 取最宽的部门类 kind（`DeptAndChild > Custom > Dept`）；
/// 5. 无可见部门但存在 `SelfOnly` 角色 → `SelfOnly`；
/// 6. 否则 → `None`。
pub async fn compute_data_scope(
    tenant: TenantId,
    user_id: UserId,
    user_dept: Option<DeptId>,
    roles: &[RoleGrant],
    dept_dir: &dyn DeptDirectory,
) -> DataScope {
    if roles.is_empty() {
        return DataScope::none();
    }
    if is_super_admin(roles) || roles.iter().any(|r| r.data_scope == DataScopeKind::All) {
        return DataScope::all();
    }

    let mut dept_ids: BTreeSet<DeptId> = BTreeSet::new();
    let mut subtree_paths: Vec<String> = Vec::new();
    let mut widest_kind = DataScopeKind::None;
    let mut has_self_only = false;

    for role in roles {
        match role.data_scope {
            DataScopeKind::Custom => {
                dept_ids.extend(role.dept_ids.iter().copied());
                widest_kind = wider(widest_kind, DataScopeKind::Custom);
            }
            DataScopeKind::Dept | DataScopeKind::DeptAndChild => {
                let dept_kind = if role.data_scope == DataScopeKind::DeptAndChild {
                    DataScopeKind::DeptAndChild
                } else {
                    DataScopeKind::Dept
                };
                widest_kind = wider(widest_kind, dept_kind);

                let Some(dept) = user_dept else {
                    tracing::warn!(
                        role_id = %role.role_id,
                        scope = dept_kind.as_str(),
                        "部门范围角色但用户无部门：该角色不贡献可见部门"
                    );
                    continue;
                };

                dept_ids.insert(dept);
                if dept_kind == DataScopeKind::DeptAndChild {
                    match dept_dir.descendants(tenant, dept).await {
                        Ok(descendants) => dept_ids.extend(descendants),
                        Err(e) => tracing::warn!(
                            role_id = %role.role_id,
                            error = %e,
                            "DeptAndChild 后代展开失败，按仅本部门处理"
                        ),
                    }
                    match dept_dir.path(tenant, dept).await {
                        Ok(path) if !path.is_empty() && !subtree_paths.contains(&path) => {
                            subtree_paths.push(path);
                        }
                        Ok(_) => {}
                        Err(e) => tracing::warn!(
                            role_id = %role.role_id,
                            error = %e,
                            "DeptAndChild ancestors 路径获取失败"
                        ),
                    }
                }
            }
            DataScopeKind::SelfOnly => {
                has_self_only = true;
                widest_kind = wider(widest_kind, DataScopeKind::SelfOnly);
            }
            DataScopeKind::All => {}
            DataScopeKind::None => {}
        }
    }

    if !dept_ids.is_empty() {
        DataScope {
            kind: widest_kind,
            dept_ids: dept_ids.into_iter().collect(),
            subtree_paths,
            user_id,
        }
    } else if has_self_only {
        DataScope {
            kind: DataScopeKind::SelfOnly,
            dept_ids: Vec::new(),
            subtree_paths: Vec::new(),
            user_id,
        }
    } else {
        DataScope::none()
    }
}

/// 宽者优先的偏序：`None < SelfOnly < Dept < Custom < DeptAndChild < All`。
fn wider(a: DataScopeKind, b: DataScopeKind) -> DataScopeKind {
    fn rank(k: DataScopeKind) -> u8 {
        match k {
            DataScopeKind::None => 0,
            DataScopeKind::SelfOnly => 1,
            DataScopeKind::Dept => 2,
            DataScopeKind::Custom => 3,
            DataScopeKind::DeptAndChild => 4,
            DataScopeKind::All => 5,
        }
    }
    if rank(b) > rank(a) { b } else { a }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cicada_kernel::RoleId;
    use cicada_ports::InMemoryDeptDirectory;

    fn role(id: i64, key: &str, scope: DataScopeKind, depts: &[i64]) -> RoleGrant {
        RoleGrant {
            role_id: RoleId::new(id),
            role_key: key.to_string(),
            data_scope: scope,
            dept_ids: depts.iter().copied().map(DeptId::new).collect(),
        }
    }

    #[tokio::test]
    async fn super_admin_short_circuits_to_all() {
        let dir = InMemoryDeptDirectory::new();
        let roles = [role(1, "admin", DataScopeKind::SelfOnly, &[])];
        let scope = compute_data_scope(TenantId::new(1), UserId::new(7), None, &roles, &dir).await;
        assert_eq!(scope.kind, DataScopeKind::All);
    }

    #[tokio::test]
    async fn no_roles_is_none() {
        let dir = InMemoryDeptDirectory::new();
        let scope = compute_data_scope(TenantId::new(1), UserId::new(7), None, &[], &dir).await;
        assert_eq!(scope.kind, DataScopeKind::None);
    }

    #[tokio::test]
    async fn custom_roles_union_depts() {
        let dir = InMemoryDeptDirectory::new();
        let roles = [
            role(1, "ops", DataScopeKind::Custom, &[3, 4]),
            role(2, "hr", DataScopeKind::Custom, &[5]),
        ];
        let scope = compute_data_scope(TenantId::new(1), UserId::new(7), None, &roles, &dir).await;
        assert_eq!(scope.kind, DataScopeKind::Custom);
        assert_eq!(
            scope.dept_ids,
            vec![DeptId::new(3), DeptId::new(4), DeptId::new(5)]
        );
    }

    #[tokio::test]
    async fn dept_and_child_expands_descendants() {
        let tenant = TenantId::new(1);
        let mut dir = InMemoryDeptDirectory::new();
        dir.insert(tenant, DeptId::new(10), DeptId::new(0));
        dir.insert(tenant, DeptId::new(11), DeptId::new(10));
        dir.insert(tenant, DeptId::new(12), DeptId::new(11));

        let roles = [role(1, "leader", DataScopeKind::DeptAndChild, &[])];
        let scope =
            compute_data_scope(tenant, UserId::new(7), Some(DeptId::new(10)), &roles, &dir).await;

        assert_eq!(scope.kind, DataScopeKind::DeptAndChild);
        assert_eq!(
            scope.dept_ids,
            vec![DeptId::new(10), DeptId::new(11), DeptId::new(12)]
        );
        assert_eq!(scope.subtree_paths, vec!["0,10".to_string()]);
    }

    #[tokio::test]
    async fn wider_kind_wins_and_depts_union() {
        let tenant = TenantId::new(1);
        let mut dir = InMemoryDeptDirectory::new();
        dir.insert(tenant, DeptId::new(10), DeptId::new(0));
        dir.insert(tenant, DeptId::new(11), DeptId::new(10));

        let roles = [
            role(1, "custom", DataScopeKind::Custom, &[20]),
            role(2, "tree", DataScopeKind::DeptAndChild, &[]),
        ];
        let scope =
            compute_data_scope(tenant, UserId::new(7), Some(DeptId::new(10)), &roles, &dir).await;

        assert_eq!(scope.kind, DataScopeKind::DeptAndChild);
        assert_eq!(
            scope.dept_ids,
            vec![DeptId::new(10), DeptId::new(11), DeptId::new(20)]
        );
    }

    #[tokio::test]
    async fn self_only_without_dept_scope() {
        let dir = InMemoryDeptDirectory::new();
        let roles = [role(1, "self", DataScopeKind::SelfOnly, &[])];
        let scope = compute_data_scope(TenantId::new(1), UserId::new(7), None, &roles, &dir).await;
        assert_eq!(scope.kind, DataScopeKind::SelfOnly);
        assert_eq!(scope.user_id, UserId::new(7));
    }
}
