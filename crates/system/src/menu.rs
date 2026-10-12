//! 菜单树装载：`role_ids → sys_role_menu → sys_menu → 树`（`/auth/routers`）。

use std::collections::HashMap;

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};

use cicada_kernel::RoleId;

use crate::entity::{MenuColumn, MenuEntity, RoleMenuColumn, RoleMenuEntity};

/// 前端路由树节点。只含 `M`（目录）与 `C`（菜单）；`F` 按钮的权限点
/// 已由 `/auth/profile` 的权限集合下发，不再重复进树。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MenuTreeNode {
    pub menu_id: i64,
    pub parent_id: i64,
    pub menu_name: String,
    pub menu_type: String,
    pub path: Option<String>,
    pub component: Option<String>,
    pub perms: Option<String>,
    pub order_num: i32,
    pub children: Vec<MenuTreeNode>,
}

/// 装载某角色集合可见的菜单树（根为 `parent_id = 0` 或父节点不在可见集合中的节点）。
pub async fn load_menu_tree(db: &DatabaseConnection, role_ids: &[RoleId]) -> Vec<MenuTreeNode> {
    if role_ids.is_empty() {
        return Vec::new();
    }
    let ids: Vec<i64> = role_ids.iter().map(|r| r.get()).collect();

    let menu_ids: Vec<i64> = RoleMenuEntity::find()
        .filter(RoleMenuColumn::RoleId.is_in(ids))
        .all(db)
        .await
        .map(|rows| rows.into_iter().map(|r| r.menu_id).collect())
        .unwrap_or_default();
    if menu_ids.is_empty() {
        return Vec::new();
    }

    let nodes: Vec<MenuTreeNode> = MenuEntity::find()
        .filter(MenuColumn::MenuId.is_in(menu_ids))
        .filter(MenuColumn::MenuType.is_in(["M".to_string(), "C".to_string()]))
        .all(db)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|m| MenuTreeNode {
            menu_id: m.menu_id,
            parent_id: m.parent_id,
            menu_name: m.menu_name,
            menu_type: m.menu_type,
            path: m.path,
            component: m.component,
            perms: m.perms,
            order_num: m.order_num,
            children: Vec::new(),
        })
        .collect();

    build_tree(nodes)
}

fn build_tree(nodes: Vec<MenuTreeNode>) -> Vec<MenuTreeNode> {
    let mut by_id: HashMap<i64, MenuTreeNode> = nodes.into_iter().map(|n| (n.menu_id, n)).collect();

    let root_ids: Vec<i64> = by_id
        .values()
        .filter(|n| n.parent_id == 0 || !by_id.contains_key(&n.parent_id))
        .map(|n| n.menu_id)
        .collect();

    let mut children: HashMap<i64, Vec<i64>> = HashMap::new();
    for n in by_id.values() {
        if n.parent_id != 0 {
            children.entry(n.parent_id).or_default().push(n.menu_id);
        }
    }

    fn assemble(
        id: i64,
        by_id: &mut HashMap<i64, MenuTreeNode>,
        children: &HashMap<i64, Vec<i64>>,
    ) -> MenuTreeNode {
        let mut node = by_id.remove(&id).expect("root 必然在 by_id 中");
        if let Some(kids) = children.get(&id) {
            for kid in kids {
                if by_id.contains_key(kid) {
                    node.children.push(assemble(*kid, by_id, children));
                }
            }
        }
        node
    }

    let mut roots: Vec<MenuTreeNode> = root_ids
        .into_iter()
        .map(|id| assemble(id, &mut by_id, &children))
        .collect();
    sort_nodes(&mut roots);
    roots
}

fn sort_nodes(nodes: &mut [MenuTreeNode]) {
    nodes.sort_by_key(|n| (n.order_num, n.menu_id));
    for n in nodes {
        sort_nodes(&mut n.children);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(menu_id: i64, parent_id: i64, order_num: i32) -> MenuTreeNode {
        MenuTreeNode {
            menu_id,
            parent_id,
            menu_name: format!("m{menu_id}"),
            menu_type: "M".to_string(),
            path: None,
            component: None,
            perms: None,
            order_num,
            children: Vec::new(),
        }
    }

    #[test]
    fn builds_and_sorts_tree() {
        let nodes = vec![node(2, 0, 2), node(1, 0, 1), node(11, 1, 1), node(12, 1, 0)];
        let roots = build_tree(nodes);
        assert_eq!(roots[0].menu_id, 1);
        assert_eq!(roots[1].menu_id, 2);
        assert_eq!(roots[0].children[0].menu_id, 12); // order_num 0 在前
        assert_eq!(roots[0].children[1].menu_id, 11);
    }

    #[test]
    fn orphan_parent_is_promoted_to_root() {
        let nodes = vec![node(5, 99, 0)]; // parent 99 不在集合里
        let roots = build_tree(nodes);
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].menu_id, 5);
    }
}
