//! 一次授权判定：数据范围 + 权限集合 + 超级管理员标记。

use cicada_kernel::{DataScope, DeptId, Permission, RoleGrant, TenantId, UserId};
use cicada_ports::DeptDirectory;
use serde::{Deserialize, Serialize};

use crate::data_scope::{compute_data_scope, is_super_admin};
use crate::permission::{PermissionSet, resolve_permissions};

/// 一个已认证主体的授权结果，随请求挂在 `RequestContext` 上。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Authorization {
    pub data_scope: DataScope,
    pub permissions: PermissionSet,
    pub is_super_admin: bool,
    /// 角色键集合（`RequestContext.roles`）。
    pub role_keys: Vec<String>,
}

/// 数据范围计算 + 权限解析，一次调用产出中间件所需的一切。
pub async fn authorize(
    tenant: TenantId,
    user_id: UserId,
    user_dept: Option<DeptId>,
    roles: &[RoleGrant],
    role_permissions: impl IntoIterator<Item = Permission>,
    dept_dir: &dyn DeptDirectory,
) -> Authorization {
    Authorization {
        is_super_admin: is_super_admin(roles),
        data_scope: compute_data_scope(tenant, user_id, user_dept, roles, dept_dir).await,
        permissions: resolve_permissions(role_permissions),
        role_keys: roles.iter().map(|r| r.role_key.clone()).collect(),
    }
}
