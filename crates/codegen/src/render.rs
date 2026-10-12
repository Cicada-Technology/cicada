//! 模板渲染：把 [`TablePlan`] 变成一组「路径 → 内容」的产物。
//!
//! 渲染是纯函数：同样的 plan + 同样的模板 → 字节级一致的产物，因此可快照、可 diff。
//! 模板可被项目覆写：`gen.template_dir` 下 `<模板名>.j2` 优先于内置
//! （模板名即 [`crate::templates::TemplateDef::name`]，如 `entity.j2`）。
//!
//! 生成逻辑（哪些列进 DTO、怎么解析时间、怎么拼 `ColumnDef`）都在 Rust 侧算好，
//! 模板只做插值，便于非 Rust 使用者改模板而不改语义。

use std::path::{Path, PathBuf};

use minijinja::Environment;
use serde::Serialize;

use crate::CodegenError;
use crate::naming;
use crate::plan::{ColumnPlan, TablePlan, TplCategory};
use crate::templates::TEMPLATES;

/// 一个产物：相对路径 + 文本内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub path: String,
    pub content: String,
}

/// 渲染输入。
pub struct RenderInput<'a> {
    pub plan: &'a TablePlan,
    /// 迁移文件的时间戳前缀，如 `20261009`。
    pub date: String,
    /// 迁移序号，如 `000008`。
    pub seq: String,
    /// 接口路径前缀，如 `/api/v1`。
    pub route_prefix: String,
    /// 模板覆写目录。
    pub template_dir: Option<PathBuf>,
}

#[derive(Serialize, Clone)]
struct ColumnCtx {
    name: String,
    field: String,
    pascal: String,
    comment: String,
    /// 实体类型（`Option<DateTimeUtc>` 等）。
    rust_type: String,
    /// 实体类型去掉 `Option<..>` 的部分。
    rust_base: String,
    /// DTO 类型：时间类列用 `String`。
    dto_type: String,
    /// 查询参数类型：`Option<base>`。
    query_type: String,
    /// 时间类列的解析方式：`datetime` / `date` / `time` / 空。
    temporal: String,
    /// 该列在 DTO 里是不是 `String`（决定空串过滤与 `.clone()`）。
    is_string: bool,
    /// 查询谓词：`eq` / `contains`，以及对应的实参写法。
    filter_method: String,
    filter_arg: String,
    /// `Set(...)` 里的表达式（`req.` 前缀已含）。
    insert_expr: String,
    /// `From<Model>` 里的表达式（`m.` 前缀已含）。
    vo_expr: String,
    /// `ColumnDef` 片段，如 `.big_integer().not_null().primary_key()`。
    column_def: String,
    query_type_cfg: String,
    html_type: String,
    dict_type: String,
    is_pk: bool,
    is_required: bool,
    is_list: bool,
    is_query: bool,
    is_insert: bool,
    is_edit: bool,
    /// 树表的父键列（树形页把它渲染成 `el-tree-select`，不进查询/表格列）。
    is_tree_parent: bool,
    /// 树表的节点显示名列（树形页把它放第一列，展开箭头才落在名字上）。
    is_tree_name: bool,
}

#[derive(Serialize)]
struct Ctx {
    table_name: String,
    table_comment: String,
    class_name: String,
    module_name: String,
    business_name: String,
    struct_name: String,
    function_name: String,
    function_author: String,
    category: String,
    perm_prefix: String,
    perms: Vec<String>,
    /// 与 `perms` 一一对应的中文按钮名（菜单 SQL 用）。
    perm_labels: Vec<String>,
    route_base: String,
    ownership: String,
    dept_pascal: String,
    owner_pascal: String,
    has_create_by: bool,
    has_update_by: bool,
    has_create_time: bool,
    has_update_time: bool,
    date: String,
    seq: String,
    pk: ColumnCtx,
    pk_path_type: String,
    /// 主键实参写法：Copy 类型直接传，`String` 主键需要 `.clone()`。
    pk_clone: String,
    /// 主键在前端/TS 里的类型（`number` / `string`）。
    pk_ts_type: String,
    columns: Vec<ColumnCtx>,
    upsert_columns: Vec<ColumnCtx>,
    /// dto.rs 需要从 `sea_orm::entity::prelude` 引入的类型（`Decimal` / `Uuid` 等）。
    dto_imports: Vec<String>,
    list_columns: Vec<ColumnCtx>,
    query_columns: Vec<ColumnCtx>,
    has_query: bool,
    /// 是否树表（`tpl_category = tree`）。
    is_tree: bool,
    /// 树节点键列（rust_field）。
    tree_pk_field: String,
    /// 树父键列（rust_field）。
    tree_parent_field: String,
    /// 树节点显示名列（rust_field）。
    tree_name_field: String,
    /// 主键在 Rust 侧是不是 `String`（决定树索引是否需要 `.clone()`）。
    pk_is_string: bool,
}

fn temporal_kind(column_type: &str) -> &'static str {
    match column_type {
        "timestamp" | "timestamptz" => "datetime",
        "date" => "date",
        "time" | "timetz" => "time",
        _ => "",
    }
}

fn is_copy_base(base: &str) -> bool {
    matches!(
        base,
        "i16"
            | "i32"
            | "i64"
            | "f32"
            | "f64"
            | "bool"
            | "Decimal"
            | "Uuid"
            | "Date"
            | "Time"
            | "DateTimeUtc"
    )
}

fn column_def(c: &ColumnPlan) -> String {
    let base = match c.column_type.as_str() {
        "int2" => "small_integer()",
        "int4" => "integer()",
        "int8" => "big_integer()",
        "bool" => "boolean()",
        "float4" => "float()",
        "float8" => "double()",
        "numeric" => "decimal()",
        "varchar" | "bpchar" | "char" => "string()",
        "text" => "text()",
        "timestamp" => "timestamp()",
        "timestamptz" => "timestamp_with_time_zone()",
        "date" => "date()",
        "time" | "timetz" => "time()",
        "json" | "jsonb" => "json()",
        "uuid" => "uuid()",
        "bytea" => "binary()",
        _ => "string()",
    };
    let mut out = format!(".{base}");
    if !c.rust_type.starts_with("Option<") {
        out.push_str(".not_null()");
    }
    if c.is_increment {
        out.push_str(".auto_increment()");
    }
    if c.is_pk {
        out.push_str(".primary_key()");
    }
    out
}

fn column_ctx(c: &ColumnPlan, plan: &TablePlan) -> ColumnCtx {
    let temporal = temporal_kind(&c.column_type);
    let optional = c.rust_type.starts_with("Option<");
    let dto_type = if temporal.is_empty() {
        c.rust_type.clone()
    } else if optional {
        "Option<String>".to_string()
    } else {
        "String".to_string()
    };
    let is_string = c.rust_type_base == "String";

    let insert_expr = if temporal.is_empty() {
        if is_copy_base(&c.rust_type_base) {
            format!("Set(req.{})", c.rust_field)
        } else {
            format!("Set(req.{}.clone())", c.rust_field)
        }
    } else {
        let parser = match (temporal, optional) {
            ("datetime", true) => "parse_datetime_opt(req.{f}.as_deref())?",
            ("datetime", false) => "parse_datetime(&req.{f})?",
            ("date", true) => "parse_date_opt(req.{f}.as_deref())?",
            ("date", false) => "parse_date(&req.{f})?",
            ("time", true) => "parse_time_opt(req.{f}.as_deref())?",
            ("time", false) => "parse_time(&req.{f})?",
            _ => unreachable!(),
        }
        .replace("{f}", &c.rust_field);
        format!("Set({parser})")
    };

    let vo_expr = if temporal.is_empty() {
        format!("m.{}", c.rust_field)
    } else {
        let conv = if temporal == "datetime" {
            "to_rfc3339()"
        } else {
            "to_string()"
        };
        if optional {
            format!("m.{}.map(|v| v.{conv})", c.rust_field)
        } else {
            format!("m.{}.{conv}", c.rust_field)
        }
    };

    let (filter_method, filter_arg) = match c.query_type.as_str() {
        "LIKE" => ("contains", "&v"),
        _ => ("eq", "v"),
    };

    let is_tree_parent = plan
        .tree
        .as_ref()
        .is_some_and(|t| t.parent_code == c.column_name);
    let is_tree_name = plan.tree.as_ref().is_some_and(|t| t.name == c.column_name);

    ColumnCtx {
        name: c.column_name.clone(),
        field: c.rust_field.clone(),
        pascal: c.pascal.clone(),
        comment: c.column_comment.clone(),
        rust_type: c.rust_type.clone(),
        rust_base: c.rust_type_base.clone(),
        dto_type,
        query_type: format!("Option<{}>", c.rust_type_base),
        temporal: temporal.to_string(),
        is_string,
        filter_method: filter_method.to_string(),
        filter_arg: filter_arg.to_string(),
        insert_expr,
        vo_expr,
        column_def: column_def(c),
        query_type_cfg: c.query_type.clone(),
        html_type: c.html_type.clone(),
        dict_type: c.dict_type.clone().unwrap_or_default(),
        is_pk: c.is_pk,
        is_required: c.is_required,
        is_list: c.is_list,
        is_query: c.is_query,
        is_insert: c.is_insert,
        is_edit: c.is_edit,
        is_tree_parent,
        is_tree_name,
    }
}

fn build_ctx(input: &RenderInput<'_>) -> Ctx {
    let plan = input.plan;
    let columns: Vec<ColumnCtx> = plan.columns.iter().map(|c| column_ctx(c, plan)).collect();
    // 新增与修改共用同一个 Upsert：模板两处引用同一集合，避免 DTO 与 handler 字段错位。
    let upsert_columns: Vec<ColumnCtx> = columns
        .iter()
        .filter(|c| c.is_insert || c.is_edit)
        .cloned()
        .collect();
    let list_columns: Vec<ColumnCtx> = columns.iter().filter(|c| c.is_list).cloned().collect();
    let query_columns: Vec<ColumnCtx> = columns.iter().filter(|c| c.is_query).cloned().collect();
    let prelude_types: &[&str] = &["Decimal", "Date", "Time", "DateTimeUtc", "Json", "Uuid"];
    let mut dto_imports: Vec<String> = columns
        .iter()
        .filter(|c| c.temporal.is_empty())
        .filter(|c| c.is_list || c.is_insert || c.is_edit || c.is_query)
        .map(|c| c.rust_base.clone())
        .filter(|b| prelude_types.contains(&b.as_str()))
        .collect();
    dto_imports.sort();
    dto_imports.dedup();

    let pk = column_ctx(plan.pk(), plan);
    let is_tree = plan.tpl_category == TplCategory::Tree;
    let pk_is_string = pk.rust_base == "String";
    let tree_pk_field = plan
        .tree
        .as_ref()
        .map(|t| naming::field_ident(&t.code))
        .unwrap_or_else(|| pk.field.clone());
    let tree_parent_field = plan
        .tree
        .as_ref()
        .map(|t| naming::field_ident(&t.parent_code))
        .unwrap_or_default();
    let tree_name_field = plan
        .tree
        .as_ref()
        .map(|t| naming::field_ident(&t.name))
        .unwrap_or_default();
    let perm_prefix = format!("{}:{}", plan.module_name, plan.business_name);
    let perms: Vec<String> = ["list", "add", "edit", "remove"]
        .iter()
        .map(|a| format!("{perm_prefix}:{a}"))
        .collect();

    Ctx {
        table_name: plan.table_name.clone(),
        table_comment: plan.table_comment.clone(),
        class_name: plan.class_name.clone(),
        module_name: plan.module_name.clone(),
        business_name: plan.business_name.clone(),
        struct_name: format!("{}Module", plan.class_name),
        function_name: plan.function_name.clone(),
        function_author: plan.function_author.clone(),
        category: plan.tpl_category.as_str().to_string(),
        perm_prefix,
        perm_labels: ["查询", "新增", "修改", "删除"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        perms,
        route_base: format!(
            "{}/{}/{}",
            input.route_prefix.trim_end_matches('/'),
            plan.module_name,
            plan.business_name
        ),
        ownership: plan.ownership().to_string(),
        dept_pascal: plan
            .column("dept_id")
            .map(|c| c.pascal.clone())
            .unwrap_or_default(),
        owner_pascal: if plan.column("owner_id").is_some() {
            plan.column("owner_id")
                .map(|c| c.pascal.clone())
                .unwrap_or_default()
        } else if plan.column("create_by").is_some() {
            "CreateBy".to_string()
        } else {
            String::new()
        },
        has_create_by: plan.column("create_by").is_some(),
        has_update_by: plan.column("update_by").is_some(),
        has_create_time: plan.column("create_time").is_some(),
        has_update_time: plan.column("update_time").is_some(),
        date: input.date.clone(),
        seq: input.seq.clone(),
        pk_path_type: pk.rust_base.clone(),
        pk_clone: if pk.rust_base == "String" {
            "pk.clone()".to_string()
        } else {
            "pk".to_string()
        },
        pk_ts_type: if pk.rust_base == "String" {
            "string".to_string()
        } else {
            "number".to_string()
        },
        pk,
        columns,
        dto_imports,
        upsert_columns,
        list_columns,
        query_columns,
        has_query: !plan.columns.iter().all(|c| !c.is_query),
        is_tree,
        tree_pk_field,
        tree_parent_field,
        tree_name_field,
        pk_is_string,
    }
}

fn environment(input: &RenderInput<'_>) -> Result<Environment<'static>, CodegenError> {
    let mut env = Environment::new();
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);
    // 产物按 POSIX 文本约定以换行结尾：也让 `cargo fmt` 不会「顺手」改动生成文件。
    env.set_keep_trailing_newline(true);
    let is_tree = input.plan.tpl_category == TplCategory::Tree;
    for def in TEMPLATES {
        // 树表优先用分类专属模板（`tree_source`），否则回落通用模板。
        let builtin = if is_tree {
            def.tree_source.unwrap_or(def.source)
        } else {
            def.source
        };
        let source: String = match &input.template_dir {
            Some(dir) => read_override(dir, def.name).unwrap_or_else(|| builtin.to_string()),
            None => builtin.to_string(),
        };
        env.add_template_owned(def.name, source)
            .map_err(|e| CodegenError::Template(format!("{}：{e}", def.name)))?;
    }
    Ok(env)
}

fn read_override(dir: &Path, name: &str) -> Option<String> {
    let path = dir.join(format!("{name}.j2"));
    match std::fs::read_to_string(&path) {
        Ok(s) => Some(s),
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = %path.display(), error = %e, "读取模板覆写失败，回落内置模板");
            }
            None
        }
    }
}

fn check_category(plan: &TablePlan) -> Result<(), CodegenError> {
    crate::plan::validate_plan(plan)?;
    match plan.tpl_category {
        TplCategory::Crud | TplCategory::Tree => Ok(()),
        other => Err(CodegenError::Unsupported(format!(
            "模板分类 {} 尚未实现（已交付单表 crud 与树表 tree）",
            other.as_str()
        ))),
    }
}

/// 渲染全部产物。
pub fn render_all(input: &RenderInput<'_>) -> Result<Vec<Artifact>, CodegenError> {
    check_category(input.plan)?;
    let ctx = build_ctx(input);
    let env = environment(input)?;
    let mut out = Vec::with_capacity(TEMPLATES.len());
    for def in TEMPLATES {
        let content = env
            .get_template(def.name)
            .and_then(|t| t.render(&ctx))
            .map_err(|e| CodegenError::Template(format!("{}：{e}", def.name)))?;
        let path = env
            .render_str(def.path, &ctx)
            .map_err(|e| CodegenError::Template(format!("路径 {}：{e}", def.name)))?;
        out.push(Artifact { path, content });
    }
    Ok(out)
}

/// 渲染单个产物（预览用）。
pub fn render_one(input: &RenderInput<'_>, name: &str) -> Result<Artifact, CodegenError> {
    check_category(input.plan)?;
    let def = TEMPLATES
        .iter()
        .find(|d| d.name == name)
        .ok_or_else(|| CodegenError::NotFound(format!("未知模板：{name}")))?;
    let ctx = build_ctx(input);
    let env = environment(input)?;
    let content = env
        .get_template(def.name)
        .and_then(|t| t.render(&ctx))
        .map_err(|e| CodegenError::Template(format!("{}：{e}", def.name)))?;
    let path = env
        .render_str(def.path, &ctx)
        .map_err(|e| CodegenError::Template(format!("路径 {}：{e}", def.name)))?;
    Ok(Artifact { path, content })
}

/// 供预览列举的模板名（README 没有预览价值）。
pub fn previewable_templates() -> Vec<&'static str> {
    TEMPLATES
        .iter()
        .filter(|d| d.name != "readme")
        .map(|d| d.name)
        .collect()
}
