//! 内置模板清单。
//!
//! 模板与生成逻辑分离：这里只负责「有哪些模板、默认长什么样、如何被项目覆写」。
//! 项目覆写目录（`gen.template_dir`）里的 `<name>.j2` 文件优先于内置模板。

/// 一个产物的模板定义。
pub struct TemplateDef {
    /// 模板名（覆写目录里的文件名去掉 `.j2`，即 `<name>.j2`）。
    pub name: &'static str,
    /// 产物路径模板（也是 minijinja 模板串，可引用上下文变量）。
    pub path: &'static str,
    /// 内置模板源码。
    pub source: &'static str,
    /// 树表（`tpl_category = tree`）分类下替换 [`TemplateDef::source`] 的模板；
    /// `None` 表示两种分类共用同一模板。
    ///
    /// 覆写目录里的 `<name>.j2` 按模板名生效，**不分分类**：覆写 `vue.j2` 会同时
    /// 替换单表页与树表页；要只改树表页，就在自己的模板里用 `is_tree` 分流。
    pub tree_source: Option<&'static str>,
}

/// 只为树表提供另一份实现的模板（其余模板共用，用 `{% if is_tree %}` 分支）。
macro_rules! tpl {
    ($name:literal, $path:literal, $file:literal) => {
        TemplateDef {
            name: $name,
            path: $path,
            source: include_str!(concat!("../templates/", $file)),
            tree_source: None,
        }
    };
    ($name:literal, $path:literal, $file:literal, tree = $tree_file:literal) => {
        TemplateDef {
            name: $name,
            path: $path,
            source: include_str!(concat!("../templates/", $file)),
            tree_source: Some(include_str!(concat!("../templates/", $tree_file))),
        }
    };
}

/// 内置模板。顺序即 zip 内的目录顺序（README 放最后）。
pub const TEMPLATES: &[TemplateDef] = &[
    tpl!(
        "migration",
        "migration/m{{ date }}_{{ seq }}_{{ business_name }}.rs",
        "migration.rs.j2"
    ),
    tpl!(
        "mod",
        "src/{{ module_name }}/{{ business_name }}/mod.rs",
        "mod.rs.j2"
    ),
    tpl!(
        "entity",
        "src/{{ module_name }}/{{ business_name }}/entity.rs",
        "entity.rs.j2"
    ),
    tpl!(
        "dto",
        "src/{{ module_name }}/{{ business_name }}/dto.rs",
        "dto.rs.j2"
    ),
    tpl!(
        "handler",
        "src/{{ module_name }}/{{ business_name }}/handler.rs",
        "handler.rs.j2"
    ),
    tpl!(
        "module",
        "src/{{ module_name }}/{{ business_name }}/module.rs",
        "module.rs.j2"
    ),
    tpl!(
        "api",
        "web/src/api/{{ module_name }}/{{ business_name }}.ts",
        "api.ts.j2"
    ),
    tpl!(
        "vue",
        "web/src/views/{{ module_name }}/{{ business_name }}/index.vue",
        "index.vue.j2",
        tree = "index-tree.vue.j2"
    ),
    tpl!(
        "menu_sql",
        "sql/{{ business_name }}_menu.sql",
        "menu.sql.j2"
    ),
    tpl!("readme", "README.md", "README.md.j2"),
];
