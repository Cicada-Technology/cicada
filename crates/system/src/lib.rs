//! cicada-system（L2 领域）
//!
//! 用户/身份绑定/角色/菜单/部门/岗位/字典/参数/公告/日志。

pub mod api_key;
pub mod cache;
pub mod dept_directory;
pub mod entity;
pub mod grant;
pub mod log_retention;
pub mod log_store;
pub mod menu;
pub mod menu_registry;
pub mod quota;
pub mod rich_text;

pub use api_key::{
    ApiKeyError, ApiKeyService, IssuedKey, KEY_PREFIX_LEN, STATUS_ACTIVE as API_KEY_ACTIVE,
    STATUS_DISABLED as API_KEY_DISABLED, generate_secret,
};
pub use cache::{
    CacheDomain, DEFAULT_TTL_SECS, cached_json, entry_key, invalidate, roles_ident, version_key,
};
pub use dept_directory::LocalDeptDirectory;
pub use entity::*;
pub use grant::{
    GrantSnapshot, load_grant_snapshot, load_permissions, load_role_grants, load_user_dept,
};
pub use log_retention::{ExpiredPartition, LogRetention, PARTITIONED_TABLES, RetentionReport};
pub use log_store::SysLogStore;
pub use menu::{MenuTreeNode, load_menu_tree};
pub use menu_registry::DbMenuRegistry;
pub use quota::{QuotaError, TenantQuota, TenantUsage};
pub use rich_text::sanitize_html;

/// `sys_export_task.status`：Pending / Running / Done / Failed。
pub const EXPORT_STATUS_PENDING: i16 = 0;
pub const EXPORT_STATUS_RUNNING: i16 = 1;
pub const EXPORT_STATUS_DONE: i16 = 2;
pub const EXPORT_STATUS_FAILED: i16 = 3;
