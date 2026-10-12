//! cicada-codegen（L3 能力）
//!
//! 代码生成：库表发现 → 类型/命名推断 → 模板渲染 → zip 打包下载。
//!
//! 安全边界（参考实现生成器的两个真实风险，这里从根上收紧）：
//! - **数据面**：只读当前 schema 的 `information_schema`/`pg_catalog`，表名以绑定参数传入，
//!   并先过标识符校验 + 框架表黑名单 + 可选前缀白名单（[`plan::build_plan`]）；
//! - **代码面**：产物路径由模板常量决定（不含用户输入），内容只经 minijinja 插值；
//!   生成的 Rust 必须能通过 `cargo check`（`cargo check` 由 fixture crate 常态验证）。
//!
//! 生成逻辑（[`plan`]）与模板（[`templates`]）分离，模板可被项目覆写。

pub mod bundle;
pub mod discovery;
pub mod entity;
pub mod naming;
pub mod plan;
pub mod render;
pub mod store;
pub mod templates;
pub mod types;

pub use bundle::zip_artifacts;
pub use discovery::{DiscoveredColumn, DiscoveredTable, TableSummary, describe_table, list_tables};
pub use entity::{
    GenTableActiveModel, GenTableCol, GenTableColumnActiveModel, GenTableColumnCol,
    GenTableColumnEntity, GenTableColumnModel, GenTableEntity, GenTableModel,
};
pub use plan::{ColumnPlan, TablePlan, TplCategory, build_plan, is_denied_table, validate_plan};
pub use render::{Artifact, RenderInput, previewable_templates, render_all, render_one};
pub use store::{GenTableQuery, plan_from_database, plan_from_models, save_plan};

/// 生成器错误。web 层据此映射 HTTP 状态码。
#[derive(Debug, thiserror::Error)]
pub enum CodegenError {
    #[error("数据库错误：{0}")]
    Db(String),
    #[error("未找到：{0}")]
    NotFound(String),
    #[error("拒绝：{0}")]
    Rejected(String),
    #[error("尚未支持：{0}")]
    Unsupported(String),
    #[error("模板错误：{0}")]
    Template(String),
    #[error("打包错误：{0}")]
    Bundle(String),
    #[error("状态错误：{0}")]
    State(String),
}

impl CodegenError {
    pub fn from_db(e: impl std::fmt::Display) -> Self {
        Self::Db(e.to_string())
    }
}
