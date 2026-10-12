//! `LocalDeptDirectory`：`DeptDirectory` 端口的本地 adapter（直连 `sys_dept`）。
//!
//! 部门树属于 `system` 域，因此「直连本模块」的本地实现放在这里；微服务版换成
//! `RemoteDeptDirectory`（HTTP + 缓存），切换点只在组合根。

use std::collections::HashMap;

use async_trait::async_trait;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use cicada_infra::Db;
use cicada_kernel::{DeptId, TenantId};
use cicada_ports::{DeptDirectory, DeptDirectoryError};

use crate::entity::{DeptColumn, DeptEntity};

/// 本地部门目录：单体版直连本模块的 `sys_dept`。
#[derive(Clone)]
pub struct LocalDeptDirectory {
    db: Db,
}

impl LocalDeptDirectory {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// 装载某租户的「部门 → 父部门」映射（不含已删除节点）。
    async fn load_parents(
        &self,
        tenant: TenantId,
    ) -> Result<HashMap<DeptId, DeptId>, DeptDirectoryError> {
        let rows = DeptEntity::find()
            .filter(DeptColumn::TenantId.eq(tenant.get()))
            .filter(DeptColumn::DeletedAt.is_null())
            .all(self.db.conn())
            .await
            .map_err(|e| DeptDirectoryError::Db(e.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|r| (DeptId::new(r.dept_id), DeptId::new(r.parent_id)))
            .collect())
    }
}

#[async_trait]
impl DeptDirectory for LocalDeptDirectory {
    async fn descendants(
        &self,
        tenant: TenantId,
        root: DeptId,
    ) -> Result<Vec<DeptId>, DeptDirectoryError> {
        let parents = self.load_parents(tenant).await?;
        if !parents.contains_key(&root) {
            return Err(DeptDirectoryError::NotFound(tenant, root));
        }

        let mut children: HashMap<DeptId, Vec<DeptId>> = HashMap::new();
        for (dept, parent) in &parents {
            children.entry(*parent).or_default().push(*dept);
        }

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
        let parents = self.load_parents(tenant).await?;
        if !parents.contains_key(&dept) {
            return Err(DeptDirectoryError::NotFound(tenant, dept));
        }

        let mut chain = vec![dept];
        let mut cursor = dept;
        while let Some(parent) = parents.get(&cursor).copied() {
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
