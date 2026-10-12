//! 商品分类（由 cicada 代码生成器产出，作者：cicada）。
//!
//! 注册到应用组合根：
//!
//! ```ignore
//! AppBuilder::new(config).with_module(Box::new(
//!     business::category::CategoryModule,
//! ))
//! ```
//!
//! 表 `biz_category` 的属主是本模块；权限点前缀 `business:category`。

pub mod dto;
pub mod entity;
pub mod handler;
pub mod module;

pub use module::CategoryModule;
