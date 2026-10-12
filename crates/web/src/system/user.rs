//! 用户管理（权限点 `system:user:*`）。
//!
//! 列表走数据范围状态机 `Query::<UserEntity>::open().scoped(&scope)`：部门/归属过滤
//! 由类型系统保证，漏做即编译失败。写操作同步维护 `sys_user_identity` 主身份镜像，
//! 用户-角色变更后按版本号整体失效权限缓存。

use axum::Json;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{delete, get, post, put};
use sea_orm::{ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use utoipa::{IntoParams, ToSchema};

use cicada_kernel::mask::{MaskPolicy, Maskable};
use cicada_kernel::scope::Query as ScopedQuery;
use cicada_macros::{Maskable, audit, require_perm};
use cicada_system::entity::{
    UserActiveModel, UserColumn, UserEntity, UserModel, UserRoleActiveModel, UserRoleColumn,
    UserRoleEntity,
};

use super::{
    ApiErr, bad_request, conflict, db_error, field_error, forbidden, not_found, ownership,
    quota_error, unauthorized, version_conflict,
};
use crate::export;
use crate::extractor::RequestContextExt;
use crate::import::{self, ImportReport, RowError};
use crate::response::created;
use crate::routes::Routes;
use crate::search::Fuzzy;
use crate::sort::SortExt;
use crate::state::AppState;

pub const PERMS: &[&str] = &[
    "system:user:list",
    "system:user:add",
    "system:user:edit",
    "system:user:remove",
    "system:user:resetPwd",
    "system:user:export",
    "system:user:import",
];

/// `sys_user` 列表允许排序的列（排序白名单）。
///
/// 只放「列表页真的会点排序」的列：每多一个可排序列，就多一条能被利用的
/// `ORDER BY` 路径（大表上非索引列排序会把排序成本推给每一次翻页）。
const USER_SORT: crate::sort::SortSpec = crate::sort::SortSpec::new(&[
    ("user_name", "sys_user", "user_name"),
    ("nick_name", "sys_user", "nick_name"),
    ("status", "sys_user", "status"),
    ("create_time", "sys_user", "create_time"),
    ("login_date", "sys_user", "login_date"),
]);

/// 内置超管账号（`init-admin` 写入的 `user_id = 1`），禁止删除。
const BUILTIN_ADMIN_ID: i64 = 1;

#[derive(Debug, Deserialize, IntoParams)]
pub struct UserListQuery {
    pub user_name: Option<String>,
    pub status: Option<i16>,
    pub dept_id: Option<i64>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    /// 排序列名（排序白名单，见 `crate::sort`）；不在清单里则忽略。
    pub sort_by: Option<String>,
    /// `asc` 升序，其余按 `desc`。
    pub sort_order: Option<String>,
}

#[derive(Debug, Serialize, ToSchema, Maskable)]
pub struct UserVo {
    pub user_id: i64,
    pub user_name: String,
    pub nick_name: String,
    pub dept_id: Option<i64>,
    #[mask(Email)]
    pub email: Option<String>,
    #[mask(Mobile)]
    pub phonenumber: Option<String>,
    pub status: i16,
    pub create_time: String,
    pub role_ids: Vec<i64>,
    /// 乐观锁版本（建模规范）。读接口回传它，改接口原样带回才会做冲突检测。
    pub version: i32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UserPage {
    pub items: Vec<UserVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UserCreate {
    pub user_name: String,
    pub nick_name: String,
    pub password: String,
    pub dept_id: Option<i64>,
    pub email: Option<String>,
    pub phonenumber: Option<String>,
    pub status: Option<i16>,
    pub remark: Option<String>,
    pub role_ids: Option<Vec<i64>>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UserUpdate {
    pub nick_name: String,
    pub dept_id: Option<i64>,
    pub email: Option<String>,
    pub phonenumber: Option<String>,
    pub status: Option<i16>,
    pub remark: Option<String>,
    pub role_ids: Option<Vec<i64>>,
    /// 乐观锁期望版本（建模规范）。带上读到的 `version` 才会做冲突检测；
    /// 不带则退回「后写覆盖」——存量客户端不带这个字段，
    /// 强制要求会让它们整批改不动，所以缺省只能是「不检查」而不是「拒绝」。
    pub version: Option<i32>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ResetPassword {
    pub password: String,
}

fn to_vo(m: UserModel, role_ids: Vec<i64>) -> UserVo {
    UserVo {
        user_id: m.user_id,
        user_name: m.user_name,
        nick_name: m.nick_name,
        dept_id: m.dept_id,
        email: m.email,
        phonenumber: m.phonenumber,
        status: m.status,
        create_time: m.create_time.to_rfc3339(),
        role_ids,
        version: m.version,
    }
}

/// 脱敏策略：超管或持有 `system:user:edit` 的编辑角色看明文，
/// 其余（只读列表角色）手机号/邮箱掩码。编辑者必须看明文，否则表单回填会写脏数据。
fn mask_policy(
    is_super_admin: bool,
    permissions: &[cicada_kernel::permission::Permission],
) -> MaskPolicy {
    if is_super_admin || permissions.iter().any(|p| p.as_str() == "system:user:edit") {
        MaskPolicy::Plain
    } else {
        MaskPolicy::Masked
    }
}

async fn roles_of<C: ConnectionTrait>(
    db: &C,
    user_ids: &[i64],
) -> Result<std::collections::HashMap<i64, Vec<i64>>, ApiErr> {
    use std::collections::HashMap;
    let mut map: HashMap<i64, Vec<i64>> = HashMap::new();
    if user_ids.is_empty() {
        return Ok(map);
    }
    let rows = UserRoleEntity::find()
        .filter(UserRoleColumn::UserId.is_in(user_ids.to_vec()))
        .all(db)
        .await
        .map_err(db_error)?;
    for r in rows {
        map.entry(r.user_id).or_default().push(r.role_id);
    }
    Ok(map)
}

async fn replace_user_roles<C: ConnectionTrait>(
    db: &C,
    tenant: i64,
    user_id: i64,
    role_ids: &[i64],
) -> Result<(), ApiErr> {
    // 越权防护归属校验：角色 id 是调用方给的，必须先确认它们都在这个租户里，
    // 否则 `sys_user_role` 里会留下一条指向别的租户角色的授权引用。
    ownership::assert_roles(db, tenant, role_ids).await?;
    UserRoleEntity::delete_many()
        .filter(UserRoleColumn::UserId.eq(user_id))
        .exec(db)
        .await
        .map_err(db_error)?;
    for role_id in role_ids {
        UserRoleActiveModel {
            user_id: Set(user_id),
            role_id: Set(*role_id),
        }
        .insert(db)
        .await
        .map_err(db_error)?;
    }
    Ok(())
}

/// 维护 `sys_user.email/phonenumber` 与对应主身份的一致性（镜像列）。
async fn sync_identity_mirror<C: ConnectionTrait>(
    db: &C,
    tenant_id: i64,
    user_id: i64,
    identity_type: &str,
    column: &str,
    value: Option<&str>,
) -> Result<(), ApiErr> {
    use sea_orm::{DatabaseBackend, Statement};
    let normalized = value.map(|v| {
        if identity_type == "Email" {
            v.trim().to_lowercase()
        } else {
            v.trim().to_string()
        }
    });
    let normalized = normalized.filter(|v| !v.is_empty());

    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!(
            "UPDATE sys_user SET {column} = $1, update_time = now() WHERE tenant_id = $2 AND user_id = $3"
        ),
        [
            sea_orm::Value::String(normalized.clone()),
            tenant_id.into(),
            user_id.into(),
        ],
    );
    db.execute_raw(stmt).await.map_err(db_error)?;

    if let Some(identifier) = normalized {
        let upd = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE sys_user_identity SET identifier = $1 \
             WHERE tenant_id = $2 AND user_id = $3 AND identity_type = $4 AND is_primary = true",
            [
                identifier.into(),
                tenant_id.into(),
                user_id.into(),
                identity_type.into(),
            ],
        );
        db.execute_raw(upd).await.map_err(db_error)?;
    }
    Ok(())
}

#[utoipa::path(
    get,
    path = "/api/v1/system/users",
    params(UserListQuery),
    responses((status = 200, description = "用户分页（受数据范围约束）", body = UserPage))
)]
#[require_perm("system:user:list")]
pub async fn list_users(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<UserListQuery>,
) -> Result<Json<UserPage>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let mut query = ScopedQuery::<UserEntity>::open().scoped(&ctx.scope());
    if let Some(name) = q.user_name.filter(|s| !s.is_empty()) {
        query = query.filter(UserColumn::UserName.fuzzy(&name));
    }
    if let Some(status) = q.status {
        query = query.filter(UserColumn::Status.eq(status));
    }
    if let Some(dept_id) = q.dept_id {
        query = query.filter(UserColumn::DeptId.eq(dept_id));
    }

    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;
    let paged = query
        // 排序白名单：只认这张表声明过的列，客户端字符串进不了标识符位置。
        // 显式排序必须排在前面才真的生效：默认次序退化成同值行的收尾键（user_id），
        // 既不改变「不传 sort_by 时的结果」，又保证翻页不重不漏。
        .sorted_by(&USER_SORT, q.sort_by.as_deref(), q.sort_order.as_deref())
        .order_by(UserColumn::UserId)
        .fetch_page(state.db.conn(), page, page_size)
        .await
        .map_err(db_error)?;

    let ids: Vec<i64> = paged.items.iter().map(|u| u.user_id).collect();
    let mut roles = roles_of(state.db.conn(), &ids).await?;
    let policy = mask_policy(ctx.is_super_admin, &ctx.permissions);
    let items = paged
        .items
        .into_iter()
        .map(|u| {
            let role_ids = roles.remove(&u.user_id).unwrap_or_default();
            to_vo(u, role_ids).masked(policy)
        })
        .collect();
    Ok(Json(UserPage {
        items,
        total: paged.total,
        page: paged.page,
        page_size: paged.page_size,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/system/users/{user_id}",
    params(("user_id" = i64, Path, description = "用户 id")),
    responses((status = 200, description = "用户详情", body = UserVo), (status = 404, description = "不存在或不在数据范围内"))
)]
#[require_perm("system:user:list")]
pub async fn get_user(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(user_id): Path<i64>,
) -> Result<Json<UserVo>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let user = ScopedQuery::<UserEntity>::open()
        .scoped(&ctx.scope())
        .filter(UserColumn::UserId.eq(user_id))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    let role_ids = roles_of(state.db.conn(), &[user_id])
        .await?
        .remove(&user_id)
        .unwrap_or_default();
    Ok(Json(
        to_vo(user, role_ids).masked(mask_policy(ctx.is_super_admin, &ctx.permissions)),
    ))
}

/// 口令规则：建号、管理员重置、导入初始密码都走这一份判定。
///
/// 违规项按统一响应的 `errors[]` 返回（422），前端表单能把消息贴回密码框；
/// 阈值来自配置 `auth.password.min_length`，写死在代码里会让那条配置永远不生效。
fn password_rule_error(state: &AppState, pw: &str) -> Option<ApiErr> {
    let min_length = state.config.auth.password.min_length;
    let problems = cicada_infra::password_problems(pw, min_length);
    if problems.is_empty() {
        return None;
    }
    Some(field_error(
        "password",
        cicada_infra::password_problem_text(&problems, min_length),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/system/users",
    request_body = UserCreate,
    responses((status = 201, description = "已创建", body = UserVo), (status = 409, description = "账号名重复"))
)]
#[require_perm("system:user:add")]
#[audit(module = "system", action = "user:add", kind = "insert")]
pub async fn create_user(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<UserCreate>,
) -> Result<(StatusCode, HeaderMap, Json<UserVo>), ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    let user_name = req.user_name.trim().to_string();
    if user_name.is_empty() || req.nick_name.trim().is_empty() || req.password.is_empty() {
        return Err(bad_request("账号名、昵称与初始密码不能为空"));
    }
    if let Some(e) = password_rule_error(&state, &req.password) {
        return Err(e);
    }
    // 租户席位闸门。
    state
        .quota
        .check_user_slot(ctx.tenant_id, 1)
        .await
        .map_err(quota_error)?;
    let pw_hash = cicada_auth::hash_password(&req.password)
        .map_err(|e| db_error(format!("密码哈希失败：{e}")))?;

    let txn = state.db.begin().await.map_err(db_error)?;
    let exists = UserEntity::find()
        .filter(UserColumn::TenantId.eq(ctx.tenant_id.get()))
        .filter(UserColumn::UserName.eq(&user_name))
        .filter(UserColumn::DeletedAt.is_null())
        .one(&txn)
        .await
        .map_err(db_error)?
        .is_some();
    if exists {
        return Err(conflict("账号名已存在"));
    }

    // 越权防护归属校验：部门同样是调用方给的 id，挂到别的租户上就等于把新人放进别人的数据范围。
    ownership::assert_dept(&txn, ctx.tenant_id.get(), req.dept_id).await?;

    let now = chrono::Utc::now();
    let user = insert_user(
        &txn,
        ctx.tenant_id.get(),
        &user_name,
        req.nick_name.trim(),
        &pw_hash,
        req.dept_id,
        req.email.as_deref(),
        req.phonenumber.as_deref(),
        req.status.unwrap_or(0),
        req.remark.as_deref(),
        Some(principal.user_name.as_str()),
        now,
    )
    .await?;
    if let Some(role_ids) = &req.role_ids {
        replace_user_roles(&txn, ctx.tenant_id.get(), user.user_id, role_ids).await?;
    }
    txn.commit().await.map_err(db_error)?;

    state.authz.invalidate_tenant(ctx.tenant_id).await;
    let role_ids = req.role_ids.clone().unwrap_or_default();
    Ok(created(
        format!("/api/v1/system/users/{}", user.user_id),
        to_vo(user, role_ids),
    ))
}

async fn insert_identity<C: ConnectionTrait>(
    db: &C,
    tenant_id: i64,
    user_id: i64,
    identity_type: &str,
    identifier: &str,
    is_primary: bool,
    verified_at: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<(), ApiErr> {
    use sea_orm::{DatabaseBackend, Statement};
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO sys_user_identity \
         (tenant_id, user_id, identity_type, identifier, provider, verified_at, is_primary, create_time) \
         VALUES ($1, $2, $3, $4, NULL, $5, $6, now())",
        [
            tenant_id.into(),
            user_id.into(),
            identity_type.into(),
            identifier.into(),
            verified_at.into(),
            is_primary.into(),
        ],
    );
    db.execute_raw(stmt).await.map_err(db_error)?;
    Ok(())
}

/// 在事务内写一条用户（账号）+ 主身份镜像（Username / Email / Phone），供建号与导入共用。
#[allow(clippy::too_many_arguments)]
async fn insert_user<C: ConnectionTrait>(
    db: &C,
    tenant_id: i64,
    user_name: &str,
    nick_name: &str,
    password_hash: &str,
    dept_id: Option<i64>,
    email: Option<&str>,
    phonenumber: Option<&str>,
    status: i16,
    remark: Option<&str>,
    create_by: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<UserModel, ApiErr> {
    let email = email
        .map(|e| e.trim().to_lowercase())
        .filter(|e| !e.is_empty());
    let phone = phonenumber
        .map(|p| {
            p.chars()
                .filter(|c| !matches!(c, ' ' | '-'))
                .collect::<String>()
        })
        .filter(|p| !p.is_empty());

    let user = UserActiveModel {
        tenant_id: Set(tenant_id),
        user_name: Set(user_name.to_string()),
        nick_name: Set(nick_name.to_string()),
        dept_id: Set(dept_id),
        password: Set(password_hash.to_string()),
        email: Set(email.clone()),
        phonenumber: Set(phone.clone()),
        status: Set(status),
        version: Set(0),
        remark: Set(remark.map(|r| r.to_string())),
        create_by: Set(create_by.map(|c| c.to_string())),
        create_time: Set(now),
        ..Default::default()
    }
    .insert(db)
    .await
    .map_err(db_error)?;

    insert_identity(
        db,
        tenant_id,
        user.user_id,
        "Username",
        user_name,
        true,
        Some(now),
    )
    .await?;
    if let Some(email) = email.as_deref() {
        insert_identity(db, tenant_id, user.user_id, "Email", email, true, None).await?;
    }
    if let Some(phone) = phone.as_deref() {
        insert_identity(db, tenant_id, user.user_id, "Phone", phone, true, None).await?;
    }
    Ok(user)
}

#[utoipa::path(
    put,
    path = "/api/v1/system/users/{user_id}",
    request_body = UserUpdate,
    responses((status = 200, description = "已更新", body = UserVo), (status = 404, description = "不存在或不在数据范围内"))
)]
#[require_perm("system:user:edit")]
#[audit(module = "system", action = "user:edit", kind = "update")]
pub async fn update_user(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(user_id): Path<i64>,
    Json(req): Json<UserUpdate>,
) -> Result<Json<UserVo>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };

    let txn = state.db.begin().await.map_err(db_error)?;
    let user = ScopedQuery::<UserEntity>::open()
        .scoped(&ctx.scope())
        .filter(UserColumn::UserId.eq(user_id))
        .one(&txn)
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;

    // 留痕快照（审计设计）：只记本接口能改的列，口令一类根本不在其中。
    let before = serde_json::json!({
        "nick_name": user.nick_name,
        "dept_id": user.dept_id,
        "status": user.status,
        "remark": user.remark,
    });

    ownership::assert_dept(&txn, ctx.tenant_id.get(), req.dept_id).await?;

    // 建模规范乐观锁：`UPDATE … WHERE user_id = ? AND version = 期望值`，同时把版本 +1。
    // 0 行受影响只有两种解释——别人先改了（版本不匹配）或行没了（软删/删除），
    // 两者对客户端都是同一句话：重新读一遍再来。SeaORM 2.0.4 没有原生 opt-lock，
    // 也没有 `#[sea_orm(version)]` 这个属性，所以这里手写 CAS；手写的那一处就是唯一出口。
    let expected = req.version.unwrap_or(user.version);
    let applied = UserEntity::update_many()
        .col_expr(
            UserColumn::NickName,
            sea_orm::sea_query::Expr::value(req.nick_name.trim().to_string()),
        )
        .col_expr(
            UserColumn::DeptId,
            sea_orm::sea_query::Expr::value(req.dept_id),
        )
        .col_expr(
            UserColumn::Status,
            sea_orm::sea_query::Expr::value(req.status.unwrap_or(user.status)),
        )
        .col_expr(
            UserColumn::Remark,
            sea_orm::sea_query::Expr::value(req.remark.clone()),
        )
        .col_expr(
            UserColumn::UpdateBy,
            sea_orm::sea_query::Expr::value(Some(principal.user_name.clone())),
        )
        .col_expr(
            UserColumn::UpdateTime,
            sea_orm::sea_query::Expr::value(Some(chrono::Utc::now())),
        )
        .col_expr(
            UserColumn::Version,
            sea_orm::sea_query::Expr::value(expected + 1),
        )
        .filter(UserColumn::UserId.eq(user_id))
        .filter(UserColumn::Version.eq(expected))
        .exec(&txn)
        .await
        .map_err(db_error)?;
    if applied.rows_affected == 0 {
        return Err(version_conflict());
    }

    sync_identity_mirror(
        &txn,
        ctx.tenant_id.get(),
        user_id,
        "Email",
        "email",
        req.email.as_deref(),
    )
    .await?;
    sync_identity_mirror(
        &txn,
        ctx.tenant_id.get(),
        user_id,
        "Phone",
        "phonenumber",
        req.phonenumber.as_deref(),
    )
    .await?;

    if let Some(role_ids) = &req.role_ids {
        replace_user_roles(&txn, ctx.tenant_id.get(), user_id, role_ids).await?;
    }
    txn.commit().await.map_err(db_error)?;

    state.authz.invalidate_tenant(ctx.tenant_id).await;

    let updated = UserEntity::find_by_id(user_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    crate::audit::record_change(
        &state.audit,
        &ctx,
        "sys_user",
        user_id.to_string(),
        before,
        serde_json::json!({
            "nick_name": updated.nick_name,
            "dept_id": updated.dept_id,
            "status": updated.status,
            "remark": updated.remark,
        }),
    )
    .await;
    let role_ids = req.role_ids.unwrap_or_default();
    Ok(Json(to_vo(updated, role_ids)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/system/users/{user_id}",
    params(("user_id" = i64, Path, description = "用户 id")),
    responses((status = 204, description = "已删除"), (status = 403, description = "内置账号不可删"))
)]
#[require_perm("system:user:remove")]
#[audit(module = "system", action = "user:remove", kind = "delete")]
pub async fn delete_user(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(user_id): Path<i64>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    if user_id == BUILTIN_ADMIN_ID {
        return Err(forbidden("内置超级管理员账号不可删除"));
    }
    let txn = state.db.begin().await.map_err(db_error)?;
    let user = ScopedQuery::<UserEntity>::open()
        .scoped(&ctx.scope())
        .filter(UserColumn::UserId.eq(user_id))
        .one(&txn)
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;

    let now = chrono::Utc::now();
    UserActiveModel {
        user_id: Set(user_id),
        tenant_id: Set(user.tenant_id),
        deleted_at: Set(Some(now)),
        update_time: Set(Some(now)),
        ..Default::default()
    }
    .update(&txn)
    .await
    .map_err(db_error)?;
    UserRoleEntity::delete_many()
        .filter(UserRoleColumn::UserId.eq(user_id))
        .exec(&txn)
        .await
        .map_err(db_error)?;
    txn.commit().await.map_err(db_error)?;

    // 改密/删除即失效：撤销该账号全部在线会话。
    let _ = state.sessions.revoke_user(user_id).await;
    state.authz.invalidate_tenant(ctx.tenant_id).await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    put,
    path = "/api/v1/system/users/{user_id}/password",
    request_body = ResetPassword,
    params(("user_id" = i64, Path, description = "用户 id")),
    responses((status = 204, description = "已重置并踢下线"))
)]
#[require_perm("system:user:resetPwd")]
#[audit(module = "system", action = "user:resetPwd", kind = "update")]
pub async fn reset_password(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(user_id): Path<i64>,
    Json(req): Json<ResetPassword>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    // 原来这里是硬编码的 `len < 6`：既比安全设计要求的 8 位松，也不查大小写与数字，
    // 而且完全绕开了配置。现在与建号同一份规则。
    if let Some(e) = password_rule_error(&state, &req.password) {
        return Err(e);
    }
    let existing = ScopedQuery::<UserEntity>::open()
        .scoped(&ctx.scope())
        .filter(UserColumn::UserId.eq(user_id))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;

    let pw_hash = cicada_auth::hash_password(&req.password)
        .map_err(|e| db_error(format!("密码哈希失败：{e}")))?;
    UserActiveModel {
        user_id: Set(user_id),
        tenant_id: Set(existing.tenant_id),
        password: Set(pw_hash),
        update_time: Set(Some(chrono::Utc::now())),
        ..Default::default()
    }
    .update(state.db.conn())
    .await
    .map_err(db_error)?;

    let _ = state.sessions.revoke_user(user_id).await;
    Ok(StatusCode::NO_CONTENT)
}

const EXPORT_HEADERS: &[&str] = &[
    "用户编号",
    "登录名称",
    "用户昵称",
    "部门编号",
    "邮箱",
    "手机号码",
    "状态",
    "创建时间",
];
const EXPORT_WIDTHS: &[f64] = &[10.0, 16.0, 16.0, 10.0, 26.0, 16.0, 8.0, 22.0];

/// 用户导出的过滤条件：同步导出取自查询串，异步导出取自 `sys_export_task.query` 快照。
#[derive(Clone, Debug, Default, Serialize, Deserialize, ToSchema)]
pub struct UserExportFilters {
    pub user_name: Option<String>,
    pub status: Option<i16>,
    /// 部门集合：异步快照可携带多部门，同步路径只有单值。
    pub dept_ids: Vec<i64>,
}

/// 用户一行 → 单元格文本（异步执行器也用它保证两种导出的行内容逐字一致）。
fn user_row_cells(u: &UserModel) -> Vec<String> {
    vec![
        u.user_id.to_string(),
        u.user_name.clone(),
        u.nick_name.clone(),
        u.dept_id.map(|d| d.to_string()).unwrap_or_default(),
        u.email.clone().unwrap_or_default(),
        u.phonenumber.clone().unwrap_or_default(),
        if u.status == 0 { "正常" } else { "停用" }.to_string(),
        u.create_time.to_rfc3339(),
    ]
}

/// 在数据范围 `scope` 内按批读取用户并写入 worksheet（`on_progress` 实时上报行数）。
pub async fn write_users_sheet(
    conn: &sea_orm::DatabaseConnection,
    scope: &cicada_kernel::Scope,
    filters: &UserExportFilters,
    sheet: &mut rust_xlsxwriter::Worksheet,
    on_progress: &mut (dyn FnMut(i64) + Send),
) -> Result<(), export::ExportError> {
    let name = filters.user_name.clone().filter(|s| !s.is_empty());
    let status = filters.status;
    let dept_ids = filters.dept_ids.clone();
    let mut row: u32 = 1;
    let mut offset: u64 = 0;
    while u64::from(row) - 1 < export::MAX_EXPORT_ROWS {
        let mut query = ScopedQuery::<UserEntity>::open().scoped(scope);
        if let Some(n) = &name {
            query = query.filter(UserColumn::UserName.fuzzy(n));
        }
        if let Some(s) = status {
            query = query.filter(UserColumn::Status.eq(s));
        }
        if !dept_ids.is_empty() {
            query = query.filter(UserColumn::DeptId.is_in(dept_ids.clone()));
        }
        let batch = query
            .order_by(UserColumn::UserId)
            .limit(export::EXPORT_BATCH)
            .offset(offset)
            .all(conn)
            .await?;
        if batch.is_empty() {
            break;
        }
        for u in &batch {
            for (col, cell) in user_row_cells(u).iter().enumerate() {
                sheet.write_string(row, col as u16, cell)?;
            }
            row += 1;
        }
        on_progress(row as i64 - 1);
        offset += batch.len() as u64;
    }
    Ok(())
}

/// 用户 xlsx 导出（同步响应与异步任务共用）：分批读库 + 常量内存写表。
/// 输入全部按值：`export::build` 的写行闭包要求未来 `'static`（跨 await 持有）。
pub async fn build_users_xlsx(
    conn: sea_orm::DatabaseConnection,
    scope: cicada_kernel::Scope,
    filters: UserExportFilters,
    on_progress: impl FnMut(i64) + Send + 'static,
) -> Result<Vec<u8>, export::ExportError> {
    let mut progress: export::Progress = Box::new(on_progress);
    export::build("用户", EXPORT_HEADERS, EXPORT_WIDTHS, move |sheet| {
        Box::pin(
            async move { write_users_sheet(&conn, &scope, &filters, sheet, &mut progress).await },
        )
    })
    .await
}

#[utoipa::path(
    get,
    path = "/api/v1/system/users/export",
    params(UserListQuery),
    responses((status = 200, description = "用户 xlsx 导出（受数据范围约束）"))
)]
#[require_perm("system:user:export")]
#[audit(module = "system", action = "user:export", kind = "export")]
pub async fn export_users(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<UserListQuery>,
) -> Result<Response, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let filters = UserExportFilters {
        user_name: q.user_name,
        status: q.status,
        dept_ids: q.dept_id.map(|d| vec![d]).unwrap_or_default(),
    };
    let bytes = build_users_xlsx(state.db.conn().clone(), ctx.scope(), filters, |_| {})
        .await
        .map_err(db_error)?;
    Ok(export::xlsx_response("users.xlsx", bytes))
}

// ============================ 导入 ============================

/// 导入模板表头：与 `IMPORT_HEADERS` 严格对应（导入时按列位置解析）。
pub const IMPORT_HEADERS: &[&str] = &[
    "登录名称",
    "用户昵称",
    "部门编号",
    "邮箱",
    "手机号码",
    "状态",
    "角色编号",
    "备注",
];
const IMPORT_WIDTHS: &[f64] = &[16.0, 16.0, 10.0, 26.0, 16.0, 8.0, 14.0, 24.0];
const IMPORT_SAMPLE: &[&str] = &[
    "zhangsan",
    "张三",
    "",
    "zhangsan@example.com",
    "13800000000",
    "正常",
    "",
    "示例行，导入前请删除",
];

/// 上传文件大小上限（字节）。
const MAX_IMPORT_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Deserialize, IntoParams)]
pub struct ImportQuery {
    /// `fail_fast`（默认，全量成功才落库）/ `partial`（逐行，成功行落库）。
    pub mode: Option<String>,
}

fn parse_status(raw: &str) -> Option<i16> {
    match raw.trim() {
        "" | "0" | "正常" | "启用" => Some(0),
        "1" | "停用" | "禁用" => Some(1),
        _ => None,
    }
}

fn parse_role_ids(raw: &str) -> Result<Vec<i64>, String> {
    let mut out = Vec::new();
    for part in raw.split([',', '，', ';', '；']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        out.push(
            part.parse::<i64>()
                .map_err(|_| format!("角色编号非法：{part}"))?,
        );
    }
    Ok(out)
}

/// 一行校验通过的用户数据。
struct NewUserRow {
    row: u32,
    user_name: String,
    nick_name: String,
    dept_id: Option<i64>,
    email: Option<String>,
    phonenumber: Option<String>,
    status: i16,
    role_ids: Vec<i64>,
    remark: Option<String>,
}

/// 逐行校验（不查库）；库级校验（唯一性、外键）在插入阶段补齐。
fn validate_row(row: &import::Row, dept_ids: &HashSet<i64>) -> Result<NewUserRow, Vec<RowError>> {
    let mut errors = Vec::new();
    let err = |field: &str, reason: String| RowError {
        row: row.number,
        field: field.to_string(),
        reason,
    };

    let user_name = row.clean(0).unwrap_or_default();
    if user_name.is_empty() {
        errors.push(err("登录名称", "不能为空".to_string()));
    } else if user_name.chars().count() > 30 {
        errors.push(err("登录名称", "长度不能超过 30".to_string()));
    }
    let nick_name = row.clean(1).unwrap_or_default();
    if nick_name.is_empty() {
        errors.push(err("用户昵称", "不能为空".to_string()));
    }

    let dept_id = match row.cell(2) {
        None => None,
        Some(raw) => match raw.parse::<i64>() {
            Ok(id) if dept_ids.contains(&id) => Some(id),
            Ok(id) => {
                errors.push(err("部门编号", format!("部门不存在：{id}")));
                None
            }
            Err(_) => {
                errors.push(err("部门编号", format!("非法数字：{raw}")));
                None
            }
        },
    };

    let email = row.clean(3).filter(|e| !e.is_empty());
    if let Some(e) = &email
        && !e.contains('@')
    {
        errors.push(err("邮箱", format!("格式非法：{e}")));
    }
    let phonenumber = row
        .clean(4)
        .map(|p| {
            p.chars()
                .filter(|c| !matches!(c, ' ' | '-'))
                .collect::<String>()
        })
        .filter(|p| !p.is_empty());
    if let Some(p) = &phonenumber
        && (p.len() < 5 || p.len() > 20 || !p.chars().all(|c| c.is_ascii_digit() || c == '+'))
    {
        errors.push(err("手机号码", format!("格式非法：{p}")));
    }

    let status = match parse_status(row.cell(5).unwrap_or("")) {
        Some(s) => s,
        None => {
            errors.push(err("状态", "只能是 正常/停用".to_string()));
            0
        }
    };

    let role_ids = match row.cell(6) {
        None => Vec::new(),
        Some(raw) => match parse_role_ids(raw) {
            Ok(ids) => ids,
            Err(reason) => {
                errors.push(err("角色编号", reason));
                Vec::new()
            }
        },
    };
    let remark = row.clean(7);

    if errors.is_empty() {
        Ok(NewUserRow {
            row: row.number,
            user_name,
            nick_name,
            dept_id,
            email,
            phonenumber,
            status,
            role_ids,
            remark,
        })
    } else {
        Err(errors)
    }
}

async fn id_set<C: ConnectionTrait>(
    db: &C,
    sql: &str,
    tenant_id: i64,
) -> Result<HashSet<i64>, ApiErr> {
    use sea_orm::{DatabaseBackend, Statement};
    let stmt = Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, [tenant_id.into()]);
    let rows = db.query_all_raw(stmt).await.map_err(db_error)?;
    Ok(rows
        .into_iter()
        .filter_map(|r| r.try_get_by_index::<i64>(0).ok())
        .collect())
}

async fn string_set<C: ConnectionTrait>(
    db: &C,
    sql: &str,
    tenant_id: i64,
) -> Result<HashSet<String>, ApiErr> {
    use sea_orm::{DatabaseBackend, Statement};
    let stmt = Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, [tenant_id.into()]);
    let rows = db.query_all_raw(stmt).await.map_err(db_error)?;
    Ok(rows
        .into_iter()
        .filter_map(|r| r.try_get_by_index::<String>(0).ok())
        .collect())
}

#[utoipa::path(
    get,
    path = "/api/v1/system/users/import-template",
    responses((status = 200, description = "用户导入模板 xlsx"))
)]
#[require_perm("system:user:import")]
pub async fn import_template(ctx: RequestContextExt) -> Result<Response, ApiErr> {
    if ctx.into_inner().principal.is_none() {
        return Err(unauthorized());
    }
    let bytes = export::build(
        "用户导入模板",
        IMPORT_HEADERS,
        IMPORT_WIDTHS,
        |sheet| {
            Box::pin(async move {
                for (col, value) in IMPORT_SAMPLE.iter().enumerate() {
                    sheet.write_string(1, col as u16, *value)?;
                }
                Ok(())
            })
        },
    )
    .await
    .map_err(db_error)?;
    Ok(export::xlsx_response("user_import_template.xlsx", bytes))
}

#[utoipa::path(
    post,
    path = "/api/v1/system/users/import",
    params(ImportQuery),
    request_body(content = String, content_type = "multipart/form-data"),
    responses(
        (status = 200, description = "导入结果报告", body = ImportReport),
        (status = 422, description = "上传缺失或模板不匹配")
    )
)]
#[require_perm("system:user:import")]
#[audit(module = "system", action = "user:import", kind = "import")]
pub async fn import_users(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<ImportQuery>,
    mut multipart: Multipart,
) -> Result<Json<ImportReport>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    let tenant_id = ctx.tenant_id.get();
    let fail_fast = !matches!(q.mode.as_deref(), Some("partial"));

    let mut file_bytes: Option<Vec<u8>> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| bad_request(format!("上传解析失败：{e}")))?
    {
        if field.name() == Some("file") {
            let data = field
                .bytes()
                .await
                .map_err(|e| bad_request(format!("读取文件失败：{e}")))?;
            file_bytes = Some(data.to_vec());
            break;
        }
    }
    let Some(bytes) = file_bytes else {
        return Err(bad_request("缺少上传文件（表单字段名应为 file）"));
    };
    if bytes.len() > MAX_IMPORT_BYTES {
        return Err(bad_request("文件超过 10MB 上限"));
    }

    let rows = import::parse_xlsx(&bytes).map_err(|e| bad_request(e.to_string()))?;
    import::check_headers(&rows, IMPORT_HEADERS).map_err(|e| bad_request(e.to_string()))?;

    let dept_ids = id_set(
        state.db.conn(),
        "SELECT dept_id FROM sys_dept WHERE tenant_id = $1 AND deleted_at IS NULL",
        tenant_id,
    )
    .await?;
    let role_ids = id_set(
        state.db.conn(),
        "SELECT role_id FROM sys_role WHERE tenant_id = $1 AND deleted_at IS NULL",
        tenant_id,
    )
    .await?;
    let existing = string_set(
        state.db.conn(),
        "SELECT user_name FROM sys_user WHERE tenant_id = $1 AND deleted_at IS NULL",
        tenant_id,
    )
    .await?;

    let data_rows = rows.iter().skip(1);
    let total = data_rows.clone().count() as u32;

    let mut valid: Vec<NewUserRow> = Vec::new();
    let mut errors: Vec<RowError> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for row in data_rows {
        match validate_row(row, &dept_ids) {
            Ok(parsed) => {
                if !seen.insert(parsed.user_name.clone()) {
                    errors.push(RowError {
                        row: parsed.row,
                        field: "登录名称".to_string(),
                        reason: format!("文件内重复：{}", parsed.user_name),
                    });
                } else if existing.contains(&parsed.user_name) {
                    errors.push(RowError {
                        row: parsed.row,
                        field: "登录名称".to_string(),
                        reason: format!("账号已存在：{}", parsed.user_name),
                    });
                } else {
                    valid.push(parsed);
                }
            }
            Err(mut row_errors) => errors.append(&mut row_errors),
        }
    }

    // 角色存在性校验（库级）。
    let mut role_errors: Vec<RowError> = Vec::new();
    valid.retain(|u| {
        let missing: Vec<i64> = u
            .role_ids
            .iter()
            .copied()
            .filter(|id| !role_ids.contains(id))
            .collect();
        if missing.is_empty() {
            true
        } else {
            role_errors.push(RowError {
                row: u.row,
                field: "角色编号".to_string(),
                reason: format!("角色不存在：{:?}", missing),
            });
            false
        }
    });
    errors.append(&mut role_errors);

    if fail_fast && !errors.is_empty() {
        return Ok(Json(ImportReport::new(total, 0, errors)));
    }

    // 租户席位闸门：整批新增不得越过套餐上限。
    state
        .quota
        .check_user_slot(ctx.tenant_id, valid.len() as i64)
        .await
        .map_err(quota_error)?;

    let init_password = load_init_password(state.db.conn()).await?;
    // 参数里的初始密码不符合规则要提前拒绝：否则整批导入会造出一堆不合规矩的账号，
    // 而它们的第一个动作就是「被要求改密」。
    if let Some(e) = password_rule_error(&state, &init_password) {
        return Err(e);
    }
    let pw_hash = cicada_auth::hash_password(&init_password)
        .map_err(|e| db_error(format!("密码哈希失败：{e}")))?;

    let txn = state.db.begin().await.map_err(db_error)?;
    let mut success = 0u32;
    for u in &valid {
        let now = chrono::Utc::now();
        match insert_user(
            &txn,
            tenant_id,
            &u.user_name,
            &u.nick_name,
            &pw_hash,
            u.dept_id,
            u.email.as_deref(),
            u.phonenumber.as_deref(),
            u.status,
            u.remark.as_deref(),
            Some(principal.user_name.as_str()),
            now,
        )
        .await
        {
            Ok(user) => {
                if !u.role_ids.is_empty()
                    && let Err(e) =
                        replace_user_roles(&txn, tenant_id, user.user_id, &u.role_ids).await
                {
                    errors.push(RowError {
                        row: u.row,
                        field: "角色编号".to_string(),
                        reason: format!("写入失败：{}", e.detail),
                    });
                    continue;
                }
                success += 1;
            }
            Err(e) => errors.push(RowError {
                row: u.row,
                field: "登录名称".to_string(),
                reason: format!("写入失败：{}", e.detail),
            }),
        }
    }
    txn.commit().await.map_err(db_error)?;
    state.authz.invalidate_tenant(ctx.tenant_id).await;

    Ok(Json(ImportReport::new(total, success, errors)))
}

/// 初始密码取自参数 `sys.user.initPassword`，缺失时回退参考实现默认值。
async fn load_init_password<C: ConnectionTrait>(db: &C) -> Result<String, ApiErr> {
    use sea_orm::{DatabaseBackend, Statement};
    let stmt = Statement::from_string(
        DatabaseBackend::Postgres,
        "SELECT config_value FROM sys_config WHERE config_key = 'sys.user.initPassword' LIMIT 1"
            .to_string(),
    );
    let rows = db.query_all_raw(stmt).await.map_err(db_error)?;
    Ok(rows
        .into_iter()
        .next()
        .and_then(|r| r.try_get_by_index::<String>(0).ok())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "123456".to_string()))
}

pub fn routes(_state: AppState) -> Routes {
    Routes::new()
        .route("/api/v1/system/users", get(list_users), list_users__perms())
        .route_audited(
            "/api/v1/system/users/export",
            get(export_users),
            export_users__perms(),
            export_users__audit(),
        )
        .route(
            "/api/v1/system/users/import-template",
            get(import_template),
            import_template__perms(),
        )
        .route_audited(
            "/api/v1/system/users/import",
            post(import_users),
            import_users__perms(),
            import_users__audit(),
        )
        .route(
            "/api/v1/system/users/{user_id}",
            get(get_user),
            get_user__perms(),
        )
        .route_audited(
            "/api/v1/system/users",
            post(create_user),
            create_user__perms(),
            create_user__audit(),
        )
        .route_audited(
            "/api/v1/system/users/{user_id}",
            put(update_user),
            update_user__perms(),
            update_user__audit(),
        )
        .route_audited(
            "/api/v1/system/users/{user_id}",
            delete(delete_user),
            delete_user__perms(),
            delete_user__audit(),
        )
        .route_audited(
            "/api/v1/system/users/{user_id}/password",
            put(reset_password),
            reset_password__perms(),
            reset_password__audit(),
        )
}

#[cfg(test)]
mod mask_tests {
    use super::*;
    use cicada_kernel::permission::Permission;

    fn sample_vo() -> UserVo {
        UserVo {
            user_id: 2,
            user_name: "zhangsan".into(),
            nick_name: "张三".into(),
            dept_id: None,
            email: Some("zhangsan@corp.com".into()),
            phonenumber: Some("13812345678".into()),
            status: 0,
            create_time: "2026-10-16T00:00:00Z".into(),
            role_ids: vec![],
            version: 0,
        }
    }

    #[test]
    fn list_only_viewer_sees_masked_contacts() {
        let perms = [Permission::new("system:user:list")];
        assert_eq!(mask_policy(false, &perms), MaskPolicy::Masked);
        let vo = sample_vo().masked(mask_policy(false, &perms));
        assert_eq!(vo.phonenumber.as_deref(), Some("138****5678"));
        assert_eq!(vo.email.as_deref(), Some("z*******@corp.com"));
        assert_eq!(vo.user_name, "zhangsan", "账号名不参与脱敏");
    }

    #[test]
    fn editor_and_superadmin_see_plain() {
        let perms = [
            Permission::new("system:user:list"),
            Permission::new("system:user:edit"),
        ];
        assert_eq!(mask_policy(false, &perms), MaskPolicy::Plain);
        assert_eq!(mask_policy(true, &[]), MaskPolicy::Plain);
        let vo = sample_vo().masked(mask_policy(false, &perms));
        assert_eq!(vo.phonenumber.as_deref(), Some("13812345678"));
    }
}
