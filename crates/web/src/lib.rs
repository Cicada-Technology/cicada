//! cicada-web（L4 接入层）
//!
//! axum 路由、中间件链、统一响应、OpenAPI 汇总，以及**模块注册契约**
//! （`Module` / `Manifest` / `Slot` / `Assembler`，最终设计见
//! `docs/superpowers/design-it-twice/FINAL.md`）。

// rust-i18n 消息目录（`locales/*.yml`）：宏在 crate 根生成 `_rust_i18n_t` 等内部项，
// `t!(..., locale = ...)` 必须能在各模块经 `crate::` 路径命中它们。
// axum handler 的 `Result<T, ApiErr>` 里 Err 分支是 problem+json 结构体（约 160 字节），
// 远超 clippy 的 128 字节阈值。错误体只在失败路径构造，装箱换来的收益抵不上
// 每个 handler 多一层 `Box` 的 API 摩擦，这里按 axum 惯例整crate放行。
#![allow(clippy::result_large_err)]

rust_i18n::i18n!("locales", fallback = "zh-CN");

pub mod assemble;
pub mod async_export;
pub mod audit;
pub mod auth;
pub mod cache;
pub mod codegen;
pub mod data;
pub mod export;
pub mod extractor;
pub mod file;
pub mod health;
pub mod i18n;
pub mod idempotency;
pub mod identity;
pub mod import;
pub mod jobs;
pub mod layers;
pub mod logs;
pub mod middleware;
pub mod module;
pub mod notify;
pub mod oauth;
pub mod online;
pub mod openapi;
pub mod page;
pub mod problem;
pub mod ratelimit;
pub mod registry;
pub mod response;
pub mod router;
pub mod routes;
pub mod search;
pub mod server;
pub mod sort;
pub mod sort_select;
pub mod state;
pub mod system;

pub use assemble::{AssembleError, AssembleReport, Assembled, Assembler};
pub use extractor::RequestContextExt;
pub use health::{healthz, metrics, readyz};
pub use module::{Manifest, MenuNode, MenuType, Module, ModuleName, SeedKey, SeedOp, TableName};
pub use problem::{BizCode, FieldError, Problem};
pub use registry::{Extension, Merge, Slot, SlotKey};
pub use router::{router, router_with, router_with_menus};
pub use routes::{RouteAudit, RouteMeta, RoutePerms, Routes};
pub use state::AppState;

// 权限标识来自 kernel，模块注册契约直接引用。
pub use cicada_kernel::Permission;

/// 默认路径：一个模块 = 一个声明块（代码生成器产出即可用）。
///
/// 生成的 `Module` 类型需要放在独立的 `mod` 内，避免同名冲突。
#[macro_export]
macro_rules! declare_module {
    ($name:literal) => {
        $crate::declare_module!($name, owns: [], perms: [])
    };
    ($name:literal, routes: $routes:path) => {
        $crate::declare_module!($name, routes: $routes, owns: [], perms: [])
    };
    ($name:literal, owns: [$($table:literal),* $(,)?], perms: [$($perm:literal),* $(,)?]) => {
        pub struct Module;

        impl $crate::Module for Module {
            fn manifest(self: Box<Self>) -> $crate::Manifest {
                $crate::Manifest::new($name)
                    .owns([$($crate::TableName($table)),*])
                    .perms([$($crate::Permission::new($perm)),*])
            }
        }
    };
    ($name:literal, routes: $routes:path, owns: [$($table:literal),* $(,)?], perms: [$($perm:literal),* $(,)?]) => {
        pub struct Module;

        impl $crate::Module for Module {
            fn manifest(self: Box<Self>) -> $crate::Manifest {
                $crate::Manifest::new($name)
                    .routes($routes)
                    .owns([$($crate::TableName($table)),*])
                    .perms([$($crate::Permission::new($perm)),*])
            }
        }
    };
}
