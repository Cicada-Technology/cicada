//! 装配器：收集模块清单，做全部跨模块校验，fail fast（FINAL.md ）。

use std::collections::{HashMap, HashSet};

use axum::Router;
use thiserror::Error;

use crate::module::{MenuNode, Module, ModuleName, Permission, RoutesFn, TableName};
use crate::registry::{Extension, Merge, SlotKey};
use crate::routes::RouteMeta;
use crate::state::AppState;

#[derive(Debug, Error)]
pub enum AssembleError {
    #[error("模块重名：{0}")]
    DuplicateModule(ModuleName),
    #[error("权限点被两个模块声明：{perm}（{first} 与 {second}）")]
    DuplicatePermission {
        perm: Permission,
        first: ModuleName,
        second: ModuleName,
    },
    #[error("模块 {module} 的种子写了自己不拥有的表 {table}")]
    ForeignTableSeed {
        module: ModuleName,
        table: TableName,
    },
    #[error("槽冲突（{slot}）：键 {key:?} 由 {first} 与 {second} 各自贡献")]
    SlotConflict {
        slot: &'static str,
        key: &'static str,
        first: ModuleName,
        second: ModuleName,
    },
    #[error("未知的装配依赖：{0}")]
    UnknownDependency(ModuleName),
    #[error("装配依赖成环：{0:?}")]
    DependencyCycle(Vec<ModuleName>),
    #[error("路由 {route} 声明的权限点未在模块清单里注册：{perm}")]
    UnseededPermission { perm: Permission, route: String },
    #[error("模块 {module} 声明的权限点没有任何路由使用：{perm}")]
    OrphanPermission {
        perm: Permission,
        module: ModuleName,
    },
}

/// 一次报全部装配错误，避免「修一个报一个」。
#[derive(Debug, Error)]
#[error("装配失败：{} 处错误", errors.len())]
pub struct AssembleReport {
    pub errors: Vec<AssembleError>,
}

struct ModuleRecord {
    name: ModuleName,
    owns: Vec<TableName>,
    routes: Option<RoutesFn>,
    perms: Vec<Permission>,
    menus: Vec<MenuNode>,
    after: Vec<ModuleName>,
    seeds: Vec<crate::module::SeedOp>,
    extensions: Vec<Extension>,
}

/// 装配器：模块装入 → 封口校验。
pub struct Assembler {
    modules: Vec<ModuleRecord>,
    seen: HashSet<ModuleName>,
    errors: Vec<AssembleError>,
}

impl Default for Assembler {
    fn default() -> Self {
        Self::new()
    }
}

impl Assembler {
    pub fn new() -> Self {
        Self {
            modules: Vec::new(),
            seen: HashSet::new(),
            errors: Vec::new(),
        }
    }

    /// 装入一个模块。feature 裁剪 = 不调用本方法 = 四处注册点同时缺席。
    pub fn with(mut self, m: Box<dyn Module>) -> Self {
        let manifest = m.manifest();
        if !self.seen.insert(manifest.name) {
            self.errors
                .push(AssembleError::DuplicateModule(manifest.name));
        }
        self.modules.push(ModuleRecord {
            name: manifest.name,
            owns: manifest.owns,
            routes: manifest.routes,
            perms: manifest.perms,
            menus: manifest.menus,
            after: manifest.after,
            seeds: manifest.seeds,
            extensions: manifest.extensions,
        });
        self
    }

    pub fn with_all<I: IntoIterator<Item = Box<dyn Module>>>(mut self, iter: I) -> Self {
        for m in iter {
            self = self.with(m);
        }
        self
    }

    /// 封口：跑全部跨模块校验。任何一处失败即返回完整 `AssembleReport`，不留半个可用服务。
    pub fn seal(self) -> Result<Assembled, AssembleReport> {
        let mut errors = self.errors;
        detect_duplicate_perms(&self.modules, &mut errors);
        detect_foreign_seed(&self.modules, &mut errors);
        detect_slot_conflicts(&self.modules, &mut errors);
        detect_dependency_issues(&self.modules, &mut errors);

        if errors.is_empty() {
            Ok(Assembled {
                modules: self.modules,
            })
        } else {
            Err(AssembleReport { errors })
        }
    }
}

/// 已封口、可反复取用的装配结果。
pub struct Assembled {
    modules: Vec<ModuleRecord>,
}

impl std::fmt::Debug for Assembled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<ModuleName> = self.modules.iter().map(|m| m.name).collect();
        f.debug_struct("Assembled")
            .field("modules", &names)
            .finish()
    }
}

impl Assembled {
    pub fn names(&self) -> impl Iterator<Item = ModuleName> + '_ {
        self.modules.iter().map(|m| m.name)
    }

    pub fn len(&self) -> usize {
        self.modules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    pub fn perm_count(&self) -> usize {
        self.modules.iter().map(|m| m.perms.len()).sum()
    }

    /// 各模块声明的菜单，转成端口层的中立声明（模块边界：落库交给菜单属主，不在这里写表）。
    ///
    /// `mount` 会消耗 `Assembled`，所以取菜单要在挂载之前；组装根拿到列表后
    /// 交给 `cicada_ports::MenuRegistry`，属主缺席应当启动失败。
    pub fn declared_menus(&self) -> Vec<cicada_ports::MenuSpec> {
        self.modules
            .iter()
            .flat_map(|m| m.menus.iter().map(move |node| (m.name.0, node)))
            .map(|(module, node)| node.to_spec(module))
            .collect()
    }

    /// 调用各模块的 routes 函数，收集路由元数据，与 `Manifest.perms` 双向对账，
    /// 通过后合并成一个 `Router`；任一方向有缺口即返回全部错误。
    pub fn mount(self, state: AppState) -> Result<Router<AppState>, AssembleReport> {
        let mut router = Router::<AppState>::new();
        let mut manifest_perms: Vec<(ModuleName, Permission)> = Vec::new();
        let mut route_metas: Vec<(ModuleName, RouteMeta)> = Vec::new();

        for m in &self.modules {
            for perm in &m.perms {
                manifest_perms.push((m.name, perm.clone()));
            }
            if let Some(routes_fn) = m.routes {
                let (module_router, metas) = routes_fn(state.clone()).into_parts();
                router = router.merge(module_router);
                for meta in metas {
                    route_metas.push((m.name, meta));
                }
            }
        }

        let errors = reconcile_route_perms(&manifest_perms, &route_metas);
        if errors.is_empty() {
            Ok(router)
        } else {
            Err(AssembleReport { errors })
        }
    }
}

/// 权限点双向对账：
/// - 路由声明了却没在模块清单注册（`UnseededPermission`）；
/// - 模块清单注册了却没有任何路由使用（`OrphanPermission`）。
pub fn reconcile_route_perms(
    manifest_perms: &[(ModuleName, Permission)],
    routes: &[(ModuleName, RouteMeta)],
) -> Vec<AssembleError> {
    let declared: HashSet<&Permission> = manifest_perms.iter().map(|(_, p)| p).collect();
    let mut used: HashSet<&Permission> = HashSet::new();
    let mut errors = Vec::new();

    for (_module, meta) in routes {
        for perm in &meta.perms {
            if !declared.contains(perm) {
                errors.push(AssembleError::UnseededPermission {
                    perm: perm.clone(),
                    route: meta.path.clone(),
                });
            }
        }
        used.extend(meta.perms.iter());
    }

    for (module, perm) in manifest_perms {
        if !used.contains(perm) {
            errors.push(AssembleError::OrphanPermission {
                perm: perm.clone(),
                module: *module,
            });
        }
    }

    errors
}

fn detect_duplicate_perms(modules: &[ModuleRecord], errors: &mut Vec<AssembleError>) {
    let mut owner: HashMap<Permission, ModuleName> = HashMap::new();
    for m in modules {
        for perm in &m.perms {
            if let Some(first) = owner.insert(perm.clone(), m.name) {
                errors.push(AssembleError::DuplicatePermission {
                    perm: perm.clone(),
                    first,
                    second: m.name,
                });
            }
        }
    }
}

fn detect_foreign_seed(modules: &[ModuleRecord], errors: &mut Vec<AssembleError>) {
    for m in modules {
        let owned: HashSet<TableName> = m.owns.iter().copied().collect();
        for seed in &m.seeds {
            if !owned.contains(&seed.table) {
                errors.push(AssembleError::ForeignTableSeed {
                    module: m.name,
                    table: seed.table,
                });
            }
        }
    }
}

fn detect_slot_conflicts(modules: &[ModuleRecord], errors: &mut Vec<AssembleError>) {
    // (SlotKey, kind, merge) -> 贡献者
    type Group<'a> = HashMap<SlotKey, (&'static str, Merge, Vec<(&'a str, ModuleName)>)>;
    let mut groups: Group<'_> = HashMap::new();
    for m in modules {
        for e in &m.extensions {
            let entry = groups
                .entry(e.slot())
                .or_insert((e.kind(), e.merge(), Vec::new()));
            entry.2.push((e.key().unwrap_or(""), m.name));
        }
    }

    for (_, (kind, merge, contributors)) in groups {
        match merge {
            Merge::Exclusive => {
                if contributors.len() > 1 {
                    errors.push(AssembleError::SlotConflict {
                        slot: kind,
                        key: "",
                        first: contributors[0].1,
                        second: contributors[1].1,
                    });
                }
            }
            Merge::Keyed => {
                let mut seen: HashMap<&'static str, ModuleName> = HashMap::new();
                for (key, module) in contributors {
                    if let Some(first) = seen.insert(key, module) {
                        errors.push(AssembleError::SlotConflict {
                            slot: kind,
                            key,
                            first,
                            second: module,
                        });
                    }
                }
            }
            Merge::Collect => {}
        }
    }
}

fn detect_dependency_issues(modules: &[ModuleRecord], errors: &mut Vec<AssembleError>) {
    let names: HashSet<ModuleName> = modules.iter().map(|m| m.name).collect();
    let mut adj: HashMap<ModuleName, Vec<ModuleName>> = HashMap::new();
    for m in modules {
        for dep in &m.after {
            if !names.contains(dep) {
                errors.push(AssembleError::UnknownDependency(*dep));
            }
        }
        adj.insert(m.name, m.after.clone());
    }
    if let Some(cycle) = find_cycle(&adj) {
        errors.push(AssembleError::DependencyCycle(cycle));
    }
}

fn find_cycle(adj: &HashMap<ModuleName, Vec<ModuleName>>) -> Option<Vec<ModuleName>> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mark {
        Unvisited,
        Visiting,
        Done,
    }

    fn dfs(
        node: ModuleName,
        adj: &HashMap<ModuleName, Vec<ModuleName>>,
        marks: &mut HashMap<ModuleName, Mark>,
        stack: &mut Vec<ModuleName>,
    ) -> Option<Vec<ModuleName>> {
        marks.insert(node, Mark::Visiting);
        stack.push(node);
        for &next in adj.get(&node).into_iter().flatten() {
            match marks.get(&next).copied().unwrap_or(Mark::Unvisited) {
                Mark::Unvisited => {
                    if let Some(c) = dfs(next, adj, marks, stack) {
                        return Some(c);
                    }
                }
                Mark::Visiting => {
                    let pos = stack.iter().position(|&x| x == next).unwrap_or(0);
                    let mut cycle = stack[pos..].to_vec();
                    cycle.push(next);
                    return Some(cycle);
                }
                Mark::Done => {}
            }
        }
        marks.insert(node, Mark::Done);
        stack.pop();
        None
    }

    let mut marks: HashMap<ModuleName, Mark> = adj.keys().map(|k| (*k, Mark::Unvisited)).collect();
    let mut stack: Vec<ModuleName> = Vec::new();
    for &start in adj.keys() {
        if matches!(marks.get(&start), Some(Mark::Unvisited))
            && let Some(c) = dfs(start, adj, &mut marks, &mut stack)
        {
            return Some(c);
        }
    }
    None
}
