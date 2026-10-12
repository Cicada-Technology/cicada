//! 角色授权快照 `RoleGrant`：L2（`system` 装载）与 L3（`rbac` 计算）之间的纯值契约。

use serde::{Deserialize, Serialize};

use crate::id::{DeptId, RoleId};
use crate::scope::DataScopeKind;

/// 一个角色的授权快照。由 `system` 从 `sys_role` / `sys_role_dept` 装载后交给 `rbac`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleGrant {
    pub role_id: RoleId,
    pub role_key: String,
    pub data_scope: DataScopeKind,
    /// `Custom` 时的自定部门（`sys_role_dept`）；`Dept` / `DeptAndChild` 时为空。
    pub dept_ids: Vec<DeptId>,
}
