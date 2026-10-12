//! 权限集合：`user → roles → menus(perms) → 权限集合`。

use std::collections::BTreeSet;

use cicada_kernel::Permission;
use serde::{Deserialize, Serialize};

/// 去重后的权限标识集合。
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionSet {
    inner: BTreeSet<Permission>,
}

impl PermissionSet {
    /// 空集合。
    pub fn new() -> Self {
        Self::default()
    }

    /// 加入一个权限点（已存在则忽略）。
    pub fn insert(&mut self, perm: Permission) {
        self.inner.insert(perm);
    }

    /// 是否持有该权限点。
    pub fn contains(&self, perm: &Permission) -> bool {
        self.inner.contains(perm)
    }

    /// 权限点数量。
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// 是否没有任何权限点（与 [`PermissionSet::len`] 配对，避免 clippy 警告）。
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// 按字典序遍历权限点。
    pub fn iter(&self) -> impl Iterator<Item = &Permission> {
        self.inner.iter()
    }

    /// 落成有序 `Vec`，用于序列化进缓存值或响应。
    pub fn into_vec(self) -> Vec<Permission> {
        self.inner.into_iter().collect()
    }
}

impl FromIterator<Permission> for PermissionSet {
    fn from_iter<T: IntoIterator<Item = Permission>>(iter: T) -> Self {
        Self {
            inner: iter.into_iter().collect(),
        }
    }
}

/// 把「角色 → 菜单权限」的扁平列表合并成去重集合。
pub fn resolve_permissions(role_perms: impl IntoIterator<Item = Permission>) -> PermissionSet {
    role_perms.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Permission {
        Permission::new(s)
    }

    #[test]
    fn resolve_dedupes_across_roles() {
        let set = resolve_permissions([
            p("system:user:list"),
            p("system:user:add"),
            p("system:user:list"),
        ]);
        assert_eq!(set.len(), 2);
        assert!(set.contains(&p("system:user:list")));
        assert!(set.contains(&p("system:user:add")));
        assert!(!set.contains(&p("system:user:remove")));
    }
}
