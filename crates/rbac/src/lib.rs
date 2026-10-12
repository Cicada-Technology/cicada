//! cicada-rbac（L3 能力）
//!
//! 授权：权限点解析、角色合并、数据范围计算、缓存。

pub mod authorize;
pub mod cache;
pub mod data_scope;
pub mod permission;

pub use authorize::{Authorization, authorize};
pub use cache::{AuthzCache, AuthzStore, InMemoryAuthzStore, RedisAuthzStore};
pub use data_scope::{SUPER_ADMIN_ROLE_KEY, compute_data_scope, is_super_admin};
pub use permission::{PermissionSet, resolve_permissions};

// RoleGrant 是 kernel 的纯值契约，这里透传方便调用方一处导入。
pub use cicada_kernel::RoleGrant;
