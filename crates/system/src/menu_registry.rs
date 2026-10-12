//! 菜单注册端口的落库实现（`sys_menu` 的属主是 `system`）。
//!
//! 模块只声明「我要哪些菜单/按钮」，这里负责把它们变成 `sys_menu` 的行；
//! 除本模块外任何模块都不得直接写 `sys_menu`（依赖规则规则 6）。

use async_trait::async_trait;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, Value};

use cicada_infra::Db;
use cicada_ports::{MenuKind, MenuRegistry, MenuRegistryError, MenuSpec};

/// 读回来的一行菜单（只保留定位需要的列）。
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    id: i64,
    name: String,
    kind: char,
    perms: String,
}

/// 按 `sys_menu` 落库的注册器。
#[derive(Clone)]
pub struct DbMenuRegistry {
    db: Db,
}

impl DbMenuRegistry {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    async fn load(&self) -> Result<Vec<Row>, MenuRegistryError> {
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT menu_id, menu_name, menu_type, COALESCE(perms, '') FROM sys_menu ORDER BY menu_id",
            [],
        );
        let rows = self
            .db
            .conn()
            .query_all_raw(stmt)
            .await
            .map_err(storage_err)?;
        Ok(rows
            .iter()
            .map(|r| Row {
                id: r.try_get_by_index::<i64>(0).unwrap_or(0),
                name: r.try_get_by_index::<String>(1).unwrap_or_default(),
                kind: r
                    .try_get_by_index::<String>(2)
                    .unwrap_or_default()
                    .chars()
                    .next()
                    .unwrap_or('C'),
                perms: r.try_get_by_index::<String>(3).unwrap_or_default(),
            })
            .collect())
    }

    async fn insert(&self, spec: &MenuSpec, parent_id: i64) -> Result<i64, MenuRegistryError> {
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO sys_menu (menu_name, parent_id, order_num, path, component, query, \
                is_frame, is_cache, menu_type, visible, status, perms, icon, remark, \
                create_by, create_time) \
             VALUES ($1,$2,$3,$4,'',NULL,1,0,$5,0,0,$6,'',$7,'menu-registry',now()) \
             RETURNING menu_id",
            [
                spec.name.clone().into(),
                parent_id.into(),
                spec.order.into(),
                spec.path.clone().unwrap_or_else(|| "#".to_string()).into(),
                Value::String(Some(spec.kind.code().to_string())),
                spec.permission.clone().unwrap_or_default().into(),
                spec.module.clone().into(),
            ],
        );
        let row = self
            .db
            .conn()
            .query_one_raw(stmt)
            .await
            .map_err(storage_err)?
            .ok_or_else(|| MenuRegistryError::Storage("插入菜单没有返回 menu_id".to_string()))?;
        row.try_get_by_index::<i64>(0).map_err(storage_err)
    }
}

#[async_trait]
impl MenuRegistry for DbMenuRegistry {
    async fn upsert(&self, specs: &[MenuSpec]) -> Result<usize, MenuRegistryError> {
        if specs.is_empty() {
            return Ok(0);
        }
        let mut rows = self.load().await?;
        let mut inserted = 0usize;
        // 目录 → 菜单 → 按钮：父级必须先落库，后面的才定位得到。
        for kind in [MenuKind::Directory, MenuKind::Menu, MenuKind::Button] {
            for spec in specs.iter().filter(|s| s.kind == kind) {
                if find_existing(&rows, spec).is_some() {
                    continue;
                }
                let parent_id = resolve_parent(&rows, spec)?;
                let id = self.insert(spec, parent_id).await?;
                rows.push(Row {
                    id,
                    name: spec.name.clone(),
                    kind: spec.kind.code(),
                    perms: spec.permission.clone().unwrap_or_default(),
                });
                inserted += 1;
            }
        }
        Ok(inserted)
    }

    async fn known_permissions(&self) -> Result<Vec<String>, MenuRegistryError> {
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT DISTINCT perms FROM sys_menu WHERE perms IS NOT NULL AND perms <> '' ORDER BY perms",
            [],
        );
        let rows = self
            .db
            .conn()
            .query_all_raw(stmt)
            .await
            .map_err(storage_err)?;
        rows.iter()
            .map(|r| r.try_get_by_index::<String>(0).map_err(storage_err))
            .collect()
    }
}

/// 幂等键：有权限标识时按标识判重，否则按「同名 + 同类型」。
fn find_existing(rows: &[Row], spec: &MenuSpec) -> Option<i64> {
    rows.iter()
        .find(|r| match &spec.permission {
            Some(perm) => r.perms == *perm,
            None => r.name == spec.name && r.kind == spec.kind.code(),
        })
        .map(|r| r.id)
}

/// 父级定位（规则见 [`MenuSpec::parent`]）。
fn resolve_parent(rows: &[Row], spec: &MenuSpec) -> Result<i64, MenuRegistryError> {
    match spec.kind {
        MenuKind::Directory => match &spec.parent {
            Some(name) => find_by_name(rows, name, Some('M')),
            None => Ok(0),
        },
        MenuKind::Menu => match &spec.parent {
            Some(name) => find_by_name(rows, name, None),
            None => Ok(0),
        },
        MenuKind::Button => {
            if let Some(name) = &spec.parent {
                return find_by_name(rows, name, None);
            }
            let perm = spec
                .permission
                .as_deref()
                .ok_or_else(|| MenuRegistryError::MissingParent(spec.name.clone()))?;
            let sibling = MenuSpec::view_sibling(perm)
                .ok_or_else(|| MenuRegistryError::MissingParent(perm.to_string()))?;
            rows.iter()
                .find(|r| r.perms == sibling)
                .map(|r| r.id)
                .ok_or_else(|| {
                    MenuRegistryError::MissingParent(format!(
                        "{sibling}（按钮 {perm} 的所属菜单，模块 {}）",
                        spec.module
                    ))
                })
        }
    }
}

fn find_by_name(rows: &[Row], name: &str, kind: Option<char>) -> Result<i64, MenuRegistryError> {
    rows.iter()
        .find(|r| r.name == name && kind.is_none_or(|k| r.kind == k))
        .map(|r| r.id)
        .ok_or_else(|| MenuRegistryError::MissingParent(name.to_string()))
}

fn storage_err(e: sea_orm::DbErr) -> MenuRegistryError {
    MenuRegistryError::Storage(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, name: &str, kind: char, perms: &str) -> Row {
        Row {
            id,
            name: name.to_string(),
            kind,
            perms: perms.to_string(),
        }
    }

    fn spec(kind: MenuKind, name: &str, perm: &str) -> MenuSpec {
        MenuSpec {
            module: "business".to_string(),
            name: name.to_string(),
            kind,
            permission: (!perm.is_empty()).then(|| perm.to_string()),
            path: (kind != MenuKind::Button).then(|| "/business/demo".to_string()),
            parent: None,
            order: 3,
        }
    }

    #[test]
    fn existing_is_matched_by_permission_first() {
        let rows = vec![row(9, "演示查询", 'F', "business:demo:list")];
        let mut want = spec(MenuKind::Button, "换了个名字", "business:demo:list");
        assert_eq!(find_existing(&rows, &want), Some(9));
        want.permission = None;
        assert_eq!(
            find_existing(&rows, &want),
            None,
            "无权限标识时按同名+同类型判重"
        );
    }

    #[test]
    fn button_parent_comes_from_the_view_sibling() {
        let rows = vec![
            row(1, "系统管理", 'M', ""),
            row(203, "文件管理", 'C', "system:file:view"),
        ];
        let button = spec(MenuKind::Button, "文件查询", "system:file:list");
        assert_eq!(resolve_parent(&rows, &button), Ok(203));

        let orphan = spec(MenuKind::Button, "孤立按钮", "business:none:list");
        assert!(matches!(
            resolve_parent(&rows, &orphan),
            Err(MenuRegistryError::MissingParent(_))
        ));
    }

    #[test]
    fn menu_parent_is_a_directory_name_or_root() {
        let rows = vec![row(1, "系统管理", 'M', "")];
        let mut menu = spec(MenuKind::Menu, "演示管理", "business:demo:view");
        assert_eq!(resolve_parent(&rows, &menu), Ok(0), "未给父级时挂根节点");
        menu.parent = Some("系统管理".to_string());
        assert_eq!(resolve_parent(&rows, &menu), Ok(1));
        menu.parent = Some("不存在的目录".to_string());
        assert!(matches!(
            resolve_parent(&rows, &menu),
            Err(MenuRegistryError::MissingParent(_))
        ));
    }
}
