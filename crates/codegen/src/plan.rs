//! 库表 → 生成计划（写 gen_table / gen_table_column 元数据）。
//!
//! 这里是**唯一**决定「生成什么」的地方：字段是否必填、是否进列表、用什么控件、
//! 什么查询方式，全在这里推断一次，之后模板只做纯渲染（可快照）。
//!
//! 契约（生成器对业务表的硬性要求，违反即明确报错，不猜）：
//! 1. 必须有且只有一个单列主键；
//! 2. 必须有 `tenant_id`（受控查询的租户维度）；
//! 3. 必须有 `deleted_at`（软删；`ScopedQuery` 会叠加 `IS NULL`）。
//!
//! 树表（`tpl_category = tree`）在此之上再加一条契约：
//! 4. 必须有非空 `parent_id`，且其类型与主键一致（根节点用 `0` 表示）。

use cicada_infra::GenConfig;

use crate::CodegenError;
use crate::discovery::DiscoveredTable;
use crate::naming;
use crate::types;

/// 模板分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TplCategory {
    /// 单表 CRUD。
    Crud,
    /// 树表。
    Tree,
    /// 主子表。
    Sub,
}

impl TplCategory {
    pub fn parse(s: &str) -> Result<Self, CodegenError> {
        match s {
            "crud" => Ok(Self::Crud),
            "tree" => Ok(Self::Tree),
            "sub" => Ok(Self::Sub),
            other => Err(CodegenError::Rejected(format!("未知模板分类：{other}"))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Crud => "crud",
            Self::Tree => "tree",
            Self::Sub => "sub",
        }
    }
}

/// 单列的生成计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnPlan {
    pub column_name: String,
    pub column_comment: String,
    pub column_type: String,
    pub rust_type: String,
    pub rust_type_base: String,
    pub rust_field: String,
    pub pascal: String,
    pub is_pk: bool,
    pub is_increment: bool,
    pub is_required: bool,
    pub is_insert: bool,
    pub is_edit: bool,
    pub is_list: bool,
    pub is_query: bool,
    pub query_type: String,
    pub html_type: String,
    pub dict_type: Option<String>,
    pub sort: i32,
    /// 框架自带列（租户 / 软删 / 审计），不参与生成的 DTO。
    pub framework: bool,
}

/// 树表元数据：节点键 / 父键 / 显示名三列（模板只按这三列组装树）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeMeta {
    /// 节点键列（取单列主键）。
    pub code: String,
    /// 父键列（约定 `parent_id`）。
    pub parent_code: String,
    /// 节点显示名列（`el-tree-select` 的 label）。
    pub name: String,
}

/// 一张表的生成计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TablePlan {
    pub table_name: String,
    pub table_comment: String,
    pub class_name: String,
    pub module_name: String,
    pub business_name: String,
    pub function_name: String,
    pub function_author: String,
    pub tpl_category: TplCategory,
    pub form_col_num: i16,
    pub gen_type: String,
    /// 树表元数据；`tpl_category != tree` 时为 `None`。
    pub tree: Option<TreeMeta>,
    pub columns: Vec<ColumnPlan>,
}

impl TablePlan {
    pub fn pk(&self) -> &ColumnPlan {
        self.columns
            .iter()
            .find(|c| c.is_pk)
            .expect("plan 保证存在主键列")
    }

    /// 归属形态：`DeptAndOwner` / `DeptOnly` / `OwnerOnly` / `NoOwnership`。
    pub fn ownership(&self) -> &'static str {
        let has_dept = self.column("dept_id").is_some();
        let has_owner = self.column("owner_id").is_some() || self.column("create_by").is_some();
        match (has_dept, has_owner) {
            (true, true) => "DeptAndOwner",
            (true, false) => "DeptOnly",
            (false, true) => "OwnerOnly",
            (false, false) => "NoOwnership",
        }
    }

    pub fn column(&self, name: &str) -> Option<&ColumnPlan> {
        self.columns.iter().find(|c| c.column_name == name)
    }
}

/// 生成计划的二次校验：`module_name` / `business_name` 会变成产物路径的一段，
/// `class_name` 会变成 Rust 类型名。这两个入口都可能来自「在线编辑」，因此必须
/// 在渲染前把它们钉死成标识符，杜绝 `../` 之类的路径穿越。
pub fn validate_plan(plan: &TablePlan) -> Result<(), CodegenError> {
    for (label, value) in [
        ("module_name", plan.module_name.as_str()),
        ("business_name", plan.business_name.as_str()),
    ] {
        if !naming::is_valid_identifier(value) {
            return Err(CodegenError::Rejected(format!(
                "{label} 不是合法标识符：{value}"
            )));
        }
    }
    let mut chars = plan.class_name.chars();
    let ok_head = matches!(chars.next(), Some(c) if c.is_ascii_uppercase());
    if !ok_head
        || plan.class_name.len() > 63
        || !plan.class_name.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return Err(CodegenError::Rejected(format!(
            "class_name 必须是 PascalCase 标识符：{}",
            plan.class_name
        )));
    }
    if plan.columns.is_empty() {
        return Err(CodegenError::Rejected("生成计划没有列".to_string()));
    }
    if plan.tpl_category == TplCategory::Tree {
        let tree = plan
            .tree
            .as_ref()
            .ok_or_else(|| CodegenError::State("树表计划缺少 tree 元数据".to_string()))?;
        for name in [&tree.code, &tree.parent_code, &tree.name] {
            if plan.column(name).is_none() {
                return Err(CodegenError::Rejected(format!(
                    "树表引用了不存在的列：{name}"
                )));
            }
        }
    }
    Ok(())
}

/// 树表父键列的约定名（与 `tenant_id` / `deleted_at` 同为契约，不做每表配置）。
pub const TREE_PARENT_COLUMN: &str = "parent_id";

/// 树表契约的落地点：把 `parent_id` 从等值查询里拿掉（前端用树接口定位），
/// 再推导出节点键 / 父键 / 显示名三列。`build_plan` 与 `plan_from_models`
/// 共用，保证「导入时」与「改完再预览时」得到同一份树元数据。
pub(crate) fn apply_tree_contract(
    columns: &mut [ColumnPlan],
    category: TplCategory,
) -> Result<Option<TreeMeta>, CodegenError> {
    if category != TplCategory::Tree {
        return Ok(None);
    }
    if let Some(parent) = columns
        .iter_mut()
        .find(|c| c.column_name == TREE_PARENT_COLUMN)
    {
        parent.is_query = false;
    }
    Ok(Some(derive_tree(columns)?))
}

/// 节点键取主键；父键取 `parent_id`；显示名取第一个字符串业务列（优先 `*_name`）。
fn derive_tree(columns: &[ColumnPlan]) -> Result<TreeMeta, CodegenError> {
    let pk = columns
        .iter()
        .find(|c| c.is_pk)
        .ok_or_else(|| CodegenError::Rejected("树表需要单列主键".to_string()))?;
    let parent = columns
        .iter()
        .find(|c| c.column_name == TREE_PARENT_COLUMN)
        .ok_or_else(|| {
            CodegenError::Rejected(format!(
                "树表需要 `{TREE_PARENT_COLUMN}` 列作为父键（与 `tenant_id` / `deleted_at` 同为生成契约）"
            ))
        })?;
    if parent.rust_type_base != pk.rust_type_base {
        return Err(CodegenError::Rejected(format!(
            "树表父键 {} 的类型 {} 与主键 {} 的类型 {} 不一致",
            parent.column_name, parent.rust_type_base, pk.column_name, pk.rust_type_base
        )));
    }
    if parent.rust_type.starts_with("Option<") {
        return Err(CodegenError::Rejected(format!(
            "树表父键 {} 必须 NOT NULL（根节点用 0 表示）",
            parent.column_name
        )));
    }
    let is_label_candidate = |c: &&ColumnPlan| {
        !c.framework
            && !c.is_pk
            && c.column_name != TREE_PARENT_COLUMN
            && c.rust_type_base == "String"
    };
    let name = columns
        .iter()
        .find(|c| is_label_candidate(c) && c.column_name.ends_with("_name"))
        .or_else(|| columns.iter().find(is_label_candidate))
        .ok_or_else(|| {
            CodegenError::Rejected(
                "树表需要至少一个字符串业务列作为节点显示名（优先 `*_name`）".to_string(),
            )
        })?;
    Ok(TreeMeta {
        code: pk.column_name.clone(),
        parent_code: TREE_PARENT_COLUMN.to_string(),
        name: name.column_name.clone(),
    })
}

/// 框架表：生成器拒绝为它们产出代码（会覆盖框架自身的实现）。
const DENIED_TABLES: &[&str] = &[
    "sys_user",
    "sys_role",
    "sys_menu",
    "sys_dept",
    "sys_post",
    "sys_dict_type",
    "sys_dict_data",
    "sys_config",
    "sys_notice",
    "sys_oper_log",
    "sys_logininfor",
    "sys_user_session",
    "sys_file",
    "sys_job",
    "sys_job_log",
    "sys_identity",
    "gen_table",
    "gen_table_column",
    "_sqlx_migrations",
    "seaql_migrations",
    "apalis_jobs",
];

/// 是否拒绝为这张表生成（框架表 + 项目黑名单）。
pub fn is_denied_table(table: &str, cfg: &GenConfig) -> bool {
    DENIED_TABLES.contains(&table) || cfg.deny_tables.iter().any(|t| t == table)
}

/// 把发现到的库表结构整理成生成计划。任何契约违反都在这里失败。
pub fn build_plan(
    discovered: &DiscoveredTable,
    cfg: &GenConfig,
    tpl_category: TplCategory,
) -> Result<TablePlan, CodegenError> {
    let table = discovered.table_name.as_str();
    if is_denied_table(table, cfg) {
        return Err(CodegenError::Rejected(format!(
            "拒绝为框架表或黑名单表生成：{table}"
        )));
    }
    if !cfg.allowed_prefixes.is_empty()
        && !cfg
            .allowed_prefixes
            .iter()
            .any(|p| !p.is_empty() && table.starts_with(p.as_str()))
    {
        return Err(CodegenError::Rejected(format!(
            "表名 {table} 不在允许的前缀列表内：{:?}",
            cfg.allowed_prefixes
        )));
    }
    if discovered.pk_count() != 1 {
        return Err(CodegenError::Rejected(format!(
            "表 {table} 需要恰好一个单列主键，实际 {} 个（复合主键暂不支持）",
            discovered.pk_count()
        )));
    }
    for required in ["tenant_id", "deleted_at"] {
        if discovered.column(required).is_none() {
            return Err(CodegenError::Rejected(format!(
                "表 {table} 缺少 {required} 列（生成器要求租户列与软删列，见生成器契约）"
            )));
        }
    }

    let class_name = naming::class_name(table, &cfg.strip_prefixes);
    let business_name = naming::business_name(table, &cfg.strip_prefixes);
    let module_name = naming::module_name(table, &cfg.strip_prefixes, &cfg.module_name);

    let mut columns = Vec::with_capacity(discovered.columns.len());
    for col in &discovered.columns {
        let field = naming::field_ident(&col.name);
        let pascal = naming::pascal_case(&field);
        let framework = types::is_framework_column(&col.name);
        let dict = types::dict_type(&col.name, &col.udt_name);
        let rust = types::rust_type(&col.udt_name, col.nullable);
        let required = !col.nullable && !col.is_pk && col.default.is_none() && !framework;
        // 不透明列（二进制 / JSON）不进 DTO：自动生成的 JSON 表单表达不了它们。
        let opaque = matches!(col.udt_name.as_str(), "json" | "jsonb" | "bytea");
        let temporal = matches!(
            col.udt_name.as_str(),
            "date" | "timestamp" | "timestamptz" | "time" | "timetz"
        );
        let is_query = !framework
            && !col.is_pk
            && !col.is_increment
            && !opaque
            && !temporal
            && !matches!(col.udt_name.as_str(), "text");
        columns.push(ColumnPlan {
            column_name: col.name.clone(),
            column_comment: if col.comment.trim().is_empty() {
                col.name.clone()
            } else {
                col.comment.trim().to_string()
            },
            column_type: col.udt_name.clone(),
            rust_type: rust.render(),
            rust_type_base: rust.base.to_string(),
            rust_field: field,
            pascal,
            is_pk: col.is_pk,
            is_increment: col.is_increment,
            is_required: required,
            is_insert: !framework && !opaque && !col.is_increment && !col.is_pk,
            is_edit: !framework && !opaque && !col.is_pk,
            is_list: !framework && !opaque,
            is_query,
            query_type: types::query_type(&col.name, &col.udt_name).to_string(),
            html_type: types::html_type(&col.name, &col.udt_name, dict),
            dict_type: dict.map(|d| d.to_string()),
            sort: col.ordinal,
            framework,
        });
    }

    if !columns.iter().any(|c| c.is_insert || c.is_edit) {
        return Err(CodegenError::Rejected(format!(
            "表 {table} 没有可生成的业务列（全部是框架列/不透明列）"
        )));
    }

    // 树表在通用列推断之后叠加自己的契约（父键不进查询、推导三列）。
    let tree = apply_tree_contract(&mut columns, tpl_category)?;

    let function_name = if discovered.comment.trim().is_empty() {
        business_name.clone()
    } else {
        discovered.comment.trim().to_string()
    };

    Ok(TablePlan {
        table_name: table.to_string(),
        table_comment: discovered.comment.trim().to_string(),
        class_name,
        module_name,
        business_name,
        function_name,
        function_author: cfg.author.clone(),
        tpl_category,
        form_col_num: 1,
        gen_type: "0".to_string(),
        tree,
        columns,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::DiscoveredColumn;

    fn col(name: &str, udt: &str, nullable: bool, pk: bool) -> DiscoveredColumn {
        DiscoveredColumn {
            name: name.to_string(),
            comment: String::new(),
            udt_name: udt.to_string(),
            data_type: udt.to_string(),
            length: None,
            nullable,
            default: None,
            is_pk: pk,
            is_increment: pk && udt == "int8",
            ordinal: 1,
        }
    }

    fn table(name: &str, columns: Vec<DiscoveredColumn>) -> DiscoveredTable {
        DiscoveredTable {
            table_name: name.to_string(),
            comment: "商品".to_string(),
            columns,
        }
    }

    fn sample() -> DiscoveredTable {
        table(
            "biz_goods",
            vec![
                col("goods_id", "int8", false, true),
                col("tenant_id", "int8", false, false),
                col("goods_name", "varchar", false, false),
                col("price", "numeric", true, false),
                col("deleted_at", "timestamptz", true, false),
                col("create_time", "timestamptz", false, false),
            ],
        )
    }

    #[test]
    fn builds_crud_plan() {
        let mut cfg = GenConfig::default();
        cfg.strip_prefixes.push("biz_".to_string());
        let plan = build_plan(&sample(), &cfg, TplCategory::Crud).unwrap();
        assert_eq!(plan.class_name, "Goods");
        assert_eq!(plan.business_name, "goods");
        assert_eq!(plan.module_name, "business");
        assert_eq!(plan.ownership(), "NoOwnership");
        // 默认配置不做前缀剥离（命名可预测）。
        let bare = build_plan(&sample(), &GenConfig::default(), TplCategory::Crud).unwrap();
        assert_eq!(bare.class_name, "BizGoods");
        assert!(plan.pk().is_pk);
        assert_eq!(plan.pk().rust_field, "goods_id");
        // 框架列不进 DTO。
        assert!(plan.column("tenant_id").unwrap().framework);
        assert!(!plan.column("tenant_id").unwrap().is_insert);
        assert!(plan.column("deleted_at").unwrap().framework);
        // 业务列进 DTO。
        assert!(plan.column("goods_name").unwrap().is_insert);
        assert!(plan.column("goods_name").unwrap().is_query);
        assert!(!plan.column("create_time").unwrap().is_query);
    }

    #[test]
    fn rejects_missing_tenant_or_soft_delete() {
        let mut t = sample();
        t.columns.retain(|c| c.name != "tenant_id");
        assert!(build_plan(&t, &GenConfig::default(), TplCategory::Crud).is_err());

        let mut t = sample();
        t.columns.retain(|c| c.name != "deleted_at");
        assert!(build_plan(&t, &GenConfig::default(), TplCategory::Crud).is_err());
    }

    #[test]
    fn validates_plan_identifiers() {
        let mut plan = build_plan(&sample(), &GenConfig::default(), TplCategory::Crud).unwrap();
        assert!(validate_plan(&plan).is_ok());
        plan.module_name = "../../etc".to_string();
        assert!(validate_plan(&plan).is_err());
        plan.module_name = "business".to_string();
        plan.class_name = "goods".to_string();
        assert!(validate_plan(&plan).is_err());
    }

    #[test]
    fn rejects_framework_tables_and_denylist() {
        let t = table(
            "sys_user",
            vec![
                col("user_id", "int8", false, true),
                col("tenant_id", "int8", false, false),
                col("deleted_at", "timestamptz", true, false),
            ],
        );
        assert!(build_plan(&t, &GenConfig::default(), TplCategory::Crud).is_err());

        let mut cfg = GenConfig::default();
        cfg.deny_tables.push("biz_goods".to_string());
        assert!(build_plan(&sample(), &cfg, TplCategory::Crud).is_err());
    }

    fn tree_table(parent: DiscoveredColumn) -> DiscoveredTable {
        let cols = vec![
            col("cat_id", "int8", false, true),
            col("tenant_id", "int8", false, false),
            parent,
            col("cat_name", "varchar", false, false),
            col("deleted_at", "timestamptz", true, false),
        ];
        table("biz_category", cols)
    }

    #[test]
    fn builds_tree_plan() {
        let t = tree_table(col("parent_id", "int8", false, false));
        let plan = build_plan(&t, &GenConfig::default(), TplCategory::Tree).unwrap();
        let tree = plan.tree.as_ref().expect("树元数据");
        assert_eq!(tree.code, "cat_id");
        assert_eq!(tree.parent_code, "parent_id");
        assert_eq!(tree.name, "cat_name");
        // 父键不进等值查询（前端走树接口定位），但仍保留在 Vo 里。
        assert!(!plan.column("parent_id").unwrap().is_query);
        assert!(plan.column("parent_id").unwrap().is_list);
        assert!(validate_plan(&plan).is_ok());
    }

    #[test]
    fn rejects_tree_contract_violations() {
        // 缺 parent_id。
        let cols = vec![
            col("cat_id", "int8", false, true),
            col("tenant_id", "int8", false, false),
            col("cat_name", "varchar", false, false),
            col("deleted_at", "timestamptz", true, false),
        ];
        assert!(
            build_plan(
                &table("biz_category", cols),
                &GenConfig::default(),
                TplCategory::Tree
            )
            .is_err()
        );

        // 父键类型与主键不一致。
        let t = tree_table(col("parent_id", "varchar", false, false));
        assert!(build_plan(&t, &GenConfig::default(), TplCategory::Tree).is_err());

        // 父键可空：树索引会拿到 None，明确拒绝。
        let t = tree_table(col("parent_id", "int8", true, false));
        assert!(build_plan(&t, &GenConfig::default(), TplCategory::Tree).is_err());
    }

    #[test]
    fn rejects_compound_primary_key() {
        let mut t = sample();
        t.columns.push(col("sku_id", "int8", false, true));
        assert!(build_plan(&t, &GenConfig::default(), TplCategory::Crud).is_err());
    }
}
