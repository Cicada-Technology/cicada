//! 本工程的迁移列表（数据库迁移 append-only）。
//!
//! 只注册**一个** `Migrator`：框架迁移排在前面（`framework_migrations()`），
//! 你自己的表追加在后面。两段共用一个 `seaql_migrations` 表，因此顺序天然确定，
//! 也不会出现「两个 Migrator 互认对方版本」的报错。
//!
//! 新增一张表：
//!
//! 1. 新建 `migrations/m<日期>_<序号>_<表名>.rs`（版本号不要与框架已用的冲突）；
//! 2. 在下面 `migrations()` 的末尾追加一行；
//! 3. `cargo run --bin migrate -- up`。
//!
//! 代码生成器产出的建表迁移也放这里（表已存在时 `if_not_exists` 是空操作）。

use cicada::sea_orm_migration::prelude::*;

mod m20261015_000001_goods;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        let mut all = cicada::migrations::framework_migrations();
        all.push(Box::new(m20261015_000001_goods::Migration));
        all
    }
}
