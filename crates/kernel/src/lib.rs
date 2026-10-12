//! cicada-kernel（L0 内核）
//!
//! 错误类型、值对象、`Scope`、ID、分页、权限标识，以及后续的
//! `Clock` / `IdGenerator` / `CurrentUser` 等 trait 契约。
//! 按依赖规则规则：不依赖任何其它 cicada crate。

pub mod context;
pub mod error;
pub mod id;
pub mod job;
pub mod mask;
pub mod page;
pub mod permission;
pub mod principal;
pub mod role;
pub mod scope;

pub use context::{RequestContext, RequestId, TraceId};
pub use error::AppError;
pub use id::{DeptId, JobId, MenuId, PostId, RoleId, TenantId, UserId};
pub use job::{ConcurrencyPolicy, TriggerKind};
pub use page::{Page, PageParams};
pub use permission::Permission;
pub use principal::{Principal, SessionId};
pub use role::RoleGrant;
pub use scope::{
    ByPrimaryKey, CrossTenant, DataScope, DataScopeKind, DataScoped, DeptAndOwner, DeptOnly,
    Exempt, InternalTask, NoOwnership, OwnerOnly, Query, QueryState, Scope, Shape, ShapeOutcome,
    UnscopedReason,
};
