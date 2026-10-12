//! 数据权限：声明式、可序列化的数据范围对象。
//!
//! 最终设计见 `docs/superpowers/design-it-twice/datascope-typestate.md` 与 `FINAL.md` ：
//! - [`DataScope`] / [`Scope`]（`value`）：纯值对象，可进 JWT claim、可随跨服务调用传递；
//! - [`DataScoped`] + [`Shape`]（`entity`）：实体声明归属形态（四种形态 + 显式豁免）；
//! - [`Query`]（`query`）：`Query<E, S>` 类型状态机，把「漏做过滤」升级为编译错误。

mod entity;
mod query;
mod value;

pub use entity::{
    DataScoped, DeptAndOwner, DeptOnly, Exempt, NoOwnership, OwnerOnly, Shape, ShapeOutcome,
};
pub use query::{ByPrimaryKey, CrossTenant, InternalTask, Query, QueryState, UnscopedReason};
pub use value::{DataScope, DataScopeKind, Scope};
