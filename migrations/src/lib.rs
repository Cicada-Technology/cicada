//! cicada-migrations（sea-orm-migration 迁移工程）
//!
//! Rust 迁移。

pub use sea_orm_migration::prelude::*;

mod m20261008_000001_core_schema;
mod m20261008_000002_dict_config_notice;
mod m20261009_000004_audit_logs;
mod m20261009_000005_file;
mod m20261009_000006_job;
mod m20261009_000007_gen;
mod m20261010_000008_tenant_package_notify_apikey;
mod m20261016_000010_export_task;
mod m20261016_000011_outbox;
mod m20261017_000012_sys_audit_log;
mod m20261017_000013_log_partitions;
mod m20261017_000014_sys_user_online;
mod m20261017_000015_trgm_indexes;
mod m20261019_000017_oper_log_context;
mod m20261020_000018_menu_refresh;

pub mod init_admin;
pub mod seed;

/// 模糊检索列与对应 trgm 索引名（迁移 000015）。
///
/// 清单是「`crates/web` 的 fuzzy 过滤列」的镜像：建索引的一端与核对测试的另一端
/// 必须读同一份数据，否则一边加了搜索框、另一边静默没有索引。
pub use m20261017_000015_trgm_indexes::{FUZZY_COLUMNS, trgm_index_name};

/// `sys_oper_log` 新增列的长度上限（迁移与写入端共用，避免两边各写一个数）。
pub use m20261019_000017_oper_log_context::{BIZ_CODE_LEN, REQ_BODY_LEN, USER_AGENT_LEN};

/// 框架自带迁移的完整列表（按版本顺序）。
///
/// 序号只增不补：`000003` 曾用于 `sys_user` 的 TOTP 二次验证列，现已并入
/// `m20261008_000001_core_schema` 的建表（一张表只建一次、一次建成最终形态）。
///
/// 使用者工程把它拼到自己的迁移列表前面，从而**只有一个** `Migrator`、
/// 一张 `seaql_migrations` 表（迁移 append-only）：
///
/// ```ignore
/// impl MigratorTrait for Migrator {
///     fn migrations() -> Vec<Box<dyn MigrationTrait>> {
///         let mut all = cicada::migrations::framework_migrations();
///         all.push(Box::new(m20261015_000001_goods::Migration));
///         all
///     }
/// }
/// ```
///
/// 两个 `MigratorTrait` 实现共用一个迁移表是不行的：SeaORM 会把「对不上本列表的
/// 已应用版本」判为错误（`Migration file ... is missing`）。
pub fn framework_migrations() -> Vec<Box<dyn MigrationTrait>> {
    vec![
        Box::new(m20261008_000001_core_schema::Migration),
        Box::new(m20261008_000002_dict_config_notice::Migration),
        Box::new(m20261009_000004_audit_logs::Migration),
        Box::new(m20261009_000005_file::Migration),
        Box::new(m20261009_000006_job::Migration),
        Box::new(m20261009_000007_gen::Migration),
        Box::new(m20261010_000008_tenant_package_notify_apikey::Migration),
        Box::new(m20261016_000010_export_task::Migration),
        Box::new(m20261016_000011_outbox::Migration),
        Box::new(m20261017_000012_sys_audit_log::Migration),
        Box::new(m20261017_000013_log_partitions::Migration),
        Box::new(m20261017_000014_sys_user_online::Migration),
        Box::new(m20261017_000015_trgm_indexes::Migration),
        Box::new(m20261019_000017_oper_log_context::Migration),
        Box::new(m20261020_000018_menu_refresh::Migration),
    ]
}

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        framework_migrations()
    }
}
