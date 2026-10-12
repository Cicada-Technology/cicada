//! 开放接口凭证：`system:apikey:*`。
//!
//! 明文密钥只在签发时返回一次；列表只暴露 `key_prefix`。凭证主体是**租户级**的：
//! 数据范围固定为「租户内全部」，`user_id = 0`（不是自然人账号，见多租户设计）。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post, put};
use sea_orm::ColumnTrait;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use cicada_kernel::scope::Query as ScopedQuery;
use cicada_macros::{audit, require_perm};
use cicada_system::entity::{ApiKeyColumn, ApiKeyEntity, ApiKeyModel};
use cicada_system::{API_KEY_ACTIVE, API_KEY_DISABLED};

use super::{ApiErr, db_error, field_error, not_found, unauthorized};
use crate::extractor::RequestContextExt;
use crate::response::created;
use crate::routes::Routes;
use crate::search::Fuzzy;
use crate::sort::SortExt;
use crate::state::AppState;

pub const PERMS: &[&str] = &[
    "system:apikey:list",
    "system:apikey:add",
    "system:apikey:edit",
    "system:apikey:remove",
];

#[derive(Debug, Deserialize, IntoParams)]
pub struct ApiKeyListQuery {
    pub name: Option<String>,
    pub status: Option<i16>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    /// 排序列名（排序白名单，见 `crate::sort`）；不在清单里则忽略。
    pub sort_by: Option<String>,
    /// `asc` 升序，其余按 `desc`。
    pub sort_order: Option<String>,
}

/// 凭证视图：**不含** `secret_hash`，也不含明文。
#[derive(Debug, Serialize, ToSchema)]
pub struct ApiKeyVo {
    pub id: i64,
    pub tenant_id: i64,
    pub name: String,
    pub key_prefix: String,
    pub scopes: Vec<String>,
    pub status: i16,
    pub expires_at: Option<String>,
    pub last_used_at: Option<String>,
    pub create_by: Option<String>,
    pub create_time: String,
}

/// 签发响应：`secret` 只在这里出现一次。
#[derive(Debug, Serialize, ToSchema)]
pub struct ApiKeyIssuedVo {
    #[serde(flatten)]
    pub key: ApiKeyVo,
    pub secret: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApiKeyPage {
    pub items: Vec<ApiKeyVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ApiKeyCreate {
    pub name: String,
    /// 权限标识列表（精确匹配）。空列表 = 只有认证、没有任何接口权限。
    pub scopes: Vec<String>,
    /// RFC3339 过期时间，缺省不过期。
    pub expires_at: Option<String>,
    pub remark: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ApiKeyStatusUpdate {
    /// 0 生效 / 1 停用。
    pub status: i16,
}

impl From<ApiKeyModel> for ApiKeyVo {
    fn from(m: ApiKeyModel) -> Self {
        let scopes = cicada_system::ApiKeyService::scopes_of(&m);
        Self {
            id: m.id,
            tenant_id: m.tenant_id,
            name: m.name,
            key_prefix: m.key_prefix,
            scopes,
            status: m.status,
            expires_at: m.expires_at.map(|t| t.to_rfc3339()),
            last_used_at: m.last_used_at.map(|t| t.to_rfc3339()),
            create_by: m.create_by,
            create_time: m.create_time.to_rfc3339(),
        }
    }
}

fn parse_expiry(raw: &Option<String>) -> Result<Option<chrono::DateTime<chrono::Utc>>, ApiErr> {
    let Some(raw) = raw.as_deref().filter(|s| !s.trim().is_empty()) else {
        return Ok(None);
    };
    let parsed = chrono::DateTime::parse_from_rfc3339(raw.trim())
        .map_err(|_| field_error("expires_at", "需为 RFC3339 时间"))?;
    Ok(Some(parsed.with_timezone(&chrono::Utc)))
}

/// 权限标识形如 `模块:资源:操作`（权限点清单），只允许字母数字下划线与冒号。
fn valid_scope(s: &str) -> bool {
    !s.is_empty()
        && s.split(':').count() >= 2
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b':')
}

/// `sys_api_key` 列表允许排序的列（排序白名单）。
///
/// 只放「列表页真的会点排序」的列：每多一个可排序列，就多一条能被利用的
/// `ORDER BY` 路径（大表上非索引列排序会把排序成本推给每一次翻页）。
const API_KEY_SORT: crate::sort::SortSpec = crate::sort::SortSpec::new(&[
    ("name", "sys_api_key", "name"),
    ("key_prefix", "sys_api_key", "key_prefix"),
    ("status", "sys_api_key", "status"),
    // 库里真实列是 `expires_at`；`expire_at` 只是对外别名，两者都落到同一个常量列。
    ("expire_at", "sys_api_key", "expires_at"),
    ("expires_at", "sys_api_key", "expires_at"),
    ("create_time", "sys_api_key", "create_time"),
]);

#[utoipa::path(
    get,
    path = "/api/v1/system/api-keys",
    params(ApiKeyListQuery),
    responses((status = 200, description = "凭证分页", body = ApiKeyPage))
)]
#[require_perm("system:apikey:list")]
pub async fn list_api_keys(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<ApiKeyListQuery>,
) -> Result<Json<ApiKeyPage>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let mut query = ScopedQuery::<ApiKeyEntity>::open().scoped(&ctx.scope());
    if let Some(name) = q.name.filter(|s| !s.is_empty()) {
        query = query.filter(ApiKeyColumn::Name.fuzzy(&name));
    }
    if let Some(status) = q.status {
        query = query.filter(ApiKeyColumn::Status.eq(status));
    }
    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;
    let paged = query
        // 排序白名单：只认这张表声明过的列，客户端字符串进不了标识符位置。
        // 显式排序必须排在前面才真的生效：默认次序退化成同值行的收尾键（id），
        // 既不改变「不传 sort_by 时的结果」，又保证翻页不重不漏。
        .sorted_by(&API_KEY_SORT, q.sort_by.as_deref(), q.sort_order.as_deref())
        .order_by(ApiKeyColumn::Id)
        .fetch_page(state.db.conn(), page, page_size)
        .await
        .map_err(db_error)?;
    Ok(Json(ApiKeyPage {
        items: paged.items.into_iter().map(ApiKeyVo::from).collect(),
        total: paged.total,
        page: paged.page,
        page_size: paged.page_size,
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/system/api-keys",
    request_body = ApiKeyCreate,
    responses((status = 201, description = "已签发（secret 只返回一次）", body = ApiKeyIssuedVo))
)]
#[require_perm("system:apikey:add")]
#[audit(module = "system", action = "apikey:add", kind = "insert")]
pub async fn create_api_key(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<ApiKeyCreate>,
) -> Result<(StatusCode, HeaderMap, Json<ApiKeyIssuedVo>), ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    let name = req.name.trim();
    if name.is_empty() {
        return Err(field_error("name", "凭证名称不能为空"));
    }
    if let Some(bad) = req.scopes.iter().find(|s| !valid_scope(s)) {
        return Err(field_error("scopes", format!("权限标识非法：{bad}")));
    }
    let expires_at = parse_expiry(&req.expires_at)?;

    let issued = state
        .api_keys
        .issue(
            ctx.tenant_id,
            name,
            req.scopes.clone(),
            expires_at,
            Some(principal.user_name.clone()),
        )
        .await
        .map_err(db_error)?;

    let id = issued.model.id;
    Ok(created(
        format!("/api/v1/system/api-keys/{id}"),
        ApiKeyIssuedVo {
            key: ApiKeyVo::from(issued.model),
            secret: issued.secret,
        },
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/system/api-keys/{id}/status",
    request_body = ApiKeyStatusUpdate,
    params(("id" = i64, Path, description = "凭证 id")),
    responses((status = 204, description = "已更新"), (status = 404, description = "不存在"))
)]
#[require_perm("system:apikey:edit")]
#[audit(module = "system", action = "apikey:status", kind = "update")]
pub async fn update_api_key_status(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(id): Path<i64>,
    Json(req): Json<ApiKeyStatusUpdate>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    if req.status != API_KEY_ACTIVE && req.status != API_KEY_DISABLED {
        return Err(field_error("status", "status 只能是 0（生效）或 1（停用）"));
    }
    let updated = state
        .api_keys
        .set_status(ctx.tenant_id, id, req.status)
        .await
        .map_err(db_error)?;
    if !updated {
        return Err(not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/api/v1/system/api-keys/{id}",
    params(("id" = i64, Path, description = "凭证 id")),
    responses((status = 204, description = "已删除"), (status = 404, description = "不存在"))
)]
#[require_perm("system:apikey:remove")]
#[audit(module = "system", action = "apikey:remove", kind = "delete")]
pub async fn delete_api_key(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let removed = state
        .api_keys
        .revoke(ctx.tenant_id, id)
        .await
        .map_err(db_error)?;
    if !removed {
        return Err(not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes(_state: AppState) -> Routes {
    Routes::new()
        .route(
            "/api/v1/system/api-keys",
            get(list_api_keys),
            list_api_keys__perms(),
        )
        .route_audited(
            "/api/v1/system/api-keys",
            post(create_api_key),
            create_api_key__perms(),
            create_api_key__audit(),
        )
        .route_audited(
            "/api/v1/system/api-keys/{id}/status",
            put(update_api_key_status),
            update_api_key_status__perms(),
            update_api_key_status__audit(),
        )
        .route_audited(
            "/api/v1/system/api-keys/{id}",
            delete(delete_api_key),
            delete_api_key__perms(),
            delete_api_key__audit(),
        )
}
