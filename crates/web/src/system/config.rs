//! 参数配置管理（M2，权限点 `system:config:*`）。
//!
//! `sys_config` 是全局表（无 `tenant_id`、无软删），`config_key` 唯一。
//! `config_type = 'Y'` 为系统内置参数：禁止删除（与参考实现一致）。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{delete, get, post, put};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use cicada_macros::{audit, require_perm};
use cicada_monitor::cache::{CLEAR_MAX_KEYS, CONFIG_DOMAIN_PREFIX};
use cicada_system::entity::{ConfigActiveModel, ConfigColumn, ConfigEntity, ConfigModel};

use super::{ApiErr, bad_request, conflict, db_error, forbidden, not_found, unauthorized};
use crate::export;
use crate::extractor::RequestContextExt;
use crate::response::created;
use crate::routes::Routes;
use crate::search::Fuzzy;
use crate::sort_select::SelectSortExt;
use crate::state::AppState;

pub const PERMS: &[&str] = &[
    "system:config:list",
    "system:config:add",
    "system:config:edit",
    "system:config:remove",
    "system:config:export",
];

const BUILTIN_TYPE: &str = "Y";

#[derive(Debug, Deserialize, IntoParams)]
pub struct ConfigListQuery {
    pub config_name: Option<String>,
    pub config_key: Option<String>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    /// 排序列名（排序白名单，见 `crate::sort`）；不在清单里则忽略。
    pub sort_by: Option<String>,
    /// `asc` 升序，其余按 `desc`。
    pub sort_order: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ConfigVo {
    pub config_id: i64,
    pub config_name: String,
    pub config_key: String,
    pub config_value: String,
    pub config_type: String,
    pub remark: Option<String>,
    pub create_time: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ConfigPage {
    pub items: Vec<ConfigVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ConfigUpsert {
    pub config_name: String,
    pub config_key: String,
    pub config_value: String,
    #[serde(default = "default_config_type")]
    pub config_type: String,
    pub remark: Option<String>,
}

fn default_config_type() -> String {
    "N".to_string()
}

impl From<ConfigModel> for ConfigVo {
    fn from(m: ConfigModel) -> Self {
        Self {
            config_id: m.config_id,
            config_name: m.config_name,
            config_key: m.config_key,
            config_value: m.config_value,
            config_type: m.config_type,
            remark: m.remark,
            create_time: m.create_time.to_rfc3339(),
        }
    }
}

async fn key_taken(
    db: &sea_orm::DatabaseConnection,
    key: &str,
    exclude: Option<i64>,
) -> Result<bool, ApiErr> {
    let mut q = ConfigEntity::find().filter(ConfigColumn::ConfigKey.eq(key));
    if let Some(id) = exclude {
        q = q.filter(ConfigColumn::ConfigId.ne(id));
    }
    Ok(q.one(db).await.map_err(db_error)?.is_some())
}

/// `sys_config` 列表允许排序的列（排序白名单）。
///
/// 只放「列表页真的会点排序」的列：每多一个可排序列，就多一条能被利用的
/// `ORDER BY` 路径（大表上非索引列排序会把排序成本推给每一次翻页）。
const CONFIG_SORT: crate::sort::SortSpec = crate::sort::SortSpec::new(&[
    ("config_name", "sys_config", "config_name"),
    ("config_key", "sys_config", "config_key"),
    ("config_type", "sys_config", "config_type"),
]);

#[utoipa::path(
    get,
    path = "/api/v1/system/configs",
    params(ConfigListQuery),
    responses((status = 200, description = "参数分页", body = ConfigPage))
)]
#[require_perm("system:config:list")]
pub async fn list_configs(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<ConfigListQuery>,
) -> Result<Json<ConfigPage>, ApiErr> {
    if ctx.into_inner().principal.is_none() {
        return Err(unauthorized());
    }
    let mut query = ConfigEntity::find();
    if let Some(name) = q.config_name.filter(|s| !s.is_empty()) {
        query = query.filter(ConfigColumn::ConfigName.fuzzy(&name));
    }
    if let Some(key) = q.config_key.filter(|s| !s.is_empty()) {
        query = query.filter(ConfigColumn::ConfigKey.fuzzy(&key));
    }
    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;
    let paginator = query
        // 排序白名单：只认这张表声明过的列，客户端字符串进不了标识符位置。
        // 显式排序必须排在前面才真的生效：默认次序退化成同值行的收尾键（config_id），
        // 既不改变「不传 sort_by 时的结果」，又保证翻页不重不漏。
        .sorted_by(&CONFIG_SORT, q.sort_by.as_deref(), q.sort_order.as_deref())
        .order_by_asc(ConfigColumn::ConfigId)
        .paginate(state.db.conn(), page_size);
    let total = paginator.num_items().await.map_err(db_error)?;
    let items = paginator
        .fetch_page(page - 1)
        .await
        .map_err(db_error)?
        .into_iter()
        .map(ConfigVo::from)
        .collect();
    Ok(Json(ConfigPage {
        items,
        total,
        page,
        page_size,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/system/configs/{config_id}",
    params(("config_id" = i64, Path, description = "参数 id")),
    responses((status = 200, description = "参数详情", body = ConfigVo), (status = 404, description = "不存在"))
)]
#[require_perm("system:config:list")]
pub async fn get_config(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(config_id): Path<i64>,
) -> Result<Json<ConfigVo>, ApiErr> {
    if ctx.into_inner().principal.is_none() {
        return Err(unauthorized());
    }
    let row = ConfigEntity::find_by_id(config_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    Ok(Json(ConfigVo::from(row)))
}

#[utoipa::path(
    post,
    path = "/api/v1/system/configs",
    request_body = ConfigUpsert,
    responses((status = 201, description = "已创建", body = ConfigVo), (status = 409, description = "参数键名重复"))
)]
#[require_perm("system:config:add")]
#[audit(module = "system", action = "config:add", kind = "insert")]
pub async fn create_config(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<ConfigUpsert>,
) -> Result<(StatusCode, HeaderMap, Json<ConfigVo>), ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    if req.config_name.trim().is_empty() || req.config_key.trim().is_empty() {
        return Err(bad_request("参数名称与参数键名不能为空"));
    }
    if key_taken(state.db.conn(), req.config_key.trim(), None).await? {
        return Err(conflict("参数键名已存在"));
    }
    let row = ConfigActiveModel {
        config_name: Set(req.config_name.trim().to_string()),
        config_key: Set(req.config_key.trim().to_string()),
        config_value: Set(req.config_value.clone()),
        config_type: Set(req.config_type.clone()),
        remark: Set(req.remark.clone()),
        create_by: Set(Some(principal.user_name.clone())),
        create_time: Set(chrono::Utc::now()),
        ..Default::default()
    }
    .insert(state.db.conn())
    .await
    .map_err(db_error)?;
    Ok(created(
        format!("/api/v1/system/configs/{}", row.config_id),
        ConfigVo::from(row),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/system/configs/{config_id}",
    request_body = ConfigUpsert,
    responses((status = 200, description = "已更新", body = ConfigVo), (status = 404, description = "不存在"), (status = 409, description = "参数键名重复"))
)]
#[require_perm("system:config:edit")]
#[audit(module = "system", action = "config:edit", kind = "update")]
pub async fn update_config(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(config_id): Path<i64>,
    Json(req): Json<ConfigUpsert>,
) -> Result<Json<ConfigVo>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    if req.config_name.trim().is_empty() || req.config_key.trim().is_empty() {
        return Err(bad_request("参数名称与参数键名不能为空"));
    }
    let existing = ConfigEntity::find_by_id(config_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    if key_taken(state.db.conn(), req.config_key.trim(), Some(config_id)).await? {
        return Err(conflict("参数键名已存在"));
    }
    // 留痕快照（审计设计）：本接口能改的列。
    let before = serde_json::json!({
        "config_name": existing.config_name,
        "config_key": existing.config_key,
        "config_value": existing.config_value,
        "config_type": existing.config_type,
        "remark": existing.remark,
    });

    ConfigActiveModel {
        config_id: Set(config_id),
        config_name: Set(req.config_name.trim().to_string()),
        config_key: Set(req.config_key.trim().to_string()),
        config_value: Set(req.config_value.clone()),
        config_type: Set(req.config_type.clone()),
        remark: Set(req.remark.clone()),
        create_time: Set(existing.create_time),
        update_by: Set(Some(principal.user_name.clone())),
        update_time: Set(Some(chrono::Utc::now())),
        ..Default::default()
    }
    .update(state.db.conn())
    .await
    .map_err(db_error)?;
    let updated = ConfigEntity::find_by_id(config_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    crate::audit::record_change(
        &state.audit,
        &ctx,
        "sys_config",
        config_id.to_string(),
        before,
        serde_json::json!({
            "config_name": updated.config_name,
            "config_key": updated.config_key,
            "config_value": updated.config_value,
            "config_type": updated.config_type,
            "remark": updated.remark,
        }),
    )
    .await;
    Ok(Json(ConfigVo::from(updated)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/system/configs/{config_id}",
    params(("config_id" = i64, Path, description = "参数 id")),
    responses((status = 204, description = "已删除"), (status = 403, description = "内置参数不可删"), (status = 404, description = "不存在"))
)]
#[require_perm("system:config:remove")]
#[audit(module = "system", action = "config:remove", kind = "delete")]
pub async fn delete_config(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(config_id): Path<i64>,
) -> Result<StatusCode, ApiErr> {
    if ctx.into_inner().principal.is_none() {
        return Err(unauthorized());
    }
    let row = ConfigEntity::find_by_id(config_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    if row.config_type == BUILTIN_TYPE {
        return Err(forbidden("系统内置参数不可删除"));
    }
    ConfigEntity::delete_by_id(config_id)
        .exec(state.db.conn())
        .await
        .map_err(db_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RefreshCacheResult {
    /// 实际清掉的缓存键数。
    pub removed: u64,
}

#[utoipa::path(
    post,
    path = "/api/v1/system/configs/refresh-cache",
    responses(
        (status = 200, description = "已刷新参数缓存", body = RefreshCacheResult),
        (status = 503, description = "缓存后端不可用")
    )
)]
#[require_perm("system:config:remove")]
#[audit(module = "system", action = "config:refreshCache", kind = "update")]
pub async fn refresh_config_cache(
    State(state): State<AppState>,
    ctx: RequestContextExt,
) -> Result<Json<RefreshCacheResult>, ApiErr> {
    if ctx.into_inner().principal.is_none() {
        return Err(unauthorized());
    }
    // 按缓存的「版本号失效」整域清空：清掉 `cicada:config*` 后下一次读取会从
    // `sys_config` 重建，无需逐键失效（也就不会漏键）。
    let removed = state
        .cache_inspector
        .clear_prefix(CONFIG_DOMAIN_PREFIX, CLEAR_MAX_KEYS)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "刷新参数缓存失败");
            crate::problem::Problem::new(
                crate::problem::BizCode::CacheUnavailable,
                "缓存后端不可用".to_string(),
            )
        })?;
    tracing::info!(removed, "参数缓存已刷新");
    Ok(Json(RefreshCacheResult { removed }))
}

const EXPORT_HEADERS: &[&str] = &[
    "参数编号",
    "参数名称",
    "参数键名",
    "参数键值",
    "系统内置",
    "备注",
    "创建时间",
];
const EXPORT_WIDTHS: &[f64] = &[10.0, 20.0, 26.0, 26.0, 10.0, 24.0, 22.0];

#[utoipa::path(
    get,
    path = "/api/v1/system/configs/export",
    params(ConfigListQuery),
    responses((status = 200, description = "参数 xlsx 导出"))
)]
#[require_perm("system:config:export")]
#[audit(module = "system", action = "config:export", kind = "export")]
pub async fn export_configs(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<ConfigListQuery>,
) -> Result<Response, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let conn = state.db.conn().clone();
    let name = q.config_name.filter(|s| !s.is_empty());
    let key = q.config_key.filter(|s| !s.is_empty());

    let bytes = export::build("参数", EXPORT_HEADERS, EXPORT_WIDTHS, |sheet| {
        Box::pin(async move {
            let mut row: u32 = 1;
            let mut offset: u64 = 0;
            while u64::from(row) - 1 < export::MAX_EXPORT_ROWS {
                let mut query = ConfigEntity::find();
                if let Some(n) = &name {
                    query = query.filter(ConfigColumn::ConfigName.fuzzy(n));
                }
                if let Some(k) = &key {
                    query = query.filter(ConfigColumn::ConfigKey.fuzzy(k));
                }
                let batch = query
                    .order_by_asc(ConfigColumn::ConfigId)
                    .limit(export::EXPORT_BATCH)
                    .offset(offset)
                    .all(&conn)
                    .await?;
                if batch.is_empty() {
                    break;
                }
                for c in &batch {
                    sheet.write_string(row, 0, c.config_id.to_string())?;
                    sheet.write_string(row, 1, &c.config_name)?;
                    sheet.write_string(row, 2, &c.config_key)?;
                    sheet.write_string(row, 3, &c.config_value)?;
                    sheet.write_string(
                        row,
                        4,
                        if c.config_type == BUILTIN_TYPE {
                            "是"
                        } else {
                            "否"
                        },
                    )?;
                    sheet.write_string(row, 5, c.remark.clone().unwrap_or_default())?;
                    sheet.write_string(row, 6, c.create_time.to_rfc3339())?;
                    row += 1;
                }
                offset += batch.len() as u64;
            }
            Ok(())
        })
    })
    .await
    .map_err(db_error)?;

    Ok(export::xlsx_response("configs.xlsx", bytes))
}

pub fn routes(_state: AppState) -> Routes {
    Routes::new()
        .route(
            "/api/v1/system/configs",
            get(list_configs),
            list_configs__perms(),
        )
        .route_audited(
            "/api/v1/system/configs/export",
            get(export_configs),
            export_configs__perms(),
            export_configs__audit(),
        )
        .route(
            "/api/v1/system/configs/{config_id}",
            get(get_config),
            get_config__perms(),
        )
        .route_audited(
            "/api/v1/system/configs",
            post(create_config),
            create_config__perms(),
            create_config__audit(),
        )
        .route_audited(
            "/api/v1/system/configs/{config_id}",
            put(update_config),
            update_config__perms(),
            update_config__audit(),
        )
        .route_audited(
            "/api/v1/system/configs/{config_id}",
            delete(delete_config),
            delete_config__perms(),
            delete_config__audit(),
        )
        .route_audited(
            "/api/v1/system/configs/refresh-cache",
            post(refresh_config_cache),
            refresh_config_cache__perms(),
            refresh_config_cache__audit(),
        )
}
