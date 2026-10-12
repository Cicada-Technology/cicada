//! 生成器测试（生成器测试：快照 + 编译）。
//!
//! - 编译：产物被 `crates/codegen/fixture`（工作区成员）include，随
//!   `cargo check --workspace` 常态编译；
//! - 快照：本测试把「模板 + 样例表」重新渲染一遍，与 fixtures 下已提交的产物逐字节比对，
//!   模板漂移会在这里失败。`UPDATE_FIXTURE=1` 时改为写入（重新生成）。

use std::path::{Path, PathBuf};

use cicada_codegen::{
    DiscoveredColumn, DiscoveredTable, RenderInput, TplCategory, build_plan, render_all,
    zip_artifacts,
};
use cicada_infra::GenConfig;

fn col(name: &str, udt: &str, comment: &str, nullable: bool, pk: bool) -> DiscoveredColumn {
    DiscoveredColumn {
        name: name.to_string(),
        comment: comment.to_string(),
        udt_name: udt.to_string(),
        data_type: udt.to_string(),
        length: None,
        nullable,
        default: None,
        is_pk: pk,
        is_increment: pk,
        ordinal: 0,
    }
}

/// 样例表：覆盖主键/框架列/关键字列/数值/布尔/日期/时间/可空时间/文本。
fn sample_table() -> DiscoveredTable {
    let cols = vec![
        col("goods_id", "int8", "商品 id", false, true),
        col("tenant_id", "int8", "租户", false, false),
        col("dept_id", "int8", "归属部门", false, false),
        col("goods_name", "varchar", "商品名称", false, false),
        col("type", "varchar", "商品类型", false, false),
        col("price", "numeric", "价格", true, false),
        col("quantity", "int4", "库存", false, false),
        col("online", "bool", "是否上架", false, false),
        col("publish_date", "date", "上架日期", true, false),
        col("publish_at", "timestamptz", "上架时间", true, false),
        col("remark", "text", "备注", true, false),
        col("create_by", "varchar", "创建人", true, false),
        col("create_time", "timestamptz", "创建时间", false, false),
        col("update_by", "varchar", "更新人", true, false),
        col("update_time", "timestamptz", "更新时间", true, false),
        col("deleted_at", "timestamptz", "软删", true, false),
    ];
    let mut cols = cols;
    for (i, c) in cols.iter_mut().enumerate() {
        c.ordinal = i as i32 + 1;
    }
    DiscoveredTable {
        table_name: "biz_goods".to_string(),
        comment: "商品".to_string(),
        columns: cols,
    }
}

fn config() -> GenConfig {
    let mut cfg = GenConfig::default();
    cfg.strip_prefixes.push("biz_".to_string());
    cfg
}

/// 树表样例：`parent_id` 非空（根=0）、有 `*_name` 标签列、含归属与框架列。
fn sample_tree_table() -> DiscoveredTable {
    let mut cols = vec![
        col("cat_id", "int8", "分类 id", false, true),
        col("tenant_id", "int8", "租户", false, false),
        col("dept_id", "int8", "归属部门", false, false),
        col("parent_id", "int8", "上级分类", false, false),
        col("cat_name", "varchar", "分类名称", false, false),
        col("sort_order", "int4", "显示顺序", false, false),
        col("online", "bool", "是否启用", false, false),
        col("remark", "text", "备注", true, false),
        col("create_by", "varchar", "创建人", true, false),
        col("create_time", "timestamptz", "创建时间", false, false),
        col("update_by", "varchar", "更新人", true, false),
        col("update_time", "timestamptz", "更新时间", true, false),
        col("deleted_at", "timestamptz", "软删", true, false),
    ];
    for (i, c) in cols.iter_mut().enumerate() {
        c.ordinal = i as i32 + 1;
    }
    DiscoveredTable {
        table_name: "biz_category".to_string(),
        comment: "商品分类".to_string(),
        columns: cols,
    }
}

fn render() -> Vec<cicada_codegen::Artifact> {
    let plan = build_plan(&sample_table(), &config(), TplCategory::Crud).expect("建计划");
    let input = RenderInput {
        plan: &plan,
        date: "20261009".to_string(),
        seq: "000008".to_string(),
        route_prefix: "/api/v1".to_string(),
        template_dir: None,
    };
    render_all(&input).expect("渲染")
}

fn render_tree() -> Vec<cicada_codegen::Artifact> {
    let plan = build_plan(&sample_tree_table(), &config(), TplCategory::Tree).expect("建树计划");
    let input = RenderInput {
        plan: &plan,
        date: "20261009".to_string(),
        seq: "000009".to_string(),
        route_prefix: "/api/v1".to_string(),
        template_dir: None,
    };
    render_all(&input).expect("渲染树表")
}

/// 需要被编译、因此需要落盘的产物。
///
/// 第一项是产物路径的后缀（唯一即可），第二项是 fixtures 目录下的文件名。
/// `migration.rs` 不是 mod 的子模块，而是被 fixture crate 单独 include 的模块。
const FIXTURE_FILES: &[(&str, &str)] = &[
    ("mod.rs", "mod.rs"),
    ("entity.rs", "entity.rs"),
    ("dto.rs", "dto.rs"),
    ("handler.rs", "handler.rs"),
    ("module.rs", "module.rs"),
    ("migration/m20261009_000008_goods.rs", "migration.rs"),
];

/// 树表样例的落盘文件（与单表一致：路径后缀 → fixtures 文件名）。
const FIXTURE_FILES_TREE: &[(&str, &str)] = &[
    ("mod.rs", "mod.rs"),
    ("entity.rs", "entity.rs"),
    ("dto.rs", "dto.rs"),
    ("handler.rs", "handler.rs"),
    ("module.rs", "module.rs"),
    ("migration/m20261009_000009_category.rs", "migration.rs"),
];

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/sample_goods")
}

fn fixtures_tree_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/sample_category")
}

/// `template/` 是使用者工程的骨架，里面的样例业务模块**就是**生成器产物。
///
/// 两者必须逐字节一致（这样「模板里的样例能跑」与「生成器产物能跑」永远是一回事）；
/// 模板改了样例或生成器改了模板，这里失败。同步方向只有一个：
/// `UPDATE_FIXTURE=1 cargo test -p cicada-codegen --test fixture` 之后
/// 把 `fixtures/sample_goods/*` 复制到 `template/`（见下面 case 的提示）。
#[test]
fn template_embeds_the_same_generated_module() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let template_goods = root.join("template/business/goods");
    let fixtures = fixtures_dir();
    let mut compared = 0;
    for (_suffix, file) in FIXTURE_FILES {
        let name = Path::new(file)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap();
        // 迁移产物在模板里是「业务迁移」目录下的一个文件，不在业务模块目录里。
        let (template_path, fixture_path) = match name {
            "migration" => (
                root.join("template/migrations/m20261015_000001_goods.rs"),
                fixtures.join("migration.rs"),
            ),
            other => (
                template_goods.join(format!("{other}.rs")),
                fixtures.join(file),
            ),
        };
        let want = std::fs::read_to_string(&fixture_path)
            .unwrap_or_else(|e| panic!("读取 {fixture_path:?} 失败：{e}"));
        let got = std::fs::read_to_string(&template_path)
            .unwrap_or_else(|e| panic!("读取 {template_path:?} 失败：{e}"));
        assert_eq!(
            got,
            want,
            "{} 与生成器产物不一致：重新生成后请从 {} 同步（单表样例是同一份代码）",
            template_path.display(),
            fixture_path.display()
        );
        compared += 1;
    }
    assert_eq!(compared, FIXTURE_FILES.len());
}

#[test]
fn fixtures_match_templates() {
    let artifacts = render();
    let dir = fixtures_dir();
    let update = std::env::var("UPDATE_FIXTURE").is_ok();
    if update {
        std::fs::create_dir_all(&dir).expect("建 fixtures 目录");
    }
    for (suffix, file) in FIXTURE_FILES {
        let artifact = artifacts
            .iter()
            .find(|a| a.path.ends_with(suffix))
            .unwrap_or_else(|| panic!("缺少产物 {suffix}"));
        let path = dir.join(file);
        if update {
            std::fs::write(&path, &artifact.content).expect("写 fixture");
            continue;
        }
        let committed = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("读取 {path:?} 失败：{e}（试 UPDATE_FIXTURE=1）"));
        assert_eq!(
            committed,
            artifact.content,
            "{} 与模板不一致：模板改了就要 UPDATE_FIXTURE=1 重新生成",
            path.display()
        );
    }
}

#[test]
fn renders_all_expected_artifacts() {
    let artifacts = render();
    let paths: Vec<&str> = artifacts.iter().map(|a| a.path.as_str()).collect();
    for expected in [
        "migration/m20261009_000008_goods.rs",
        "src/business/goods/mod.rs",
        "src/business/goods/entity.rs",
        "src/business/goods/dto.rs",
        "src/business/goods/handler.rs",
        "src/business/goods/module.rs",
        "web/src/api/business/goods.ts",
        "web/src/views/business/goods/index.vue",
        "sql/goods_menu.sql",
        "README.md",
    ] {
        assert!(
            paths.contains(&expected),
            "缺少产物：{expected}（实际 {paths:?}）"
        );
    }
    assert!(artifacts.iter().all(|a| !a.content.trim().is_empty()));
    // 生成的 Rust 不允许出现占位宏（硬约束）。
    for a in artifacts.iter().filter(|a| a.path.ends_with(".rs")) {
        for banned in ["todo!()", "unimplemented!()", "panic!(\""] {
            assert!(!a.content.contains(banned), "{} 含占位：{banned}", a.path);
        }
    }
}

#[test]
fn entity_marks_ownership_and_escapes_keyword_columns() {
    let artifacts = render();
    let entity = artifacts
        .iter()
        .find(|a| a.path.ends_with("entity.rs"))
        .expect("entity");
    assert!(entity.content.contains("dept = Column::DeptId"));
    assert!(entity.content.contains("owner = Column::CreateBy"));
    assert!(entity.content.contains("pub type_col: String,"));
    assert!(entity.content.contains("pub price: Option<Decimal>,"));
}

#[test]
fn bundles_into_a_zip() {
    let bytes = zip_artifacts(&render()).expect("打包");
    assert_eq!(&bytes[..2], b"PK", "应是 zip 魔数");
    assert!(bytes.len() > 1024);

    // 读回中央目录：条目路径必须与产物一致（zip 可被解压工具识别）。
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip 应可读回");
    let mut names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    names.sort();
    assert_eq!(names, {
        let mut expected: Vec<&str> = vec![
            "README.md",
            "migration/m20261009_000008_goods.rs",
            "sql/goods_menu.sql",
            "src/business/goods/dto.rs",
            "src/business/goods/entity.rs",
            "src/business/goods/handler.rs",
            "src/business/goods/mod.rs",
            "src/business/goods/module.rs",
            "web/src/api/business/goods.ts",
            "web/src/views/business/goods/index.vue",
        ];
        expected.sort();
        expected
    });
}

#[test]
fn tree_fixtures_match_templates() {
    let artifacts = render_tree();
    let dir = fixtures_tree_dir();
    let update = std::env::var("UPDATE_FIXTURE").is_ok();
    if update {
        std::fs::create_dir_all(&dir).expect("建 fixtures 目录");
    }
    for (suffix, file) in FIXTURE_FILES_TREE {
        let artifact = artifacts
            .iter()
            .find(|a| a.path.ends_with(suffix))
            .unwrap_or_else(|| panic!("缺少树表产物 {suffix}"));
        let path = dir.join(file);
        if update {
            std::fs::write(&path, &artifact.content).expect("写树表 fixture");
            continue;
        }
        let committed = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("读取 {path:?} 失败：{e}（试 UPDATE_FIXTURE=1）"));
        assert_eq!(
            committed,
            artifact.content,
            "{} 与模板不一致：模板改了就要 UPDATE_FIXTURE=1 重新生成",
            path.display()
        );
    }
}

#[test]
fn tree_renders_forest_endpoint_and_children() {
    let artifacts = render_tree();
    let handler = artifacts
        .iter()
        .find(|a| a.path.ends_with("handler.rs"))
        .expect("handler");
    assert!(handler.content.contains("pub async fn tree_models"));
    assert!(handler.content.contains("fn assemble_tree"));
    assert!(
        handler
            .content
            .contains("\"/api/v1/business/category/tree\"")
    );

    let dto = artifacts
        .iter()
        .find(|a| a.path.ends_with("dto.rs"))
        .expect("dto");
    assert!(dto.content.contains("pub children: Vec<CategoryVo>"));

    let vue = artifacts
        .iter()
        .find(|a| a.path.ends_with("index.vue"))
        .expect("vue");
    // 树表页用 row-key + tree-props，且父键用 el-tree-select 而非普通输入框。
    assert!(vue.content.contains("row-key=\"cat_id\""));
    assert!(
        vue.content
            .contains(":tree-props=\"{ children: 'children' }\"")
    );
    assert!(vue.content.contains("el-tree-select"));
    assert!(vue.content.contains("treeCategory"));

    let api = artifacts
        .iter()
        .find(|a| a.path.ends_with(".ts"))
        .expect("api");
    assert!(api.content.contains("/business/category/tree"));
}

#[test]
fn refuses_sub_templates_for_now() {
    let plan = build_plan(&sample_table(), &config(), TplCategory::Sub).expect("建计划");
    let input = RenderInput {
        plan: &plan,
        date: "20261009".to_string(),
        seq: "000008".to_string(),
        route_prefix: "/api/v1".to_string(),
        template_dir: None,
    };
    assert!(render_all(&input).is_err());
}

#[test]
fn template_overrides_take_precedence() {
    let plan = build_plan(&sample_table(), &config(), TplCategory::Crud).expect("建计划");
    let dir = std::env::temp_dir().join(format!("cicada-codegen-tpl-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时模板目录");
    std::fs::write(dir.join("entity.j2"), "// overridden\n").expect("写覆写模板");
    let input = RenderInput {
        plan: &plan,
        date: "20261009".to_string(),
        seq: "000008".to_string(),
        route_prefix: "/api/v1".to_string(),
        template_dir: Some(dir.clone()),
    };
    let artifacts = render_all(&input).expect("渲染");
    let entity = artifacts
        .iter()
        .find(|a| a.path.ends_with("entity.rs"))
        .expect("entity");
    assert_eq!(entity.content.trim(), "// overridden");
    let _ = std::fs::remove_dir_all(&dir);
}
