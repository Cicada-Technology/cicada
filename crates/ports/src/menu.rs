//! 菜单注册端口 `MenuRegistry`。
//!
//! `sys_menu` 的属主是 `system`。模块（框架自带的或 `cicada new` 生成的）只**声明**
//! 自己要哪些菜单与按钮，落库交给属主注册的 adapter——属主缺席时启动失败，
//! 而不是让模块自己去 `INSERT sys_menu`。
//!
//! 两个 adapter：[`InMemoryMenuRegistry`]（测试夹具）与 `cicada_system::DbMenuRegistry`（真实落库）。

use std::sync::Mutex;

use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MenuRegistryError {
    #[error("菜单父级找不到：{0}")]
    MissingParent(String),
    #[error("菜单落库失败：{0}")]
    Storage(String),
}

/// 与参考实现的 `menu_type` 对应：`M` 目录 / `C` 菜单 / `F` 按钮。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKind {
    Directory,
    Menu,
    Button,
}

impl MenuKind {
    pub fn code(self) -> char {
        match self {
            Self::Directory => 'M',
            Self::Menu => 'C',
            Self::Button => 'F',
        }
    }
}

/// 一条菜单声明。`module` 是声明方模块名，用于幂等键与排障归属。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuSpec {
    pub module: String,
    pub name: String,
    pub kind: MenuKind,
    /// 权限标识（`system:file:list`）。目录通常没有。
    pub permission: Option<String>,
    /// 前端路由（`C` 类必填；`M`/`F` 可为 `#`）。
    pub path: Option<String>,
    /// 父级定位：
    /// - `Button` 留空时按约定推断——取同模块里 `permission` 把动作段换成 `view` 的那条
    ///   （`system:file:add` → `system:file:view`），找不到再报错；
    /// - `Menu` 用父目录的**菜单名**（如 `"系统管理"`），留空挂到根；
    /// - `Directory` 忽略本字段。
    pub parent: Option<String>,
    /// 同级排序（参考实现 `order_num`）。
    pub order: i32,
}

impl MenuSpec {
    /// 把权限的动作段换成 `view`：按钮→所属菜单的对账键。
    pub fn view_sibling(permission: &str) -> Option<String> {
        let parts: Vec<&str> = permission.split(':').collect();
        if parts.len() < 3 {
            return None;
        }
        Some([parts[..parts.len() - 1].join(":"), "view".to_string()].join(":"))
    }
}

/// 菜单属主提供的落库缝。实现必须**幂等**：装配可能在同一进程里跑多次（测试、热重载）。
#[async_trait]
pub trait MenuRegistry: Send + Sync + 'static {
    /// 落库一批声明，返回新插入的行数（已存在的只更新可变字段，不计入）。
    async fn upsert(&self, specs: &[MenuSpec]) -> Result<usize, MenuRegistryError>;

    /// 当前已登记的菜单权限标识（装配期对账用）。
    async fn known_permissions(&self) -> Result<Vec<String>, MenuRegistryError>;
}

/// 测试夹具：只记录，不落库。
#[derive(Default)]
pub struct InMemoryMenuRegistry {
    written: Mutex<Vec<MenuSpec>>,
}

impl InMemoryMenuRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn written(&self) -> Vec<MenuSpec> {
        self.written.lock().unwrap().clone()
    }
}

#[async_trait]
impl MenuRegistry for InMemoryMenuRegistry {
    async fn upsert(&self, specs: &[MenuSpec]) -> Result<usize, MenuRegistryError> {
        let mut guard = self.written.lock().unwrap();
        let before = guard.len();
        for spec in specs {
            // 幂等键与真实实现一致：同模块 + 同权限（或同名）。
            let exists = guard.iter().any(|s| {
                s.module == spec.module
                    && (match (&s.permission, &spec.permission) {
                        (Some(a), Some(b)) => a == b,
                        _ => s.name == spec.name,
                    })
            });
            if !exists {
                guard.push(spec.clone());
            }
        }
        Ok(guard.len() - before)
    }

    async fn known_permissions(&self) -> Result<Vec<String>, MenuRegistryError> {
        Ok(self
            .written
            .lock()
            .unwrap()
            .iter()
            .filter_map(|s| s.permission.clone())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, perm: &str, kind: MenuKind) -> MenuSpec {
        MenuSpec {
            module: "business".to_string(),
            name: name.to_string(),
            kind,
            permission: Some(perm.to_string()),
            path: Some("/business/demo".to_string()),
            parent: None,
            order: 1,
        }
    }

    #[tokio::test]
    async fn upsert_is_idempotent_by_module_and_permission() {
        let reg = InMemoryMenuRegistry::new();
        let rows = [spec("演示查询", "business:demo:list", MenuKind::Button)];
        assert_eq!(reg.upsert(&rows).await.unwrap(), 1);
        assert_eq!(reg.upsert(&rows).await.unwrap(), 0, "重复声明不应重复插入");
        assert_eq!(
            reg.known_permissions().await.unwrap(),
            vec!["business:demo:list"]
        );
    }

    #[test]
    fn view_sibling_moves_the_action_segment() {
        assert_eq!(
            MenuSpec::view_sibling("system:file:add").as_deref(),
            Some("system:file:view")
        );
        assert_eq!(
            MenuSpec::view_sibling("monitor:cache:clear").as_deref(),
            Some("monitor:cache:view")
        );
        assert_eq!(MenuSpec::view_sibling("notify:send"), None);
    }

    #[test]
    fn kinds_map_to_codes() {
        assert_eq!(MenuKind::Directory.code(), 'M');
        assert_eq!(MenuKind::Menu.code(), 'C');
        assert_eq!(MenuKind::Button.code(), 'F');
    }
}
