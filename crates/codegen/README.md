# cicada-codegen

L3 能力：代码生成器——库表发现 → 类型/命名推断 → 模板渲染 → zip 打包。

## 职责

- `discovery`：只读当前 schema 的 `information_schema` / `pg_catalog`，表名以绑定参数传入。
- `plan`：`TablePlan` 构建与校验；标识符校验 + 框架表黑名单（`is_denied_table`）+ 可选前缀白名单。
- `naming` / `types`：库表名到 Rust/TS 标识符与字段类型的推断。
- `templates` + `render`：minijinja 渲染，产物路径由模板常量决定（不含用户输入）。
- `store`：生成配置（`gen_table` / `gen_table_column`）落库与预览。
- `bundle::zip_artifacts`：一次生成的多文件打成 zip 下载。

已交付模板分类：单表 `crud` 与树表 `tree`。主子表 `sub` 按代码生成 **递延**——
元数据可存，渲染阶段明确报错，不产出半成品。

## 公开 API

`describe_table` / `list_tables` / `DiscoveredTable` / `DiscoveredColumn`、
`build_plan` / `validate_plan` / `is_denied_table` / `TablePlan` / `TplCategory`、
`render_all` / `render_one` / `previewable_templates` / `Artifact`、`zip_artifacts`、`store::*`。

## 依赖

`cicada-kernel` / `cicada-infra` / `cicada-macros` / `sea-orm` / `minijinja` / `zip`。

## 不做什么

- 不写文件到磁盘、不热重载工程：产物以 zip 交给使用者，落盘由 CLI 决定。
- 不保证生成的代码「一步到位可上线」：只保证能 `cargo check` 通过（fixture crate 常态验证）。
