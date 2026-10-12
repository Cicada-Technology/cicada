//! 租户与套餐管理：`system:tenant:*` / `system:package:*`。
//!
//! 这是**平台级**操作：`sys_tenant` 不带 `tenant_id`，管理它意味着跨租户。
//! 因此这些接口在权限点之外还要求 `is_super_admin`，避免租户管理员改到自己头上。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post, put};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, Set,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use cicada_kernel::TenantId;
use cicada_macros::{audit, require_perm};
use cicada_system::entity::{
    TenantActiveModel, TenantColumn, TenantEntity, TenantModel, TenantPackageActiveModel,
    TenantPackageColumn, TenantPackageEntity, TenantPackageModel,
};

use super::{
    ApiErr, bad_request, conflict, db_error, forbidden, not_found, referenced, unauthorized,
};
use crate::extractor::RequestContextExt;
use crate::response::created;
use crate::routes::Routes;
use crate::search::Fuzzy;
use crate::sort_select::SelectSortExt;
use crate::state::AppState;

pub const PERMS: &[&str] = &[
    "system:tenant:list",
    "system:tenant:add",
    "system:tenant:edit",
    "system:tenant:remove",
    "system:package:list",
    "system:package:add",
    "system:package:edit",
    "system:package:remove",
];

fn require_platform(ctx: &cicada_kernel::RequestContext) -> Result<(), ApiErr> {
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    if !ctx.is_super_admin {
        return Err(forbidden("租户与套餐管理是平台级操作，仅超级管理员可用"));
    }
    Ok(())
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct TenantListQuery {
    pub tenant_name: Option<String>,
    pub status: Option<i16>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    /// 排序列名（排序白名单，见 `crate::sort`）；不在清单里则忽略。
    pub sort_by: Option<String>,
    /// `asc` 升序，其余按 `desc`。
    pub sort_order: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TenantVo {
    pub tenant_id: i64,
    pub tenant_name: String,
    pub status: i16,
    pub package_id: Option<i64>,
    pub package_name: Option<String>,
    pub remark: Option<String>,
    pub create_time: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TenantPage {
    pub items: Vec<TenantVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

/// 租户用量与限额快照（`-1` = 不限）。
#[derive(Debug, Serialize, ToSchema)]
pub struct QuotaVo {
    pub tenant_id: i64,
    pub package_code: Option<String>,
    pub user_limit: i32,
    pub storage_limit_mb: i64,
    pub api_qps_limit: i32,
    pub users: i64,
    pub storage_mb: i64,
    pub features: serde_json::Value,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TenantUpsert {
    pub tenant_name: String,
    #[serde(default)]
    pub status: i16,
    pub package_id: Option<i64>,
    pub remark: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct PackageListQuery {
    pub code: Option<String>,
    pub name: Option<String>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    /// 排序列名（排序白名单，见 `crate::sort`）；不在清单里则忽略。
    pub sort_by: Option<String>,
    /// `asc` 升序，其余按 `desc`。
    pub sort_order: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PackageVo {
    pub id: i64,
    pub code: String,
    pub name: String,
    pub user_limit: i32,
    pub storage_limit_mb: i64,
    pub api_qps_limit: i32,
    pub features: serde_json::Value,
    pub status: i16,
    pub remark: Option<String>,
    pub create_time: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PackagePage {
    pub items: Vec<PackageVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PackageUpsert {
    pub code: String,
    pub name: String,
    /// -1 = 不限。
    #[serde(default = "unlimited_i32")]
    pub user_limit: i32,
    #[serde(default = "unlimited_i64")]
    pub storage_limit_mb: i64,
    #[serde(default = "unlimited_i32")]
    pub api_qps_limit: i32,
    #[serde(default)]
    pub features: serde_json::Value,
    #[serde(default)]
    pub status: i16,
    pub remark: Option<String>,
}

fn unlimited_i32() -> i32 {
    -1
}

fn unlimited_i64() -> i64 {
    -1
}

impl From<TenantPackageModel> for PackageVo {
    fn from(m: TenantPackageModel) -> Self {
        Self {
            id: m.id,
            code: m.code,
            name: m.name,
            user_limit: m.user_limit,
            storage_limit_mb: m.storage_limit_mb,
            api_qps_limit: m.api_qps_limit,
            features: m.features,
            status: m.status,
            remark: m.remark,
            create_time: m.create_time.to_rfc3339(),
        }
    }
}

async fn tenant_vo(state: &AppState, m: TenantModel) -> Result<TenantVo, ApiErr> {
    let package_name = match m.package_id {
        Some(id) => TenantPackageEntity::find_by_id(id)
            .one(state.db.conn())
            .await
            .map_err(db_error)?
            .map(|p| p.name),
        None => None,
    };
    Ok(TenantVo {
        tenant_id: m.tenant_id,
        tenant_name: m.tenant_name,
        status: m.status,
        package_id: m.package_id,
        package_name,
        remark: m.remark,
        create_time: m.create_time.to_rfc3339(),
    })
}

/// `sys_tenant` 列表允许排序的列（排序白名单）。
///
/// 只放「列表页真的会点排序」的列：每多一个可排序列，就多一条能被利用的
/// `ORDER BY` 路径（大表上非索引列排序会把排序成本推给每一次翻页）。
const TENANT_SORT: crate::sort::SortSpec = crate::sort::SortSpec::new(&[
    ("tenant_name", "sys_tenant", "tenant_name"),
    ("status", "sys_tenant", "status"),
    ("create_time", "sys_tenant", "create_time"),
]);

#[utoipa::path(
    get,
    path = "/api/v1/system/tenants",
    params(TenantListQuery),
    responses((status = 200, description = "租户分页", body = TenantPage), (status = 403, description = "非平台级操作"))
)]
#[require_perm("system:tenant:list")]
pub async fn list_tenants(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<TenantListQuery>,
) -> Result<Json<TenantPage>, ApiErr> {
    let ctx = ctx.into_inner();
    require_platform(&ctx)?;

    let mut query = TenantEntity::find();
    if let Some(name) = q.tenant_name.filter(|s| !s.is_empty()) {
        query = query.filter(TenantColumn::TenantName.fuzzy(&name));
    }
    if let Some(status) = q.status {
        query = query.filter(TenantColumn::Status.eq(status));
    }
    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;
    let paginator = query
        // 排序白名单：只认这张表声明过的列，客户端字符串进不了标识符位置。
        // 显式排序必须排在前面才真的生效：默认次序退化成同值行的收尾键（tenant_id），
        // 既不改变「不传 sort_by 时的结果」，又保证翻页不重不漏。
        .sorted_by(&TENANT_SORT, q.sort_by.as_deref(), q.sort_order.as_deref())
        .order_by_asc(TenantColumn::TenantId)
        .paginate(state.db.conn(), page_size);
    let total = paginator.num_items().await.map_err(db_error)?;
    let rows = paginator
        .fetch_page(page.saturating_sub(1))
        .await
        .map_err(db_error)?;

    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        items.push(tenant_vo(&state, row).await?);
    }
    Ok(Json(TenantPage {
        items,
        total,
        page,
        page_size,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/system/tenants/{tenant_id}/quota",
    params(("tenant_id" = i64, Path, description = "租户 id")),
    responses((status = 200, description = "租户用量与限额", body = QuotaVo))
)]
#[require_perm("system:tenant:list")]
pub async fn get_quota(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(tenant_id): Path<i64>,
) -> Result<Json<QuotaVo>, ApiErr> {
    let ctx = ctx.into_inner();
    require_platform(&ctx)?;

    // 先确认租户存在（用量快照对不存在的租户没有意义）。
    let _tenant = TenantEntity::find_by_id(tenant_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    let usage = state
        .quota
        .usage(TenantId::new(tenant_id))
        .await
        .map_err(db_error)?;
    let package = state
        .quota
        .package(TenantId::new(tenant_id))
        .await
        .map_err(db_error)?;

    Ok(Json(QuotaVo {
        tenant_id,
        package_code: package.as_ref().map(|p| p.code.clone()),
        user_limit: package.as_ref().map(|p| p.user_limit).unwrap_or(-1),
        storage_limit_mb: package.as_ref().map(|p| p.storage_limit_mb).unwrap_or(-1),
        api_qps_limit: package.as_ref().map(|p| p.api_qps_limit).unwrap_or(-1),
        users: usage.users,
        storage_mb: usage.storage_mb(),
        features: package
            .map(|p| p.features)
            .unwrap_or_else(|| serde_json::json!({})),
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/system/tenants",
    request_body = TenantUpsert,
    responses((status = 201, description = "已创建", body = TenantVo))
)]
#[require_perm("system:tenant:add")]
#[audit(module = "system", action = "tenant:add", kind = "insert")]
pub async fn create_tenant(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<TenantUpsert>,
) -> Result<(StatusCode, HeaderMap, Json<TenantVo>), ApiErr> {
    let ctx = ctx.into_inner();
    require_platform(&ctx)?;
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    let name = req.tenant_name.trim();
    if name.is_empty() {
        return Err(bad_request("租户名称不能为空"));
    }
    if let Some(id) = req.package_id {
        ensure_package(&state, id).await?;
    }
    let row = TenantActiveModel {
        tenant_name: Set(name.to_string()),
        status: Set(req.status),
        package_id: Set(req.package_id),
        remark: Set(req.remark.clone()),
        create_by: Set(Some(principal.user_name.clone())),
        create_time: Set(chrono::Utc::now()),
        ..Default::default()
    }
    .insert(state.db.conn())
    .await
    .map_err(db_error)?;
    Ok(created(
        format!("/api/v1/system/tenants/{}", row.tenant_id),
        tenant_vo(&state, row).await?,
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/system/tenants/{tenant_id}",
    request_body = TenantUpsert,
    params(("tenant_id" = i64, Path, description = "租户 id")),
    responses((status = 200, description = "已更新", body = TenantVo), (status = 404, description = "不存在"))
)]
#[require_perm("system:tenant:edit")]
#[audit(module = "system", action = "tenant:edit", kind = "update")]
pub async fn update_tenant(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(tenant_id): Path<i64>,
    Json(req): Json<TenantUpsert>,
) -> Result<Json<TenantVo>, ApiErr> {
    let ctx = ctx.into_inner();
    require_platform(&ctx)?;
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    let name = req.tenant_name.trim();
    if name.is_empty() {
        return Err(bad_request("租户名称不能为空"));
    }
    if req.package_id != Some(1) && tenant_id == 1 && req.status != 0 {
        return Err(bad_request("默认租户不可停用"));
    }
    if let Some(id) = req.package_id {
        ensure_package(&state, id).await?;
    }
    // 留痕（审计设计）：先读当前行；读不到时仍由更新的 `RecordNotUpdated` 分支报 404。
    let previous = TenantEntity::find_by_id(tenant_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?;
    let before = previous
        .as_ref()
        .map(|t| serde_json::json!({"tenant_name": t.tenant_name, "status": t.status, "package_id": t.package_id, "remark": t.remark}));

    let row = TenantActiveModel {
        tenant_id: Set(tenant_id),
        tenant_name: Set(name.to_string()),
        status: Set(req.status),
        package_id: Set(req.package_id),
        remark: Set(req.remark.clone()),
        update_by: Set(Some(principal.user_name.clone())),
        update_time: Set(Some(chrono::Utc::now())),
        ..Default::default()
    }
    .update(state.db.conn())
    .await
    .map_err(|e| match e {
        sea_orm::DbErr::RecordNotUpdated => not_found(),
        other => db_error(other),
    })?;
    if let Some(before) = before {
        crate::audit::record_change(
            &state.audit,
            &ctx,
            "sys_tenant",
            tenant_id.to_string(),
            before,
            serde_json::json!({"tenant_name": row.tenant_name, "status": row.status, "package_id": row.package_id, "remark": row.remark}),
        )
        .await;
    }
    Ok(Json(tenant_vo(&state, row).await?))
}

#[utoipa::path(
    delete,
    path = "/api/v1/system/tenants/{tenant_id}",
    params(("tenant_id" = i64, Path, description = "租户 id")),
    responses((status = 204, description = "已删除"), (status = 409, description = "仍有数据"))
)]
#[require_perm("system:tenant:remove")]
#[audit(module = "system", action = "tenant:remove", kind = "delete")]
pub async fn delete_tenant(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(tenant_id): Path<i64>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    require_platform(&ctx)?;
    if tenant_id == 1 {
        return Err(conflict("默认租户不可删除"));
    }
    let usage = state
        .quota
        .usage(TenantId::new(tenant_id))
        .await
        .map_err(db_error)?;
    if usage.users > 0 || usage.storage_bytes > 0 {
        return Err(referenced("租户下仍有用户或文件，拒绝删除"));
    }
    let res = TenantEntity::delete_by_id(tenant_id)
        .exec(state.db.conn())
        .await
        .map_err(db_error)?;
    if res.rows_affected == 0 {
        return Err(not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn ensure_package(state: &AppState, id: i64) -> Result<(), ApiErr> {
    let exists = TenantPackageEntity::find_by_id(id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .is_some();
    if !exists {
        return Err(bad_request("套餐不存在"));
    }
    Ok(())
}

/// `sys_tenant_package` 列表允许排序的列（排序白名单）。
///
/// 只放「列表页真的会点排序」的列：每多一个可排序列，就多一条能被利用的
/// `ORDER BY` 路径（大表上非索引列排序会把排序成本推给每一次翻页）。
const PACKAGE_SORT: crate::sort::SortSpec = crate::sort::SortSpec::new(&[
    ("code", "sys_tenant_package", "code"),
    ("name", "sys_tenant_package", "name"),
    ("user_limit", "sys_tenant_package", "user_limit"),
]);

#[utoipa::path(
    get,
    path = "/api/v1/system/tenant-packages",
    params(PackageListQuery),
    responses((status = 200, description = "套餐分页", body = PackagePage))
)]
#[require_perm("system:package:list")]
pub async fn list_packages(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<PackageListQuery>,
) -> Result<Json<PackagePage>, ApiErr> {
    let ctx = ctx.into_inner();
    require_platform(&ctx)?;

    let mut query = TenantPackageEntity::find();
    if let Some(code) = q.code.filter(|s| !s.is_empty()) {
        query = query.filter(TenantPackageColumn::Code.fuzzy(&code));
    }
    if let Some(name) = q.name.filter(|s| !s.is_empty()) {
        query = query.filter(TenantPackageColumn::Name.fuzzy(&name));
    }
    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;
    let paginator = query
        // 排序白名单：只认这张表声明过的列，客户端字符串进不了标识符位置。
        // 显式排序必须排在前面才真的生效：默认次序退化成同值行的收尾键（id），
        // 既不改变「不传 sort_by 时的结果」，又保证翻页不重不漏。
        .sorted_by(&PACKAGE_SORT, q.sort_by.as_deref(), q.sort_order.as_deref())
        .order_by_asc(TenantPackageColumn::Id)
        .paginate(state.db.conn(), page_size);
    let total = paginator.num_items().await.map_err(db_error)?;
    let items = paginator
        .fetch_page(page.saturating_sub(1))
        .await
        .map_err(db_error)?
        .into_iter()
        .map(PackageVo::from)
        .collect();
    Ok(Json(PackagePage {
        items,
        total,
        page,
        page_size,
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/system/tenant-packages",
    request_body = PackageUpsert,
    responses((status = 201, description = "已创建", body = PackageVo), (status = 409, description = "套餐标识重复"))
)]
#[require_perm("system:package:add")]
#[audit(module = "system", action = "package:add", kind = "insert")]
pub async fn create_package(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<PackageUpsert>,
) -> Result<(StatusCode, HeaderMap, Json<PackageVo>), ApiErr> {
    let ctx = ctx.into_inner();
    require_platform(&ctx)?;
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    let code = req.code.trim();
    if code.is_empty() || req.name.trim().is_empty() {
        return Err(bad_request("套餐标识与名称不能为空"));
    }
    let taken = TenantPackageEntity::find()
        .filter(TenantPackageColumn::Code.eq(code))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .is_some();
    if taken {
        return Err(conflict("套餐标识已存在"));
    }
    let row = TenantPackageActiveModel {
        code: Set(code.to_string()),
        name: Set(req.name.trim().to_string()),
        user_limit: Set(req.user_limit),
        storage_limit_mb: Set(req.storage_limit_mb),
        api_qps_limit: Set(req.api_qps_limit),
        features: Set(req.features.clone()),
        status: Set(req.status),
        remark: Set(req.remark.clone()),
        create_by: Set(Some(principal.user_name.clone())),
        create_time: Set(chrono::Utc::now()),
        ..Default::default()
    }
    .insert(state.db.conn())
    .await
    .map_err(db_error)?;
    Ok(created(
        format!("/api/v1/system/tenant-packages/{}", row.id),
        PackageVo::from(row),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/system/tenant-packages/{id}",
    request_body = PackageUpsert,
    params(("id" = i64, Path, description = "套餐 id")),
    responses((status = 200, description = "已更新", body = PackageVo))
)]
#[require_perm("system:package:edit")]
#[audit(module = "system", action = "package:edit", kind = "update")]
pub async fn update_package(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(id): Path<i64>,
    Json(req): Json<PackageUpsert>,
) -> Result<Json<PackageVo>, ApiErr> {
    let ctx = ctx.into_inner();
    require_platform(&ctx)?;
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    let code = req.code.trim();
    if code.is_empty() || req.name.trim().is_empty() {
        return Err(bad_request("套餐标识与名称不能为空"));
    }
    // 留痕（审计设计）：套餐限额与状态都是平台侧敏感变更。
    let previous = TenantPackageEntity::find_by_id(id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?;
    let before = previous.as_ref().map(|p| {
        serde_json::json!({
            "code": p.code,
            "name": p.name,
            "user_limit": p.user_limit,
            "storage_limit_mb": p.storage_limit_mb,
            "api_qps_limit": p.api_qps_limit,
            "status": p.status,
            "remark": p.remark,
        })
    });

    let row = TenantPackageActiveModel {
        id: Set(id),
        code: Set(code.to_string()),
        name: Set(req.name.trim().to_string()),
        user_limit: Set(req.user_limit),
        storage_limit_mb: Set(req.storage_limit_mb),
        api_qps_limit: Set(req.api_qps_limit),
        features: Set(req.features.clone()),
        status: Set(req.status),
        remark: Set(req.remark.clone()),
        update_by: Set(Some(principal.user_name.clone())),
        update_time: Set(Some(chrono::Utc::now())),
        ..Default::default()
    }
    .update(state.db.conn())
    .await
    .map_err(|e| match e {
        sea_orm::DbErr::RecordNotUpdated => not_found(),
        other => db_error(other),
    })?;
    if let Some(before) = before {
        crate::audit::record_change(
            &state.audit,
            &ctx,
            "sys_tenant_package",
            id.to_string(),
            before,
            serde_json::json!({
                "code": row.code,
                "name": row.name,
                "user_limit": row.user_limit,
                "storage_limit_mb": row.storage_limit_mb,
                "api_qps_limit": row.api_qps_limit,
                "status": row.status,
                "remark": row.remark,
            }),
        )
        .await;
    }
    Ok(Json(PackageVo::from(row)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/system/tenant-packages/{id}",
    params(("id" = i64, Path, description = "套餐 id")),
    responses((status = 204, description = "已删除"), (status = 409, description = "仍有租户在用量"))
)]
#[require_perm("system:package:remove")]
#[audit(module = "system", action = "package:remove", kind = "delete")]
pub async fn delete_package(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    require_platform(&ctx)?;
    let in_use = TenantEntity::find()
        .filter(TenantColumn::PackageId.eq(id))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .is_some();
    if in_use {
        return Err(conflict("仍有租户绑定该套餐"));
    }
    let res = TenantPackageEntity::delete_by_id(id)
        .exec(state.db.conn())
        .await
        .map_err(db_error)?;
    if res.rows_affected == 0 {
        return Err(not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes(_state: AppState) -> Routes {
    Routes::new()
        .route(
            "/api/v1/system/tenants",
            get(list_tenants),
            list_tenants__perms(),
        )
        .route_audited(
            "/api/v1/system/tenants",
            post(create_tenant),
            create_tenant__perms(),
            create_tenant__audit(),
        )
        .route(
            "/api/v1/system/tenants/{tenant_id}/quota",
            get(get_quota),
            get_quota__perms(),
        )
        .route_audited(
            "/api/v1/system/tenants/{tenant_id}",
            put(update_tenant),
            update_tenant__perms(),
            update_tenant__audit(),
        )
        .route_audited(
            "/api/v1/system/tenants/{tenant_id}",
            delete(delete_tenant),
            delete_tenant__perms(),
            delete_tenant__audit(),
        )
        .route(
            "/api/v1/system/tenant-packages",
            get(list_packages),
            list_packages__perms(),
        )
        .route_audited(
            "/api/v1/system/tenant-packages",
            post(create_package),
            create_package__perms(),
            create_package__audit(),
        )
        .route_audited(
            "/api/v1/system/tenant-packages/{id}",
            put(update_package),
            update_package__perms(),
            update_package__audit(),
        )
        .route_audited(
            "/api/v1/system/tenant-packages/{id}",
            delete(delete_package),
            delete_package__perms(),
            delete_package__audit(),
        )
}
