//! 系统域实体（P0 核心表）。
//!
//! 命名约定：每个子模块内的 `Entity` / `Model` / `Column` / `ActiveModel` /
//! `Relation` 在此统一重命名为 `XxxEntity` / `XxxModel` …，避免同名冲突。

pub mod api_key;
pub mod audit_log;
pub mod config;
pub mod dept;
pub mod dict_data;
pub mod dict_type;
pub mod export;
pub mod file;
pub mod login_info;
pub mod menu;
pub mod notice;
pub mod notice_read;
pub mod oper_log;
pub mod post;
pub mod role;
pub mod role_dept;
pub mod role_menu;
pub mod tenant;
pub mod tenant_package;
pub mod user;
pub mod user_identity;
pub mod user_role;

pub use api_key::{
    ActiveModel as ApiKeyActiveModel, Column as ApiKeyColumn, Entity as ApiKeyEntity,
    Model as ApiKeyModel, Relation as ApiKeyRelation,
};
pub use audit_log::{
    ActiveModel as AuditLogActiveModel, Column as AuditLogColumn, Entity as AuditLogEntity,
    Model as AuditLogModel, Relation as AuditLogRelation,
};
pub use config::{
    ActiveModel as ConfigActiveModel, Column as ConfigColumn, Entity as ConfigEntity,
    Model as ConfigModel, Relation as ConfigRelation,
};
pub use dept::{
    ActiveModel as DeptActiveModel, Column as DeptColumn, Entity as DeptEntity, Model as DeptModel,
    Relation as DeptRelation,
};
pub use dict_data::{
    ActiveModel as DictDataActiveModel, Column as DictDataColumn, Entity as DictDataEntity,
    Model as DictDataModel, Relation as DictDataRelation,
};
pub use dict_type::{
    ActiveModel as DictTypeActiveModel, Column as DictTypeColumn, Entity as DictTypeEntity,
    Model as DictTypeModel, Relation as DictTypeRelation,
};
pub use export::{
    ActiveModel as ExportActiveModel, Column as ExportColumn, Entity as ExportEntity,
    Model as ExportModel, Relation as ExportRelation,
};
pub use file::{
    ActiveModel as FileActiveModel, Column as FileColumn, Entity as FileEntity, Model as FileModel,
    Relation as FileRelation,
};
pub use login_info::{
    ActiveModel as LoginInfoActiveModel, Column as LoginInfoColumn, Entity as LoginInfoEntity,
    Model as LoginInfoModel, Relation as LoginInfoRelation,
};
pub use menu::{
    ActiveModel as MenuActiveModel, Column as MenuColumn, Entity as MenuEntity, Model as MenuModel,
    Relation as MenuRelation,
};
pub use notice::{
    ActiveModel as NoticeActiveModel, Column as NoticeColumn, Entity as NoticeEntity,
    Model as NoticeModel, Relation as NoticeRelation,
};
pub use notice_read::{
    ActiveModel as NoticeReadActiveModel, Column as NoticeReadColumn, Entity as NoticeReadEntity,
    Model as NoticeReadModel, Relation as NoticeReadRelation,
};
pub use oper_log::{
    ActiveModel as OperLogActiveModel, Column as OperLogColumn, Entity as OperLogEntity,
    Model as OperLogModel, Relation as OperLogRelation,
};
pub use post::{
    ActiveModel as PostActiveModel, Column as PostColumn, Entity as PostEntity, Model as PostModel,
    Relation as PostRelation,
};
pub use role::{
    ActiveModel as RoleActiveModel, Column as RoleColumn, Entity as RoleEntity, Model as RoleModel,
    Relation as RoleRelation,
};
pub use role_dept::{
    ActiveModel as RoleDeptActiveModel, Column as RoleDeptColumn, Entity as RoleDeptEntity,
    Model as RoleDeptModel, Relation as RoleDeptRelation,
};
pub use role_menu::{
    ActiveModel as RoleMenuActiveModel, Column as RoleMenuColumn, Entity as RoleMenuEntity,
    Model as RoleMenuModel, Relation as RoleMenuRelation,
};
pub use tenant::{
    ActiveModel as TenantActiveModel, Column as TenantColumn, Entity as TenantEntity,
    Model as TenantModel, Relation as TenantRelation,
};
pub use tenant_package::{
    ActiveModel as TenantPackageActiveModel, Column as TenantPackageColumn,
    Entity as TenantPackageEntity, Model as TenantPackageModel, Relation as TenantPackageRelation,
};
pub use user::{
    ActiveModel as UserActiveModel, Column as UserColumn, Entity as UserEntity, Model as UserModel,
    Relation as UserRelation,
};
pub use user_identity::{
    ActiveModel as UserIdentityActiveModel, Column as UserIdentityColumn,
    Entity as UserIdentityEntity, Model as UserIdentityModel, Relation as UserIdentityRelation,
};
pub use user_role::{
    ActiveModel as UserRoleActiveModel, Column as UserRoleColumn, Entity as UserRoleEntity,
    Model as UserRoleModel, Relation as UserRoleRelation,
};
