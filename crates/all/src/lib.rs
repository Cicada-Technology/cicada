//! cicada（单一入口门面 crate）
//!
//! re-export + feature 转发，供「不想研究 crate 划分」的使用者一把接入
//! 。`cicada new` 生成的使用者工程只需要：
//!
//! ```toml
//! cicada = { git = "https://github.com/<org>/cicada", tag = "v0.1.0", features = ["full"] }
//! ```
//!
//! 三条使用路径：
//!
//! - **门面接入**：[`bootstrap`] 提供装配骨架，业务模块经
//!   [`AppBuilder::with_module`](bootstrap::AppBuilder::with_module) 挂上；
//! - **分层接入**：按需 `cicada::web` / `cicada::kernel` / … 直接依赖某一层；
//! - **定制接入**：实现 `ports` 里的 trait（存储 / 通知 / 目录 / 任务），替换默认 adapter。
//!
//! 关于 feature：领域模块的裁剪靠**不注册模块**（装配器里少一个 `with_module`），
//! 而不是靠 feature——`AppState` 的形状决定了这些 crate 必须参与编译。因此本 crate
//! 的 feature 只覆盖真正可选的重型部件：`worker`（任务运行时）、`migrations`（框架迁移）、
//! `codegen`（代码生成器）。公布并测试的组合是 `full`（默认，三者全开）与
//! `["migrations"]`（不跑后台任务的只读服务）。

#![forbid(unsafe_code)]

pub mod bootstrap;

// ---- 门面：分层 crate 的稳定别名（`cicada::web::...` 与 `cicada_web::...` 等价）----
pub use cicada_audit as audit;
pub use cicada_auth as auth;
pub use cicada_infra as infra;
pub use cicada_kernel as kernel;
pub use cicada_monitor as monitor;
pub use cicada_notify as notify;
pub use cicada_ports as ports;
pub use cicada_rbac as rbac;
pub use cicada_storage as storage;
pub use cicada_system as system;
pub use cicada_web as web;

#[cfg(feature = "codegen")]
pub use cicada_codegen as codegen;
#[cfg(feature = "worker")]
pub use cicada_jobs as jobs;
#[cfg(feature = "migrations")]
pub use cicada_migrations as migrations;
#[cfg(feature = "worker")]
pub use cicada_worker as worker;

// ---- 高频类型直接提到根，省掉一层命名空间（使用者工程的常见路径）----
pub use bootstrap::{App, AppBuilder, BootstrapError};
pub use cicada_infra::{Cache, Config, Db, TracingGuard, init_tracing, init_tracing_with};
pub use cicada_kernel::{Permission, Scope, TenantId, UserId};
pub use cicada_ports::JobContext;
pub use cicada_web::state::AppState;
pub use cicada_web::{Manifest, Module, Routes, router, router_with};

// ---- 生态 crate 转发：使用者工程不必再各写一份版本，避免版本漂移 ----
pub use axum;
pub use chrono;
pub use sea_orm;
pub use sea_orm_migration;
pub use serde;
pub use serde_json;
pub use utoipa;
pub use uuid;

/// 常用的 `use` 集合：`use cicada::prelude::*;`
pub mod prelude {
    pub use crate::bootstrap::{AppBuilder, BootstrapError};
    pub use crate::kernel::error::AppError;
    pub use crate::kernel::{Permission, RequestContext, Scope, TenantId, UserId};
    pub use crate::web::{Manifest, Module, Routes};
    pub use async_trait::async_trait;
    pub use sea_orm::{
        ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, Set,
    };
    pub use sea_orm_migration::prelude::*;
    pub use serde::{Deserialize, Serialize};
}
