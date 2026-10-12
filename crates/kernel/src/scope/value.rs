//! 数据范围与请求级窄接口的纯值部分（datascope-typestate.md ）。
//!
//! 无闭包、无 SQL、无连接：可进 JWT claim、可随跨服务调用传递。

use serde::{Deserialize, Serialize};

use crate::id::{DeptId, TenantId, UserId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DataScopeKind {
    /// 全部数据权限。
    All,
    /// 自定义数据权限（指定部门集合）。
    Custom,
    /// 本部门数据权限。
    Dept,
    /// 本部门及以下数据权限。
    DeptAndChild,
    /// 仅本人数据权限。
    SelfOnly,
    /// 无数据权限（内部兜底）。
    None,
}

impl DataScopeKind {
    /// RLS 会话变量与日志用的稳定标识。
    pub fn as_str(self) -> &'static str {
        match self {
            DataScopeKind::All => "All",
            DataScopeKind::Custom => "Custom",
            DataScopeKind::Dept => "Dept",
            DataScopeKind::DeptAndChild => "DeptAndChild",
            DataScopeKind::SelfOnly => "SelfOnly",
            DataScopeKind::None => "None",
        }
    }

    /// 参考实现 `sys_role.data_scope` 的数字值 → 枚举（存量数据映射）。
    /// `1=All, 2=Custom, 3=Dept, 4=DeptAndChild, 5=SelfOnly`；未知值返回 `None`。
    pub fn from_code(v: i16) -> Option<Self> {
        match v {
            1 => Some(Self::All),
            2 => Some(Self::Custom),
            3 => Some(Self::Dept),
            4 => Some(Self::DeptAndChild),
            5 => Some(Self::SelfOnly),
            _ => None,
        }
    }
}

/// 声明式范围对象。只由 `rbac` 构造、消费侧只读；不含闭包/SQL，可进 JWT claim、
/// 可随跨服务调用传递。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataScope {
    pub kind: DataScopeKind,
    /// `Custom` / `Dept` / `DeptAndChild` 时生效。
    pub dept_ids: Vec<DeptId>,
    /// `DeptAndChild`：部门 ancestors 前缀（声明形式，与 一致）。
    pub subtree_paths: Vec<String>,
    /// `SelfOnly` 时生效。
    pub user_id: UserId,
}

impl DataScope {
    pub fn all() -> Self {
        Self {
            kind: DataScopeKind::All,
            dept_ids: Vec::new(),
            subtree_paths: Vec::new(),
            user_id: UserId::new(0),
        }
    }

    pub fn none() -> Self {
        Self {
            kind: DataScopeKind::None,
            dept_ids: Vec::new(),
            subtree_paths: Vec::new(),
            user_id: UserId::new(0),
        }
    }
}

/// 请求级窄接口：持久层只认识「哪个租户、谁、能看多大」，不认识 HTTP。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub tenant_id: TenantId,
    /// `None` 表示系统任务（定时任务 / 迁移）。
    pub user_id: Option<UserId>,
    pub data_scope: DataScope,
}

#[cfg(any(test, feature = "testkit"))]
impl Scope {
    /// 测试夹具：直接构造，不经过任何 `RequestContext`（datascope-typestate.md 约束 6）。
    pub fn for_test(tenant: TenantId, user: UserId, depts: impl Into<Vec<DeptId>>) -> Self {
        let dept_ids = depts.into();
        let kind = if dept_ids.is_empty() {
            DataScopeKind::Dept
        } else {
            DataScopeKind::Custom
        };
        Self {
            tenant_id: tenant,
            user_id: Some(user),
            data_scope: DataScope {
                kind,
                dept_ids,
                subtree_paths: Vec::new(),
                user_id: user,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_scope_roundtrips_through_json() {
        let scope = DataScope {
            kind: DataScopeKind::Custom,
            dept_ids: vec![DeptId::new(3), DeptId::new(4)],
            subtree_paths: vec![],
            user_id: UserId::new(0),
        };
        let json = serde_json::to_string(&scope).unwrap();
        let back: DataScope = serde_json::from_str(&json).unwrap();
        assert_eq!(scope, back);
    }

    #[test]
    fn scope_all_and_none() {
        assert_eq!(DataScope::all().kind, DataScopeKind::All);
        assert_eq!(DataScope::none().kind, DataScopeKind::None);
    }

    #[test]
    fn kind_as_str_is_stable() {
        assert_eq!(DataScopeKind::DeptAndChild.as_str(), "DeptAndChild");
        assert_eq!(DataScopeKind::SelfOnly.as_str(), "SelfOnly");
    }

    #[test]
    fn from_code_maps_all_five() {
        assert_eq!(DataScopeKind::from_code(1), Some(DataScopeKind::All));
        assert_eq!(DataScopeKind::from_code(2), Some(DataScopeKind::Custom));
        assert_eq!(DataScopeKind::from_code(3), Some(DataScopeKind::Dept));
        assert_eq!(
            DataScopeKind::from_code(4),
            Some(DataScopeKind::DeptAndChild)
        );
        assert_eq!(DataScopeKind::from_code(5), Some(DataScopeKind::SelfOnly));
        assert_eq!(DataScopeKind::from_code(9), None);
    }
}
