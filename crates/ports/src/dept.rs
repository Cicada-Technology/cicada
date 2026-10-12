//! 部门目录端口 `DeptDirectory`。
//!
//! 部门树属于 `system` 域（remote but owned）。`rbac` 在计算 `DataScope` 时，
//! 通过本端口把「本部门及以下」展开成 `dept_ids`，业务服务只消费展开后的列表，
//! 不再远程问一次部门树。

use std::collections::HashMap;

use async_trait::async_trait;

use cicada_kernel::{DeptId, TenantId};

#[derive(Debug, thiserror::Error)]
pub enum DeptDirectoryError {
    #[error("部门不存在：租户 {0} 部门 {1}")]
    NotFound(TenantId, DeptId),
    #[error("部门目录查询失败：{0}")]
    Db(String),
}

/// 部门目录：返回后代集合与 ancestors 路径。
#[async_trait]
pub trait DeptDirectory: Send + Sync {
    /// `root` 的全部后代（不含自身）。
    async fn descendants(
        &self,
        tenant: TenantId,
        root: DeptId,
    ) -> Result<Vec<DeptId>, DeptDirectoryError>;

    /// `dept` 的祖先链，**含自身**，从根到叶（如 `"0,100,101"` 表示 101 挂在 100 下）。
    async fn path(&self, tenant: TenantId, dept: DeptId) -> Result<String, DeptDirectoryError>;
}

/// 测试夹具：内存部门目录，从父子关系推导后代与路径。
#[derive(Default)]
pub struct InMemoryDeptDirectory {
    /// (tenant, dept) -> parent；根节点 parent 为 0。
    parents: HashMap<(TenantId, DeptId), DeptId>,
}

impl InMemoryDeptDirectory {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一条父子关系。`parent = DeptId::new(0)` 表示根节点。
    pub fn insert(&mut self, tenant: TenantId, dept: DeptId, parent: DeptId) {
        self.parents.insert((tenant, dept), parent);
    }

    fn children_map(&self, tenant: TenantId) -> HashMap<DeptId, Vec<DeptId>> {
        let mut children: HashMap<DeptId, Vec<DeptId>> = HashMap::new();
        for ((t, dept), parent) in &self.parents {
            if *t != tenant {
                continue;
            }
            children.entry(*parent).or_default().push(*dept);
        }
        children
    }
}

#[async_trait]
impl DeptDirectory for InMemoryDeptDirectory {
    async fn descendants(
        &self,
        tenant: TenantId,
        root: DeptId,
    ) -> Result<Vec<DeptId>, DeptDirectoryError> {
        let children = self.children_map(tenant);
        let mut out = Vec::new();
        let mut stack: Vec<DeptId> = children.get(&root).cloned().unwrap_or_default();
        while let Some(node) = stack.pop() {
            out.push(node);
            if let Some(kids) = children.get(&node) {
                stack.extend(kids);
            }
        }
        Ok(out)
    }

    async fn path(&self, tenant: TenantId, dept: DeptId) -> Result<String, DeptDirectoryError> {
        if !self.parents.contains_key(&(tenant, dept)) {
            return Err(DeptDirectoryError::NotFound(tenant, dept));
        }
        let mut chain = vec![dept];
        let mut cursor = dept;
        while let Some(parent) = self.parents.get(&(tenant, cursor)).copied() {
            if parent == DeptId::new(0) {
                break;
            }
            chain.push(parent);
            cursor = parent;
        }
        chain.push(DeptId::new(0));
        chain.reverse();
        Ok(chain
            .iter()
            .map(|d| d.get().to_string())
            .collect::<Vec<_>>()
            .join(","))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn descendants_and_path() {
        let tenant = TenantId::new(1);
        let mut dir = InMemoryDeptDirectory::new();
        // 100(根) -> 101 -> 102
        dir.insert(tenant, DeptId::new(100), DeptId::new(0));
        dir.insert(tenant, DeptId::new(101), DeptId::new(100));
        dir.insert(tenant, DeptId::new(102), DeptId::new(101));
        dir.insert(tenant, DeptId::new(200), DeptId::new(0));

        let mut desc = dir.descendants(tenant, DeptId::new(100)).await.unwrap();
        desc.sort();
        assert_eq!(desc, vec![DeptId::new(101), DeptId::new(102)]);

        assert_eq!(
            dir.path(tenant, DeptId::new(102)).await.unwrap(),
            "0,100,101,102"
        );
        assert!(dir.path(tenant, DeptId::new(999)).await.is_err());
    }
}
