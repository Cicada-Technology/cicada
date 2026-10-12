//! 模块声明（Manifest）与 `Module` trait：默认路径（FINAL.md 项目目标）。

use crate::registry::Extension;
use crate::routes::Routes;
use crate::state::AppState;

pub use cicada_kernel::Permission;

/// 模块的路由函数：交出带元数据的 `Routes`（不是裸 `Router`）。
pub type RoutesFn = fn(AppState) -> Routes;

/// 模块的稳定标识。用于重名检测、依赖排序、报错指认。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ModuleName(pub &'static str);

impl std::fmt::Display for ModuleName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// 模块拥有的表名（数据所有权， 规则 1）。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TableName(pub &'static str);

impl std::fmt::Display for TableName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// 种子的稳定键，冲突判定的域。例如 `"perm:system:user:add"` / `"dict:sys_normal_disable"`。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SeedKey(pub &'static str);

/// 菜单类型：`M` 目录 / `C` 菜单 / `F` 按钮（与参考实现一致）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuType {
    Directory,
    Menu,
    Button,
}

/// 菜单节点声明。落库交给菜单属主（`cicada_ports::MenuRegistry`，
/// 实现是 `cicada_system::DbMenuRegistry`），模块不直插 `sys_menu`（模块边界 / 依赖规则规则 6）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuNode {
    pub name: &'static str,
    pub menu_type: MenuType,
    pub permission: Option<Permission>,
    /// 前端路由（`C` 类需要；目录与按钮可留 `None`，落库为 `#`）。
    pub path: Option<&'static str>,
    /// 父级：目录/菜单填父目录的**菜单名**（如 `"系统管理"`）；
    /// 按钮通常留空，由属主按 `xxx:view` 约定推断所属菜单。
    pub parent: Option<&'static str>,
    /// 同级排序（参考实现 `order_num`）。
    pub order: i32,
}

impl MenuNode {
    /// 最简构造：按钮常用（名称 + 权限点，父级按约定推断）。
    pub fn button(name: &'static str, permission: Permission) -> Self {
        Self {
            name,
            menu_type: MenuType::Button,
            permission: Some(permission),
            path: None,
            parent: None,
            order: 1,
        }
    }

    /// 菜单（`C` 类）：带前端路由与可选父目录名。
    pub fn menu(
        name: &'static str,
        permission: Permission,
        path: &'static str,
        parent: Option<&'static str>,
    ) -> Self {
        Self {
            name,
            menu_type: MenuType::Menu,
            permission: Some(permission),
            path: Some(path),
            parent,
            order: 1,
        }
    }

    /// 转成端口层的中立声明，交给菜单属主落库。
    pub fn to_spec(&self, module: &str) -> cicada_ports::MenuSpec {
        use cicada_ports::{MenuKind, MenuSpec};
        MenuSpec {
            module: module.to_string(),
            name: self.name.to_string(),
            kind: match self.menu_type {
                MenuType::Directory => MenuKind::Directory,
                MenuType::Menu => MenuKind::Menu,
                MenuType::Button => MenuKind::Button,
            },
            permission: self.permission.as_ref().map(|p| p.as_str().to_string()),
            path: self.path.map(str::to_string),
            parent: self.parent.map(str::to_string),
            order: self.order,
        }
    }
}

/// 一条种子操作：写入 `table`，以 `key` 做冲突判定。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeedOp {
    pub key: SeedKey,
    pub table: TableName,
}

/// 模块对框架的全部声明。所有字段可缺省，只有 `name` 必填。
pub struct Manifest {
    pub name: ModuleName,
    /// 本模块拥有的表，用于种子属主校验与跨模块顺序推导。
    pub owns: Vec<TableName>,
    /// 本模块的路由函数（含逐路由权限元数据）。
    pub routes: Option<RoutesFn>,
    /// 权限点（`system:user:add` 形式），与 `#[require_perm]` 双向对账。
    pub perms: Vec<Permission>,
    /// 菜单节点，交给菜单属主落库。
    pub menus: Vec<MenuNode>,
    /// 种子数据，只允许写 `owns` 声明的表。
    pub seeds: Vec<SeedOp>,
    /// 非数据原因的附加偏序（拓扑排序用，必须无环）。
    pub after: Vec<ModuleName>,
    /// 逃生通道：框架事先不知道的自定义注册点。
    pub extensions: Vec<Extension>,
}

impl Manifest {
    pub fn new(name: &'static str) -> Self {
        Self {
            name: ModuleName(name),
            owns: Vec::new(),
            routes: None,
            perms: Vec::new(),
            menus: Vec::new(),
            seeds: Vec::new(),
            after: Vec::new(),
            extensions: Vec::new(),
        }
    }

    pub fn owns(mut self, tables: impl IntoIterator<Item = TableName>) -> Self {
        self.owns.extend(tables);
        self
    }

    pub fn routes(mut self, routes: RoutesFn) -> Self {
        self.routes = Some(routes);
        self
    }

    pub fn perms(mut self, perms: impl IntoIterator<Item = Permission>) -> Self {
        self.perms.extend(perms);
        self
    }

    pub fn menus(mut self, menus: impl IntoIterator<Item = MenuNode>) -> Self {
        self.menus.extend(menus);
        self
    }

    pub fn seeds(mut self, seeds: impl IntoIterator<Item = SeedOp>) -> Self {
        self.seeds.extend(seeds);
        self
    }

    pub fn after(mut self, after: impl IntoIterator<Item = ModuleName>) -> Self {
        self.after.extend(after);
        self
    }

    pub fn extensions(mut self, extensions: impl IntoIterator<Item = Extension>) -> Self {
        self.extensions.extend(extensions);
        self
    }
}

/// 模块唯一必须实现的东西：一个方法，交出清单。
///
/// 取 `self: Box<Self>` 以保住 `dyn Module`（框架模块与业务模块进同一个 `Vec`）。
pub trait Module: Send + Sync + 'static {
    fn manifest(self: Box<Self>) -> Manifest;
}
