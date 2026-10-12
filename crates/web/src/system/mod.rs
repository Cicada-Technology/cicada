//! 系统管理模块（system 域）：
//! M1 = 用户 / 角色 / 菜单 CRUD；M2 = 部门 / 岗位 / 字典 / 参数 / 公告 CRUD。
//!
//! 统一的写路径：`#[require_perm]` 声明权限点 → 运行时守卫 403；
//! `#[audit]` 记录 `sys_oper_log`；角色 / 角色-菜单 / 用户-角色变更后调用
//! 同一份变更还要让权限集合与菜单树两份缓存一起失效，
//! 走 `invalidate_grant_caches`，别只记得其一。

pub mod api_key;
pub mod config;
pub mod dept;
pub mod dict;
pub mod menu;
pub mod notice;
mod ownership;
pub mod post;
pub mod role;
pub mod tenant;
pub mod user;

use axum::http::StatusCode;

use crate::Permission;
use crate::module::{Manifest, Module};
use crate::routes::Routes;
use crate::state::AppState;

pub(crate) use crate::problem::{ApiErr, BizCode, FieldError, Problem};

pub(crate) fn err(status: StatusCode, msg: impl Into<String>) -> ApiErr {
    ApiErr::from_status(status, msg)
}

pub(crate) fn not_found() -> ApiErr {
    err(StatusCode::NOT_FOUND, "资源不存在")
}

pub(crate) fn unauthorized() -> ApiErr {
    err(StatusCode::UNAUTHORIZED, "未认证")
}

pub(crate) fn forbidden(msg: impl Into<String>) -> ApiErr {
    err(StatusCode::FORBIDDEN, msg)
}

pub(crate) fn conflict(msg: impl Into<String>) -> ApiErr {
    err(StatusCode::CONFLICT, msg)
}

/// 权限相关的两份缓存一起失效：权限集合（权限模型的角色版本号）与菜单树（缓存设计）。
///
/// 菜单树按「角色集合」缓存，所以角色变更与菜单变更都可能改变它。这两类都是低频
/// 管理操作，宁可粗一点整域失效，也不要漏掉一处变成「改了菜单，前端刷新还看不到」。
pub(crate) async fn invalidate_grant_caches(state: &AppState, tenant: cicada_kernel::TenantId) {
    state.authz.invalidate_tenant(tenant).await;
    cicada_system::invalidate(&state.sys_cache, cicada_system::CacheDomain::Menu, None).await;
}

/// 乐观锁冲突（业务码目录 `CICADA_SYS_3003`，建模规范「并发编辑报明确冲突，而非静默覆盖」）。
///
/// 与 `conflict`（唯一约束 3002）同为 409，但客户端的补救动作不同：唯一冲突是
/// 「换个值再来」，乐观锁是「重新读一遍再改」，所以必须是独立业务码。
pub(crate) fn version_conflict() -> ApiErr {
    Problem::new(BizCode::SysOptimisticLock, "数据已被他人修改，请刷新后重试")
}

/// 存在关联数据、拒绝删除（业务码目录 `CICADA_SYS_3004`）：与「唯一约束冲突」（3002）
/// 同为 409，但客户端要给出的补救动作完全不同。
pub(crate) fn referenced(msg: impl Into<String>) -> ApiErr {
    Problem::new(BizCode::SysReferenced, msg)
}

/// 引用了别的租户的行（业务码目录 `CICADA_RBAC_2002`，越权防护的归属校验）。
///
/// 用 403 而不是 404：调用方自己把 id 提交上来，答案不是「这行不存在」而是
/// 「这行不是你的」；也不是 2001「缺权限标识」——那种情况的补救是去要权限，
/// 而这里的补救是改掉请求体里的 id。
pub(crate) fn tenant_mismatch(msg: impl Into<String>) -> ApiErr {
    Problem::new(BizCode::RbacTenantMismatch, msg)
}

pub(crate) fn bad_request(msg: impl Into<String>) -> ApiErr {
    err(StatusCode::UNPROCESSABLE_ENTITY, msg)
}

/// 字段级校验失败（业务码目录 `CICADA_VALIDATION_4001`）：`detail` 给人读，
/// `errors[]` 带字段名，前端表单据此把消息贴回对应输入框。
pub(crate) fn field_error(field: impl Into<String>, msg: impl Into<String>) -> ApiErr {
    let msg = msg.into();
    Problem::new(BizCode::ValidationFields, msg.clone()).fields(vec![FieldError::new(field, msg)])
}

/// 树结构非法（父节点指向自身或后代，业务码目录 `CICADA_SYS_3005`）。
pub(crate) fn tree_error(msg: impl Into<String>) -> ApiErr {
    Problem::new(BizCode::SysTreeIllegal, msg)
}

/// 限额闸门：用 409 与「403 无权限」区分开，客户端能识别是套餐上限而不是权限问题。
pub(crate) fn quota_error(e: cicada_system::QuotaError) -> ApiErr {
    tracing::info!(error = %e, "租户限额闸门拒绝写入");
    err(StatusCode::CONFLICT, e.to_string())
}

pub(crate) fn db_error(e: impl std::fmt::Display) -> ApiErr {
    tracing::error!(error = %e, "系统管理模块数据库错误");
    err(
        StatusCode::INTERNAL_SERVER_ERROR,
        "系统数据操作失败，请稍后重试",
    )
}

/// 系统管理模块：用户 / 角色 / 菜单 / 部门 / 岗位 / 字典 / 参数 / 公告的权限点与路由
/// 一并声明，装配期双向对账。
pub struct SystemModule;

impl Module for SystemModule {
    fn manifest(self: Box<Self>) -> Manifest {
        let perms: Vec<Permission> = role::PERMS
            .iter()
            .chain(menu::PERMS.iter())
            .chain(user::PERMS.iter())
            .chain(dept::PERMS.iter())
            .chain(post::PERMS.iter())
            .chain(dict::PERMS.iter())
            .chain(config::PERMS.iter())
            .chain(notice::PERMS.iter())
            .chain(api_key::PERMS.iter())
            .chain(tenant::PERMS.iter())
            .map(|p| Permission::new(*p))
            .collect();
        Manifest::new("cicada:system").perms(perms).routes(routes)
    }
}

pub fn routes(state: AppState) -> Routes {
    role::routes(state.clone())
        .merge(menu::routes(state.clone()))
        .merge(user::routes(state.clone()))
        .merge(dept::routes(state.clone()))
        .merge(post::routes(state.clone()))
        .merge(dict::routes(state.clone()))
        .merge(config::routes(state.clone()))
        .merge(notice::routes(state.clone()))
        .merge(api_key::routes(state.clone()))
        .merge(tenant::routes(state))
}
