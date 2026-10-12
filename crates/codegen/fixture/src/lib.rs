//! 生成代码的编译 fixture（生成的代码必须开箱可编译）。
//!
//! 本 crate 没有一行手写业务逻辑：它把 `crates/codegen/fixtures/` 下**由生成器产出**
//! 的模块 include 进来。于是 `cargo check --workspace` / `cargo test --workspace`
//! 每次都会重新编译生成产物——模板一旦产出无法编译的代码，这里立刻失败。
//!
//! 重新生成：`UPDATE_FIXTURE=1 cargo test -p cicada-codegen --test fixture`。

// 生成的 handler 一律 `Result<T, ApiErr>`，Err 是 problem+json 结构体（>128 字节）。
// 失败路径构造，装箱换来的收益抵不上每个业务模块重复压一行 lint 的噪音。
#![allow(clippy::result_large_err)]

#[rustfmt::skip]
#[path = "../../fixtures/sample_goods/mod.rs"]
pub mod sample_goods;

// 生成的迁移文件也一起编译（它依赖 `sea-orm-migration`）。
#[rustfmt::skip]
#[path = "../../fixtures/sample_goods/migration.rs"]
pub mod sample_goods_migration;

// 树表（`tpl_category = tree`）样例：验证树形接口与前端页模板同样开箱可编译。
#[rustfmt::skip]
#[path = "../../fixtures/sample_category/mod.rs"]
pub mod sample_category;

#[rustfmt::skip]
#[path = "../../fixtures/sample_category/migration.rs"]
pub mod sample_category_migration;
