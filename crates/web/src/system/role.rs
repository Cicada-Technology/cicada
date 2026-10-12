//! 角色管理（权限点 `system:role:*`）。
//!
//! 角色变更（含角色-菜单、角色-部门）后按版本号整体失效权限缓存。

use std::collections::HashSet;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{delete, get, post, put};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect, Set,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use cicada_kernel::scope::Query as ScopedQuery;
use cicada_macros::{audit, require_perm};
use cicada_system::entity::{
    RoleActiveModel, RoleColumn, RoleDeptActiveModel, RoleDeptColumn, RoleDeptEntity, RoleEntity,
    RoleMenuActiveModel, RoleMenuColumn, RoleMenuEntity, RoleModel, UserColumn, UserEntity,
    UserRoleActiveModel, UserRoleColumn, UserRoleEntity,
};

use super::{
    ApiErr, bad_request, conflict, db_error, field_error, forbidden, not_found, ownership,
    unauthorized, version_conflict,
};
use crate::Problem;
use crate::export;
use crate::extractor::RequestContextExt;
use crate::response::created;
use crate::routes::Routes;
use crate::search::Fuzzy;
use crate::sort_select::SelectSortExt;
use crate::state::AppState;

/// 本模块实现的路由权限点（与 `Manifest.perms` 双向对账）。
pub const PERMS: &[&str] = &[
    "system:role:list",
    "system:role:add",
    "system:role:edit",
    "system:role:remove",
    "system:role:export",
];

const BUILTIN_ADMIN_KEY: &str = "admin";

#[derive(Debug, Deserialize, IntoParams)]
pub struct RoleListQuery {
    pub role_name: Option<String>,
    pub role_key: Option<String>,
    pub status: Option<i16>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    /// 排序列名（排序白名单，见 `crate::sort`）；不在清单里则忽略。
    pub sort_by: Option<String>,
    /// `asc` 升序，其余按 `desc`。
    pub sort_order: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RoleVo {
    pub role_id: i64,
    pub role_name: String,
    pub role_key: String,
    pub role_sort: i32,
    pub data_scope: i16,
    pub status: i16,
    pub remark: Option<String>,
    pub create_time: String,
    /// 乐观锁版本（建模规范）。读接口回传，`PUT` 原样带回才做冲突检测。
    pub version: i32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RolePage {
    pub items: Vec<RoleVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct RoleUpsert {
    pub role_name: String,
    pub role_key: String,
    pub role_sort: Option<i32>,
    /// 数据范围 `data_scope`：1 全部 / 2 自定义 / 3 本部门 / 4 本部门及以下 / 5 仅本人。
    pub data_scope: Option<i16>,
    pub status: Option<i16>,
    pub remark: Option<String>,
    /// 角色可访问的菜单 / 权限点集合（`sys_role_menu`）。
    pub menu_ids: Option<Vec<i64>>,
    /// `data_scope = 2`（自定义）时的可见部门（`sys_role_dept`）。
    pub dept_ids: Option<Vec<i64>>,
    /// 乐观锁期望版本（建模规范），只有 `PUT` 读它；`POST` 建号时新版本恒为 0。
    pub version: Option<i32>,
}

impl From<RoleModel> for RoleVo {
    fn from(m: RoleModel) -> Self {
        Self {
            role_id: m.role_id,
            role_name: m.role_name,
            role_key: m.role_key,
            role_sort: m.role_sort,
            data_scope: m.data_scope,
            status: m.status,
            remark: m.remark,
            create_time: m.create_time.to_rfc3339(),
            version: m.version,
        }
    }
}

fn valid_data_scope(v: i16) -> bool {
    (1..=5).contains(&v)
}

async fn replace_role_menus<C: ConnectionTrait>(
    db: &C,
    role_id: i64,
    menu_ids: &[i64],
) -> Result<(), ApiErr> {
    // 菜单是全局字典，但「授一个不存在的菜单」同样要当场拒——它会留下一条解析不出
    // 任何权限点的授权引用（越权防护的归属校验在这里退化成存在性校验）。
    ownership::assert_menus(db, menu_ids).await?;
    RoleMenuEntity::delete_many()
        .filter(RoleMenuColumn::RoleId.eq(role_id))
        .exec(db)
        .await
        .map_err(db_error)?;
    for menu_id in menu_ids {
        RoleMenuActiveModel {
            role_id: Set(role_id),
            menu_id: Set(*menu_id),
        }
        .insert(db)
        .await
        .map_err(db_error)?;
    }
    Ok(())
}

async fn apply_role_depts<C: ConnectionTrait>(
    db: &C,
    tenant: i64,
    role_id: i64,
    data_scope: i16,
    dept_ids: Option<&[i64]>,
) -> Result<(), ApiErr> {
    // 自定义数据范围的部门决定这个角色能看见哪些行，因此它必须留在本租户内（越权防护）。
    if data_scope == 2
        && let Some(ids) = dept_ids
    {
        ownership::assert_depts(db, tenant, ids).await?;
    }
    RoleDeptEntity::delete_many()
        .filter(RoleDeptColumn::RoleId.eq(role_id))
        .exec(db)
        .await
        .map_err(db_error)?;
    if data_scope != 2 {
        return Ok(());
    }
    for dept_id in dept_ids.unwrap_or_default() {
        RoleDeptActiveModel {
            role_id: Set(role_id),
            dept_id: Set(*dept_id),
        }
        .insert(db)
        .await
        .map_err(db_error)?;
    }
    Ok(())
}

/// 角色必须存在且属于当前租户——四个角色子接口都要先过这一关，抽出来一处写。
async fn load_role(state: &AppState, tenant: i64, role_id: i64) -> Result<RoleModel, ApiErr> {
    RoleEntity::find()
        .filter(RoleColumn::RoleId.eq(role_id))
        .filter(RoleColumn::TenantId.eq(tenant))
        .filter(RoleColumn::DeletedAt.is_null())
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)
}

async fn member_ids_of_role(state: &AppState, role_id: i64) -> Result<Vec<i64>, ApiErr> {
    Ok(UserRoleEntity::find()
        .filter(UserRoleColumn::RoleId.eq(role_id))
        .all(state.db.conn())
        .await
        .map_err(db_error)?
        .into_iter()
        .map(|r| r.user_id)
        .collect())
}

/// `sys_role` 列表允许排序的列（排序白名单）。
///
/// 只放「列表页真的会点排序」的列：每多一个可排序列，就多一条能被利用的
/// `ORDER BY` 路径（大表上非索引列排序会把排序成本推给每一次翻页）。
const ROLE_SORT: crate::sort::SortSpec = crate::sort::SortSpec::new(&[
    ("role_name", "sys_role", "role_name"),
    ("role_key", "sys_role", "role_key"),
    ("role_sort", "sys_role", "role_sort"),
    ("status", "sys_role", "status"),
    ("create_time", "sys_role", "create_time"),
]);

#[utoipa::path(
    get,
    path = "/api/v1/system/roles",
    params(RoleListQuery),
    responses((status = 200, description = "角色分页", body = RolePage))
)]
#[require_perm("system:role:list")]
pub async fn list_roles(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<RoleListQuery>,
) -> Result<Json<RolePage>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let mut query = RoleEntity::find()
        .filter(RoleColumn::TenantId.eq(ctx.tenant_id.get()))
        .filter(RoleColumn::DeletedAt.is_null());
    if let Some(name) = q.role_name.filter(|s| !s.is_empty()) {
        query = query.filter(RoleColumn::RoleName.fuzzy(&name));
    }
    if let Some(key) = q.role_key.filter(|s| !s.is_empty()) {
        query = query.filter(RoleColumn::RoleKey.fuzzy(&key));
    }
    if let Some(status) = q.status {
        query = query.filter(RoleColumn::Status.eq(status));
    }

    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;
    let paginator = query
        // 排序白名单：只认这张表声明过的列，客户端字符串进不了标识符位置。
        // 显式排序必须排在前面才真的生效：默认次序退化成同值行的收尾键（role_sort → role_id），
        // 既不改变「不传 sort_by 时的结果」，又保证翻页不重不漏。
        .sorted_by(&ROLE_SORT, q.sort_by.as_deref(), q.sort_order.as_deref())
        .order_by_asc(RoleColumn::RoleSort)
        .order_by_asc(RoleColumn::RoleId)
        .paginate(state.db.conn(), page_size);
    let total = paginator.num_items().await.map_err(db_error)?;
    let items = paginator
        .fetch_page(page - 1)
        .await
        .map_err(db_error)?
        .into_iter()
        .map(RoleVo::from)
        .collect();
    Ok(Json(RolePage {
        items,
        total,
        page,
        page_size,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/system/roles/{role_id}",
    params(("role_id" = i64, Path, description = "角色 id")),
    responses((status = 200, description = "角色详情", body = RoleVo), (status = 404, description = "不存在"))
)]
#[require_perm("system:role:list")]
pub async fn get_role(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(role_id): Path<i64>,
) -> Result<Json<RoleVo>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let role = RoleEntity::find()
        .filter(RoleColumn::RoleId.eq(role_id))
        .filter(RoleColumn::TenantId.eq(ctx.tenant_id.get()))
        .filter(RoleColumn::DeletedAt.is_null())
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    Ok(Json(RoleVo::from(role)))
}

#[utoipa::path(
    get,
    path = "/api/v1/system/roles/{role_id}/menus",
    params(("role_id" = i64, Path, description = "角色 id")),
    responses((status = 200, description = "角色已授权的菜单 id 集合", body = [i64]))
)]
#[require_perm("system:role:list")]
pub async fn list_role_menus(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(role_id): Path<i64>,
) -> Result<Json<Vec<i64>>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    // 先确认角色属于当前租户：不做这一步，跨租户拿别人的 role_id 也能读出授权集合，
    // 而「角色存在与否」本身就是一条不该泄漏的信息。
    let exists = RoleEntity::find()
        .filter(RoleColumn::RoleId.eq(role_id))
        .filter(RoleColumn::TenantId.eq(ctx.tenant_id.get()))
        .filter(RoleColumn::DeletedAt.is_null())
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .is_some();
    if !exists {
        return Err(not_found());
    }

    // `RoleVo` 不带 `menu_ids`，而前端「权限分配」提交时是整体替换（先删后插）：
    // 没有这条读路径，编辑角色就会把未显示的既有授权清空。返回平铺 id 列表而不是树，
    // 因为它就是提交时用的那份 `menu_ids`（接口清单的 `role-tree/{roleId}` 是
    // 带勾选状态的树形版本，界面若改为「树核勾选」再补）。
    let menu_ids = RoleMenuEntity::find()
        .filter(RoleMenuColumn::RoleId.eq(role_id))
        .all(state.db.conn())
        .await
        .map_err(db_error)?
        .into_iter()
        .map(|row| row.menu_id)
        .collect();
    Ok(Json(menu_ids))
}

#[utoipa::path(
    post,
    path = "/api/v1/system/roles",
    request_body = RoleUpsert,
    responses((status = 201, description = "已创建", body = RoleVo), (status = 409, description = "标识重复"))
)]
#[require_perm("system:role:add")]
#[audit(module = "system", action = "role:add", kind = "insert")]
pub async fn create_role(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<RoleUpsert>,
) -> Result<(StatusCode, HeaderMap, Json<RoleVo>), ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    if req.role_name.trim().is_empty() || req.role_key.trim().is_empty() {
        return Err(bad_request("角色名与角色标识不能为空"));
    }
    let data_scope = req.data_scope.unwrap_or(1);
    if !valid_data_scope(data_scope) {
        return Err(bad_request("data_scope 非法（1..=5）"));
    }

    let txn = state.db.begin().await.map_err(db_error)?;
    let exists = RoleEntity::find()
        .filter(RoleColumn::TenantId.eq(ctx.tenant_id.get()))
        .filter(RoleColumn::RoleKey.eq(req.role_key.trim()))
        .filter(RoleColumn::DeletedAt.is_null())
        .one(&txn)
        .await
        .map_err(db_error)?
        .is_some();
    if exists {
        return Err(conflict("角色标识已存在"));
    }

    let role = RoleActiveModel {
        tenant_id: Set(ctx.tenant_id.get()),
        role_name: Set(req.role_name.trim().to_string()),
        role_key: Set(req.role_key.trim().to_string()),
        role_sort: Set(req.role_sort.unwrap_or(0)),
        data_scope: Set(data_scope),
        status: Set(req.status.unwrap_or(0)),
        version: Set(0),
        remark: Set(req.remark.clone()),
        create_by: Set(Some(principal.user_name.clone())),
        create_time: Set(chrono::Utc::now()),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(db_error)?;

    if let Some(menus) = &req.menu_ids {
        replace_role_menus(&txn, role.role_id, menus).await?;
    }
    apply_role_depts(
        &txn,
        ctx.tenant_id.get(),
        role.role_id,
        data_scope,
        req.dept_ids.as_deref(),
    )
    .await?;
    txn.commit().await.map_err(db_error)?;

    // 新角色可能被立即使用；按版本号整体失效，严格安全。
    super::invalidate_grant_caches(&state, ctx.tenant_id).await;
    Ok(created(
        format!("/api/v1/system/roles/{}", role.role_id),
        RoleVo::from(role),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/system/roles/{role_id}",
    request_body = RoleUpsert,
    responses((status = 200, description = "已更新", body = RoleVo), (status = 404, description = "不存在"))
)]
#[require_perm("system:role:edit")]
#[audit(module = "system", action = "role:edit", kind = "update")]
pub async fn update_role(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(role_id): Path<i64>,
    Json(req): Json<RoleUpsert>,
) -> Result<Json<RoleVo>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    let data_scope = req.data_scope.unwrap_or(1);
    if !valid_data_scope(data_scope) {
        return Err(bad_request("data_scope 非法（1..=5）"));
    }

    let txn = state.db.begin().await.map_err(db_error)?;
    let role = RoleEntity::find()
        .filter(RoleColumn::RoleId.eq(role_id))
        .filter(RoleColumn::TenantId.eq(ctx.tenant_id.get()))
        .filter(RoleColumn::DeletedAt.is_null())
        .one(&txn)
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;

    // 内置超管：只允许改名称/排序/备注，禁止改标识与数据范围。
    let builtin = role.role_key == BUILTIN_ADMIN_KEY;
    if builtin && (req.role_key.trim() != role.role_key || data_scope != role.data_scope) {
        return Err(forbidden("内置超级管理员角色的标识与数据范围不可修改"));
    }

    if !builtin {
        let dup = RoleEntity::find()
            .filter(RoleColumn::TenantId.eq(ctx.tenant_id.get()))
            .filter(RoleColumn::RoleKey.eq(req.role_key.trim()))
            .filter(RoleColumn::RoleId.ne(role_id))
            .filter(RoleColumn::DeletedAt.is_null())
            .one(&txn)
            .await
            .map_err(db_error)?
            .is_some();
        if dup {
            return Err(conflict("角色标识已存在"));
        }
    }

    // 留痕快照（审计设计）：本接口能改的列。
    let before = serde_json::json!({
        "role_name": role.role_name,
        "role_key": role.role_key,
        "role_sort": role.role_sort,
        "data_scope": role.data_scope,
        "status": role.status,
        "remark": role.remark,
    });

    // 建模规范乐观锁：手写 CAS（SeaORM 2.0.4 没有原生 opt-lock，也没有 `#[sea_orm(version)]`
    // 这个属性）。判定只看 `sys_role` 这一行：菜单与部门关联是本角色的从属数据，
    // 它们的重写不该让一次角色编辑失败。
    let expected = req.version.unwrap_or(role.version);
    let applied = RoleEntity::update_many()
        .col_expr(
            RoleColumn::RoleName,
            sea_orm::sea_query::Expr::value(req.role_name.trim().to_string()),
        )
        .col_expr(
            RoleColumn::RoleKey,
            sea_orm::sea_query::Expr::value(req.role_key.trim().to_string()),
        )
        .col_expr(
            RoleColumn::RoleSort,
            sea_orm::sea_query::Expr::value(req.role_sort.unwrap_or(role.role_sort)),
        )
        .col_expr(
            RoleColumn::DataScope,
            sea_orm::sea_query::Expr::value(data_scope),
        )
        .col_expr(
            RoleColumn::Status,
            sea_orm::sea_query::Expr::value(req.status.unwrap_or(role.status)),
        )
        .col_expr(
            RoleColumn::Remark,
            sea_orm::sea_query::Expr::value(req.remark.clone()),
        )
        .col_expr(
            RoleColumn::UpdateBy,
            sea_orm::sea_query::Expr::value(Some(principal.user_name.clone())),
        )
        .col_expr(
            RoleColumn::UpdateTime,
            sea_orm::sea_query::Expr::value(Some(chrono::Utc::now())),
        )
        .col_expr(
            RoleColumn::Version,
            sea_orm::sea_query::Expr::value(expected + 1),
        )
        .filter(RoleColumn::RoleId.eq(role_id))
        .filter(RoleColumn::Version.eq(expected))
        .exec(&txn)
        .await
        .map_err(db_error)?;
    if applied.rows_affected == 0 {
        return Err(version_conflict());
    }

    if let Some(menus) = &req.menu_ids {
        replace_role_menus(&txn, role_id, menus).await?;
    }
    apply_role_depts(
        &txn,
        ctx.tenant_id.get(),
        role_id,
        data_scope,
        req.dept_ids.as_deref(),
    )
    .await?;
    txn.commit().await.map_err(db_error)?;

    super::invalidate_grant_caches(&state, ctx.tenant_id).await;

    let updated = RoleEntity::find_by_id(role_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    crate::audit::record_change(
        &state.audit,
        &ctx,
        "sys_role",
        role_id.to_string(),
        before,
        serde_json::json!({
            "role_name": updated.role_name,
            "role_key": updated.role_key,
            "role_sort": updated.role_sort,
            "data_scope": updated.data_scope,
            "status": updated.status,
            "remark": updated.remark,
        }),
    )
    .await;
    Ok(Json(RoleVo::from(updated)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/system/roles/{role_id}",
    params(("role_id" = i64, Path, description = "角色 id")),
    responses((status = 204, description = "已删除"), (status = 403, description = "内置角色不可删"))
)]
#[require_perm("system:role:remove")]
#[audit(module = "system", action = "role:remove", kind = "delete")]
pub async fn delete_role(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(role_id): Path<i64>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let txn = state.db.begin().await.map_err(db_error)?;
    let role = RoleEntity::find()
        .filter(RoleColumn::RoleId.eq(role_id))
        .filter(RoleColumn::TenantId.eq(ctx.tenant_id.get()))
        .filter(RoleColumn::DeletedAt.is_null())
        .one(&txn)
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    if role.role_key == BUILTIN_ADMIN_KEY {
        return Err(forbidden("内置超级管理员角色不可删除"));
    }

    let now = chrono::Utc::now();
    RoleActiveModel {
        role_id: Set(role_id),
        deleted_at: Set(Some(now)),
        update_time: Set(Some(now)),
        ..Default::default()
    }
    .update(&txn)
    .await
    .map_err(db_error)?;
    RoleMenuEntity::delete_many()
        .filter(RoleMenuColumn::RoleId.eq(role_id))
        .exec(&txn)
        .await
        .map_err(db_error)?;
    RoleDeptEntity::delete_many()
        .filter(RoleDeptColumn::RoleId.eq(role_id))
        .exec(&txn)
        .await
        .map_err(db_error)?;
    UserRoleEntity::delete_many()
        .filter(UserRoleColumn::RoleId.eq(role_id))
        .exec(&txn)
        .await
        .map_err(db_error)?;
    txn.commit().await.map_err(db_error)?;

    super::invalidate_grant_caches(&state, ctx.tenant_id).await;
    Ok(StatusCode::NO_CONTENT)
}

fn data_scope_label(v: i16) -> &'static str {
    match v {
        1 => "全部",
        2 => "自定义",
        3 => "本部门",
        4 => "本部门及以下",
        5 => "仅本人",
        _ => "-",
    }
}

const EXPORT_HEADERS: &[&str] = &[
    "角色编号",
    "角色名称",
    "权限字符",
    "显示顺序",
    "数据范围",
    "状态",
    "创建时间",
];
const EXPORT_WIDTHS: &[f64] = &[10.0, 18.0, 18.0, 10.0, 14.0, 8.0, 22.0];

#[utoipa::path(
    get,
    path = "/api/v1/system/roles/export",
    params(RoleListQuery),
    responses((status = 200, description = "角色 xlsx 导出"))
)]
#[require_perm("system:role:export")]
#[audit(module = "system", action = "role:export", kind = "export")]
pub async fn export_roles(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<RoleListQuery>,
) -> Result<Response, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let tenant = ctx.tenant_id.get();
    let conn = state.db.conn().clone();
    let name = q.role_name.filter(|s| !s.is_empty());
    let key = q.role_key.filter(|s| !s.is_empty());
    let status = q.status;

    let bytes = export::build("角色", EXPORT_HEADERS, EXPORT_WIDTHS, |sheet| {
        Box::pin(async move {
            let mut row: u32 = 1;
            let mut offset: u64 = 0;
            while u64::from(row) - 1 < export::MAX_EXPORT_ROWS {
                let mut query = RoleEntity::find()
                    .filter(RoleColumn::TenantId.eq(tenant))
                    .filter(RoleColumn::DeletedAt.is_null());
                if let Some(n) = &name {
                    query = query.filter(RoleColumn::RoleName.fuzzy(n));
                }
                if let Some(k) = &key {
                    query = query.filter(RoleColumn::RoleKey.fuzzy(k));
                }
                if let Some(s) = status {
                    query = query.filter(RoleColumn::Status.eq(s));
                }
                let batch = query
                    .order_by_asc(RoleColumn::RoleSort)
                    .order_by_asc(RoleColumn::RoleId)
                    .limit(export::EXPORT_BATCH)
                    .offset(offset)
                    .all(&conn)
                    .await?;
                if batch.is_empty() {
                    break;
                }
                for r in &batch {
                    sheet.write_string(row, 0, r.role_id.to_string())?;
                    sheet.write_string(row, 1, &r.role_name)?;
                    sheet.write_string(row, 2, &r.role_key)?;
                    sheet.write_string(row, 3, r.role_sort.to_string())?;
                    sheet.write_string(row, 4, data_scope_label(r.data_scope))?;
                    sheet.write_string(row, 5, if r.status == 0 { "正常" } else { "停用" })?;
                    sheet.write_string(row, 6, r.create_time.to_rfc3339())?;
                    row += 1;
                }
                offset += batch.len() as u64;
            }
            Ok(())
        })
    })
    .await
    .map_err(db_error)?;

    Ok(export::xlsx_response("roles.xlsx", bytes))
}

/// `PUT /system/roles/{id}/status`：启停角色（接口清单）。
///
/// 与 `PUT /system/roles/{id}` 分开，因为界面是表格里的开关：整体更新会把
/// `menu_ids`/`dept_ids` 的「未提交即清空」风险带进一个只想改状态的点击。
#[derive(Debug, Deserialize, ToSchema)]
pub struct RoleStatusUpdate {
    /// 0 正常 / 1 停用。
    pub status: i16,
}

#[utoipa::path(
    put,
    path = "/api/v1/system/roles/{role_id}/status",
    params(("role_id" = i64, Path, description = "角色 id")),
    request_body = RoleStatusUpdate,
    responses(
        (status = 200, description = "已更新", body = RoleVo),
        (status = 422, description = "状态值非法", body = Problem),
        (status = 404, description = "不存在")
    )
)]
#[require_perm("system:role:edit")]
#[audit(module = "system", action = "role:status", kind = "update")]
pub async fn set_role_status(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(role_id): Path<i64>,
    Json(req): Json<RoleStatusUpdate>,
) -> Result<Json<RoleVo>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    if !matches!(req.status, 0 | 1) {
        return Err(field_error("status", "只能是 0（正常）或 1（停用）"));
    }
    let role = load_role(&state, ctx.tenant_id.get(), role_id).await?;
    if role.role_key == "admin" {
        return Err(forbidden("内置超级角色不可停用"));
    }

    RoleActiveModel {
        role_id: Set(role_id),
        status: Set(req.status),
        update_by: Set(Some(principal.user_name.clone())),
        update_time: Set(Some(chrono::Utc::now())),
        ..Default::default()
    }
    .update(state.db.conn())
    .await
    .map_err(db_error)?;

    // 停用角色等于收回它带着的全部权限点。
    super::invalidate_grant_caches(&state, ctx.tenant_id).await;
    let updated = RoleEntity::find_by_id(role_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    crate::audit::record_change(
        &state.audit,
        &ctx,
        "sys_role",
        role_id.to_string(),
        serde_json::json!({ "status": role.status }),
        serde_json::json!({ "status": updated.status }),
    )
    .await;
    Ok(Json(RoleVo::from(updated)))
}

/// `PUT /system/roles/{id}/data-scope`：只改数据范围与其自定部门（接口清单）。
#[derive(Debug, Deserialize, ToSchema)]
pub struct RoleDataScopeUpdate {
    /// 1 全部 / 2 自定 / 3 本部门 / 4 本部门及以下 / 5 仅本人。
    pub data_scope: i16,
    /// `data_scope = 2` 时生效；其它取值忽略。
    pub dept_ids: Option<Vec<i64>>,
}

#[utoipa::path(
    put,
    path = "/api/v1/system/roles/{role_id}/data-scope",
    params(("role_id" = i64, Path, description = "角色 id")),
    request_body = RoleDataScopeUpdate,
    responses(
        (status = 200, description = "已更新", body = RoleVo),
        (status = 422, description = "数据范围非法", body = Problem),
        (status = 404, description = "不存在")
    )
)]
#[require_perm("system:role:edit")]
#[audit(module = "system", action = "role:scope", kind = "update")]
pub async fn set_role_data_scope(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(role_id): Path<i64>,
    Json(req): Json<RoleDataScopeUpdate>,
) -> Result<Json<RoleVo>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    if !valid_data_scope(req.data_scope) {
        return Err(field_error("data_scope", "只能是 1..=5"));
    }
    let role = load_role(&state, ctx.tenant_id.get(), role_id).await?;
    let dept_ids = req.dept_ids.clone().unwrap_or_default();
    if req.data_scope == 2 && dept_ids.is_empty() {
        return Err(field_error("dept_ids", "自定数据范围至少要选一个部门"));
    }

    let txn = state.db.begin().await.map_err(db_error)?;
    RoleActiveModel {
        role_id: Set(role_id),
        data_scope: Set(req.data_scope),
        update_by: Set(Some(principal.user_name.clone())),
        update_time: Set(Some(chrono::Utc::now())),
        ..Default::default()
    }
    .update(&txn)
    .await
    .map_err(db_error)?;
    apply_role_depts(
        &txn,
        ctx.tenant_id.get(),
        role_id,
        req.data_scope,
        Some(&dept_ids),
    )
    .await?;
    txn.commit().await.map_err(db_error)?;

    super::invalidate_grant_caches(&state, ctx.tenant_id).await;
    let updated = RoleEntity::find_by_id(role_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    crate::audit::record_change(
        &state.audit,
        &ctx,
        "sys_role",
        role_id.to_string(),
        serde_json::json!({ "data_scope": role.data_scope }),
        serde_json::json!({ "data_scope": updated.data_scope, "dept_ids": dept_ids }),
    )
    .await;
    Ok(Json(RoleVo::from(updated)))
}

/// `GET /system/roles/{id}/users`：本租户用户 + 是否已属于该角色（接口清单）。
///
/// 一张列表带 `belonged` 标记，同时服务「已分配」与「未分配」两个页签：
/// 两个端点各自分页会让「搜索 + 翻页」在两处重复实现，而它们要的其实是同一份数据。
#[derive(Debug, Deserialize, IntoParams)]
pub struct RoleUserQuery {
    pub user_name: Option<String>,
    /// true 只看已分配，false 只看未分配，省略看全部。
    pub belonged: Option<bool>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RoleUserVo {
    pub user_id: i64,
    pub user_name: String,
    pub nick_name: String,
    pub status: i16,
    pub belonged: bool,
}

/// 批量分配 / 取消的结果：`changed` 是真正动过的条数（重复分配幂等，不计入）。
#[derive(Debug, Serialize, ToSchema)]
pub struct RoleUsersResult {
    pub action: String,
    pub changed: usize,
    pub requested: usize,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RoleUserPage {
    pub items: Vec<RoleUserVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

#[utoipa::path(
    get,
    path = "/api/v1/system/roles/{role_id}/users",
    params(RoleUserQuery),
    responses((status = 200, description = "角色用户分页", body = RoleUserPage))
)]
#[require_perm("system:role:list")]
pub async fn list_role_users(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(role_id): Path<i64>,
    Query(q): Query<RoleUserQuery>,
) -> Result<Json<RoleUserPage>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    load_role(&state, ctx.tenant_id.get(), role_id).await?;
    let member_ids = member_ids_of_role(&state, role_id).await?;

    let mut query = ScopedQuery::<UserEntity>::open().scoped(&ctx.scope());
    query = query.filter(UserColumn::DeletedAt.is_null());
    if let Some(name) = q.user_name.filter(|s| !s.is_empty()) {
        query = query.filter(UserColumn::UserName.fuzzy(&name));
    }
    if let Some(true) = q.belonged {
        query = query.filter(UserColumn::UserId.is_in(member_ids.clone()));
    } else if let Some(false) = q.belonged {
        query = query.filter(UserColumn::UserId.is_not_in(member_ids.clone()));
    }

    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;
    let paged = query
        .order_by(UserColumn::UserId)
        .fetch_page(state.db.conn(), page, page_size)
        .await
        .map_err(db_error)?;
    let members: HashSet<i64> = member_ids.into_iter().collect();
    Ok(Json(RoleUserPage {
        items: paged
            .items
            .into_iter()
            .map(|u| RoleUserVo {
                user_id: u.user_id,
                user_name: u.user_name,
                nick_name: u.nick_name,
                status: u.status,
                belonged: members.contains(&u.user_id),
            })
            .collect(),
        total: paged.total,
        page: paged.page,
        page_size: paged.page_size,
    }))
}

/// `PUT /system/roles/{id}/users`：批量分配 / 取消分配用户（接口清单）。
#[derive(Debug, Deserialize, ToSchema)]
pub struct RoleUsersUpdate {
    /// `allocate` 分配 / `cancel` 取消。
    pub action: String,
    pub user_ids: Vec<i64>,
}

#[utoipa::path(
    put,
    path = "/api/v1/system/roles/{role_id}/users",
    params(("role_id" = i64, Path, description = "角色 id")),
    request_body = RoleUsersUpdate,
    responses(
        (status = 200, description = "已处理条数", body = RoleUsersResult),
        (status = 422, description = "参数或归属不合法", body = Problem)
    )
)]
#[require_perm("system:role:edit")]
#[audit(module = "system", action = "role:users", kind = "update")]
pub async fn update_role_users(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(role_id): Path<i64>,
    Json(req): Json<RoleUsersUpdate>,
) -> Result<Json<RoleUsersResult>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    if !matches!(req.action.as_str(), "allocate" | "cancel") {
        return Err(field_error("action", "只能是 allocate 或 cancel"));
    }
    if req.user_ids.is_empty() {
        return Err(field_error("user_ids", "不能为空"));
    }
    load_role(&state, ctx.tenant_id.get(), role_id).await?;

    // 越权防护：只能操作当前租户可见的用户。少一个就整批拒绝——
    // 部分成功会让调用方以为是一次事务里的操作。
    let visible = ScopedQuery::<UserEntity>::open()
        .scoped(&ctx.scope())
        .filter(UserColumn::UserId.is_in(req.user_ids.clone()))
        .filter(UserColumn::DeletedAt.is_null())
        .count(state.db.conn())
        .await
        .map_err(db_error)?;
    if visible as usize != req.user_ids.len() {
        return Err(field_error(
            "user_ids",
            "有用户不在当前租户可见范围内，整批未执行",
        ));
    }

    let existing = UserRoleEntity::find()
        .filter(UserRoleColumn::RoleId.eq(role_id))
        .filter(UserRoleColumn::UserId.is_in(req.user_ids.clone()))
        .all(state.db.conn())
        .await
        .map_err(db_error)?
        .into_iter()
        .map(|r| r.user_id)
        .collect::<HashSet<_>>();

    let conn = state.db.conn();
    let mut changed = 0usize;
    if req.action == "allocate" {
        for user_id in &req.user_ids {
            if existing.contains(user_id) {
                continue;
            }
            UserRoleActiveModel {
                user_id: Set(*user_id),
                role_id: Set(role_id),
            }
            .insert(conn)
            .await
            .map_err(db_error)?;
            changed += 1;
        }
    } else {
        changed = UserRoleEntity::delete_many()
            .filter(UserRoleColumn::RoleId.eq(role_id))
            .filter(UserRoleColumn::UserId.is_in(req.user_ids.clone()))
            .exec(conn)
            .await
            .map_err(db_error)?
            .rows_affected as usize;
    }

    // 角色成员变了 = 这些用户的权限集合变了。
    super::invalidate_grant_caches(&state, ctx.tenant_id).await;
    Ok(Json(RoleUsersResult {
        action: req.action,
        changed,
        requested: req.user_ids.len(),
    }))
}

pub fn routes(_state: AppState) -> Routes {
    Routes::new()
        .route("/api/v1/system/roles", get(list_roles), list_roles__perms())
        .route_audited(
            "/api/v1/system/roles/export",
            get(export_roles),
            export_roles__perms(),
            export_roles__audit(),
        )
        .route(
            "/api/v1/system/roles/{role_id}",
            get(get_role),
            get_role__perms(),
        )
        .route(
            "/api/v1/system/roles/{role_id}/menus",
            get(list_role_menus),
            list_role_menus__perms(),
        )
        .route(
            "/api/v1/system/roles/{role_id}/users",
            get(list_role_users),
            list_role_users__perms(),
        )
        .route_audited(
            "/api/v1/system/roles/{role_id}/users",
            put(update_role_users),
            update_role_users__perms(),
            update_role_users__audit(),
        )
        .route_audited(
            "/api/v1/system/roles/{role_id}/status",
            put(set_role_status),
            set_role_status__perms(),
            set_role_status__audit(),
        )
        .route_audited(
            "/api/v1/system/roles/{role_id}/data-scope",
            put(set_role_data_scope),
            set_role_data_scope__perms(),
            set_role_data_scope__audit(),
        )
        .route_audited(
            "/api/v1/system/roles",
            post(create_role),
            create_role__perms(),
            create_role__audit(),
        )
        .route_audited(
            "/api/v1/system/roles/{role_id}",
            put(update_role),
            update_role__perms(),
            update_role__audit(),
        )
        .route_audited(
            "/api/v1/system/roles/{role_id}",
            delete(delete_role),
            delete_role__perms(),
            delete_role__audit(),
        )
}
