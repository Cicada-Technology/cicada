//! 字典管理（M2，权限点 `system:dict:*`）：字典类型 + 字典数据。
//!
//! `sys_dict_type` / `sys_dict_data` 是**全局字典**（无 `tenant_id`、无软删），
//! 与参考实现一致不参与租户/数据范围过滤，故直接按主键/`dict_type` 过滤。
//! 删除字典类型前要求其下无字典数据（失败关闭，避免悬挂数据）。

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
use cicada_system::entity::{
    DictDataActiveModel, DictDataColumn, DictDataEntity, DictDataModel, DictTypeActiveModel,
    DictTypeColumn, DictTypeEntity, DictTypeModel,
};

use super::{ApiErr, bad_request, conflict, db_error, not_found, referenced, unauthorized};
use crate::export;
use crate::extractor::RequestContextExt;
use crate::response::created;
use crate::routes::Routes;
use crate::search::Fuzzy;
use crate::sort_select::SelectSortExt;
use crate::state::AppState;

pub const PERMS: &[&str] = &[
    "system:dict:list",
    "system:dict:add",
    "system:dict:edit",
    "system:dict:remove",
    "system:dict:export",
];

// ---------- 字典类型 ----------

#[derive(Debug, Deserialize, IntoParams)]
pub struct DictTypeListQuery {
    pub dict_name: Option<String>,
    pub dict_type: Option<String>,
    pub status: Option<i16>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    /// 排序列名（排序白名单，见 `crate::sort`）；不在清单里则忽略。
    pub sort_by: Option<String>,
    /// `asc` 升序，其余按 `desc`。
    pub sort_order: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DictTypeVo {
    pub dict_id: i64,
    pub dict_name: String,
    pub dict_type: String,
    pub status: i16,
    pub remark: Option<String>,
    pub create_time: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DictTypePage {
    pub items: Vec<DictTypeVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DictTypeUpsert {
    pub dict_name: String,
    pub dict_type: String,
    #[serde(default)]
    pub status: i16,
    pub remark: Option<String>,
}

impl From<DictTypeModel> for DictTypeVo {
    fn from(m: DictTypeModel) -> Self {
        Self {
            dict_id: m.dict_id,
            dict_name: m.dict_name,
            dict_type: m.dict_type,
            status: m.status,
            remark: m.remark,
            create_time: m.create_time.to_rfc3339(),
        }
    }
}

/// `sys_dict_type` 列表允许排序的列（排序白名单）。
///
/// 只放「列表页真的会点排序」的列：每多一个可排序列，就多一条能被利用的
/// `ORDER BY` 路径（大表上非索引列排序会把排序成本推给每一次翻页）。
const DICT_TYPE_SORT: crate::sort::SortSpec = crate::sort::SortSpec::new(&[
    ("dict_name", "sys_dict_type", "dict_name"),
    ("dict_type", "sys_dict_type", "dict_type"),
    ("status", "sys_dict_type", "status"),
]);

#[utoipa::path(
    get,
    path = "/api/v1/system/dict/types",
    params(DictTypeListQuery),
    responses((status = 200, description = "字典类型分页", body = DictTypePage))
)]
#[require_perm("system:dict:list")]
pub async fn list_dict_types(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<DictTypeListQuery>,
) -> Result<Json<DictTypePage>, ApiErr> {
    if ctx.into_inner().principal.is_none() {
        return Err(unauthorized());
    }
    let mut query = DictTypeEntity::find();
    if let Some(name) = q.dict_name.filter(|s| !s.is_empty()) {
        query = query.filter(DictTypeColumn::DictName.fuzzy(&name));
    }
    if let Some(ty) = q.dict_type.filter(|s| !s.is_empty()) {
        query = query.filter(DictTypeColumn::DictType.fuzzy(&ty));
    }
    if let Some(status) = q.status {
        query = query.filter(DictTypeColumn::Status.eq(status));
    }
    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;
    let paginator = query
        // 排序白名单：只认这张表声明过的列，客户端字符串进不了标识符位置。
        // 显式排序必须排在前面才真的生效：默认次序退化成同值行的收尾键（dict_id），
        // 既不改变「不传 sort_by 时的结果」，又保证翻页不重不漏。
        .sorted_by(
            &DICT_TYPE_SORT,
            q.sort_by.as_deref(),
            q.sort_order.as_deref(),
        )
        .order_by_asc(DictTypeColumn::DictId)
        .paginate(state.db.conn(), page_size);
    let total = paginator.num_items().await.map_err(db_error)?;
    let items = paginator
        .fetch_page(page - 1)
        .await
        .map_err(db_error)?
        .into_iter()
        .map(DictTypeVo::from)
        .collect();
    Ok(Json(DictTypePage {
        items,
        total,
        page,
        page_size,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/system/dict/types/{dict_id}",
    params(("dict_id" = i64, Path, description = "字典类型 id")),
    responses((status = 200, description = "字典类型详情", body = DictTypeVo), (status = 404, description = "不存在"))
)]
#[require_perm("system:dict:list")]
pub async fn get_dict_type(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(dict_id): Path<i64>,
) -> Result<Json<DictTypeVo>, ApiErr> {
    if ctx.into_inner().principal.is_none() {
        return Err(unauthorized());
    }
    let row = DictTypeEntity::find_by_id(dict_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    Ok(Json(DictTypeVo::from(row)))
}

#[utoipa::path(
    post,
    path = "/api/v1/system/dict/types",
    request_body = DictTypeUpsert,
    responses((status = 201, description = "已创建", body = DictTypeVo), (status = 409, description = "字典类型重复"))
)]
#[require_perm("system:dict:add")]
#[audit(module = "system", action = "dict:type:add", kind = "insert")]
pub async fn create_dict_type(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<DictTypeUpsert>,
) -> Result<(StatusCode, HeaderMap, Json<DictTypeVo>), ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    if req.dict_name.trim().is_empty() || req.dict_type.trim().is_empty() {
        return Err(bad_request("字典名称与字典类型不能为空"));
    }
    let dup = DictTypeEntity::find()
        .filter(DictTypeColumn::DictType.eq(req.dict_type.trim()))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .is_some();
    if dup {
        return Err(conflict("字典类型已存在"));
    }
    let row = DictTypeActiveModel {
        dict_name: Set(req.dict_name.trim().to_string()),
        dict_type: Set(req.dict_type.trim().to_string()),
        status: Set(req.status),
        remark: Set(req.remark.clone()),
        create_by: Set(Some(principal.user_name.clone())),
        create_time: Set(chrono::Utc::now()),
        ..Default::default()
    }
    .insert(state.db.conn())
    .await
    .map_err(db_error)?;
    // 缓存设计：字典是全局表，任何一条写都要让整域的读重新回源。
    cicada_system::invalidate(&state.sys_cache, cicada_system::CacheDomain::Dict, None).await;
    Ok(created(
        format!("/api/v1/system/dict/types/{}", row.dict_id),
        DictTypeVo::from(row),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/system/dict/types/{dict_id}",
    request_body = DictTypeUpsert,
    responses((status = 200, description = "已更新", body = DictTypeVo), (status = 404, description = "不存在"), (status = 409, description = "字典类型重复"))
)]
#[require_perm("system:dict:edit")]
#[audit(module = "system", action = "dict:type:edit", kind = "update")]
pub async fn update_dict_type(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(dict_id): Path<i64>,
    Json(req): Json<DictTypeUpsert>,
) -> Result<Json<DictTypeVo>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    if req.dict_name.trim().is_empty() || req.dict_type.trim().is_empty() {
        return Err(bad_request("字典名称与字典类型不能为空"));
    }
    let existing = DictTypeEntity::find_by_id(dict_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    let dup = DictTypeEntity::find()
        .filter(DictTypeColumn::DictType.eq(req.dict_type.trim()))
        .filter(DictTypeColumn::DictId.ne(dict_id))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .is_some();
    if dup {
        return Err(conflict("字典类型已存在"));
    }
    // 留痕快照（审计设计）：本接口能改的列。
    let before = serde_json::json!({
        "dict_name": existing.dict_name,
        "dict_type": existing.dict_type,
        "status": existing.status,
        "remark": existing.remark,
    });

    DictTypeActiveModel {
        dict_id: Set(dict_id),
        dict_name: Set(req.dict_name.trim().to_string()),
        dict_type: Set(req.dict_type.trim().to_string()),
        status: Set(req.status),
        remark: Set(req.remark.clone()),
        create_time: Set(existing.create_time),
        update_by: Set(Some(principal.user_name.clone())),
        update_time: Set(Some(chrono::Utc::now())),
        ..Default::default()
    }
    .update(state.db.conn())
    .await
    .map_err(db_error)?;
    let updated = DictTypeEntity::find_by_id(dict_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    crate::audit::record_change(
        &state.audit,
        &ctx,
        "sys_dict_type",
        dict_id.to_string(),
        before,
        serde_json::json!({
            "dict_name": updated.dict_name,
            "dict_type": updated.dict_type,
            "status": updated.status,
            "remark": updated.remark,
        }),
    )
    .await;
    // 缓存设计：字典是全局表，任何一条写都要让整域的读重新回源。
    cicada_system::invalidate(&state.sys_cache, cicada_system::CacheDomain::Dict, None).await;
    Ok(Json(DictTypeVo::from(updated)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/system/dict/types/{dict_id}",
    params(("dict_id" = i64, Path, description = "字典类型 id")),
    responses((status = 204, description = "已删除"), (status = 404, description = "不存在"), (status = 409, description = "存在字典数据"))
)]
#[require_perm("system:dict:remove")]
#[audit(module = "system", action = "dict:type:remove", kind = "delete")]
pub async fn delete_dict_type(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(dict_id): Path<i64>,
) -> Result<StatusCode, ApiErr> {
    if ctx.into_inner().principal.is_none() {
        return Err(unauthorized());
    }
    let row = DictTypeEntity::find_by_id(dict_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    let has_data = DictDataEntity::find()
        .filter(DictDataColumn::DictType.eq(&row.dict_type))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .is_some();
    if has_data {
        return Err(referenced("字典类型下存在字典数据，不能删除"));
    }
    DictTypeEntity::delete_by_id(dict_id)
        .exec(state.db.conn())
        .await
        .map_err(db_error)?;
    // 缓存设计：字典是全局表，任何一条写都要让整域的读重新回源。
    cicada_system::invalidate(&state.sys_cache, cicada_system::CacheDomain::Dict, None).await;
    Ok(StatusCode::NO_CONTENT)
}

// ---------- 字典数据 ----------

#[derive(Debug, Deserialize, IntoParams)]
pub struct DictDataListQuery {
    pub dict_type: Option<String>,
    pub dict_label: Option<String>,
    pub status: Option<i16>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    /// 排序列名（排序白名单，见 `crate::sort`）；不在清单里则忽略。
    pub sort_by: Option<String>,
    /// `asc` 升序，其余按 `desc`。
    pub sort_order: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DictDataVo {
    pub dict_code: i64,
    pub dict_sort: i32,
    pub dict_label: String,
    pub dict_value: String,
    pub dict_type: String,
    pub css_class: Option<String>,
    pub list_class: Option<String>,
    pub is_default: String,
    pub status: i16,
    pub remark: Option<String>,
    pub create_time: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DictDataPage {
    pub items: Vec<DictDataVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DictDataUpsert {
    pub dict_label: String,
    pub dict_value: String,
    pub dict_type: String,
    #[serde(default)]
    pub dict_sort: i32,
    pub css_class: Option<String>,
    pub list_class: Option<String>,
    #[serde(default = "default_is_default")]
    pub is_default: String,
    #[serde(default)]
    pub status: i16,
    pub remark: Option<String>,
}

fn default_is_default() -> String {
    "N".to_string()
}

impl From<DictDataModel> for DictDataVo {
    fn from(m: DictDataModel) -> Self {
        Self {
            dict_code: m.dict_code,
            dict_sort: m.dict_sort,
            dict_label: m.dict_label,
            dict_value: m.dict_value,
            dict_type: m.dict_type,
            css_class: m.css_class,
            list_class: m.list_class,
            is_default: m.is_default,
            status: m.status,
            remark: m.remark,
            create_time: m.create_time.to_rfc3339(),
        }
    }
}

/// `sys_dict_data` 列表允许排序的列（排序白名单）。
///
/// 只放「列表页真的会点排序」的列：每多一个可排序列，就多一条能被利用的
/// `ORDER BY` 路径（大表上非索引列排序会把排序成本推给每一次翻页）。
const DICT_DATA_SORT: crate::sort::SortSpec = crate::sort::SortSpec::new(&[
    ("dict_sort", "sys_dict_data", "dict_sort"),
    ("dict_label", "sys_dict_data", "dict_label"),
    ("dict_value", "sys_dict_data", "dict_value"),
    ("status", "sys_dict_data", "status"),
]);

#[utoipa::path(
    get,
    path = "/api/v1/system/dict/data",
    params(DictDataListQuery),
    responses((status = 200, description = "字典数据分页", body = DictDataPage))
)]
#[require_perm("system:dict:list")]
pub async fn list_dict_data(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<DictDataListQuery>,
) -> Result<Json<DictDataPage>, ApiErr> {
    if ctx.into_inner().principal.is_none() {
        return Err(unauthorized());
    }
    let mut query = DictDataEntity::find();
    if let Some(ty) = q.dict_type.filter(|s| !s.is_empty()) {
        query = query.filter(DictDataColumn::DictType.eq(ty));
    }
    if let Some(label) = q.dict_label.filter(|s| !s.is_empty()) {
        query = query.filter(DictDataColumn::DictLabel.fuzzy(&label));
    }
    if let Some(status) = q.status {
        query = query.filter(DictDataColumn::Status.eq(status));
    }
    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;
    let paginator = query
        // 排序白名单：只认这张表声明过的列，客户端字符串进不了标识符位置。
        // 显式排序必须排在前面才真的生效：默认次序退化成同值行的收尾键（dict_sort → dict_code），
        // 既不改变「不传 sort_by 时的结果」，又保证翻页不重不漏。
        .sorted_by(
            &DICT_DATA_SORT,
            q.sort_by.as_deref(),
            q.sort_order.as_deref(),
        )
        .order_by_asc(DictDataColumn::DictSort)
        .order_by_asc(DictDataColumn::DictCode)
        .paginate(state.db.conn(), page_size);
    let total = paginator.num_items().await.map_err(db_error)?;
    let items = paginator
        .fetch_page(page - 1)
        .await
        .map_err(db_error)?
        .into_iter()
        .map(DictDataVo::from)
        .collect();
    Ok(Json(DictDataPage {
        items,
        total,
        page,
        page_size,
    }))
}

/// 按字典类型取全部启用项。
///
/// **只要求登录，不声明权限点**：字典是渲染表单的参考数据（性别、状态…），任何能打开
/// 某个页面的角色都需要它。若复用 `list_dict_data`（要求 `system:dict:list`），没有字典
/// 管理权限的用户会看到一片空下拉框——前端 `stores/dict.ts` 拉的就是这条路径。
#[utoipa::path(
    get,
    path = "/api/v1/system/dict/data/by-type/{dict_type}",
    params(("dict_type" = String, Path, description = "字典类型")),
    responses((status = 200, description = "该类型的全部启用项（按 sort 升序）", body = [DictDataVo]))
)]
pub async fn list_dict_data_by_type(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(dict_type): Path<String>,
) -> Result<Json<Vec<DictDataVo>>, ApiErr> {
    if ctx.into_inner().principal.is_none() {
        return Err(unauthorized());
    }
    // 只出**启用**项：表单下拉框不该出现被停用的取值，停用与删除在界面上是两种意图，
    // 但都不该被选到。
    //
    // 这条是全站最热的读（每个下拉框、每张列表的标签翻译都要它），所以进缓存。
    // `sys_dict_data` 是全局表（无 `tenant_id`），因此键用全局槽位；真正的失效靠
    // 字典任一写路径自增版本号，TTL 只是兜底。
    let items = cicada_system::cached_json(
        &state.sys_cache,
        cicada_system::CacheDomain::Dict,
        None,
        &dict_type,
        || async {
            Some(
                DictDataEntity::find()
                    .filter(DictDataColumn::DictType.eq(&dict_type))
                    .filter(DictDataColumn::Status.eq(0))
                    .order_by_asc(DictDataColumn::DictSort)
                    .order_by_asc(DictDataColumn::DictCode)
                    .all(state.db.conn())
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(DictDataVo::from)
                    .collect::<Vec<DictDataVo>>(),
            )
        },
    )
    .await
    .unwrap_or_default();
    Ok(Json(items))
}

#[utoipa::path(
    get,
    path = "/api/v1/system/dict/data/{dict_code}",
    params(("dict_code" = i64, Path, description = "字典数据 id")),
    responses((status = 200, description = "字典数据详情", body = DictDataVo), (status = 404, description = "不存在"))
)]
#[require_perm("system:dict:list")]
pub async fn get_dict_data(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(dict_code): Path<i64>,
) -> Result<Json<DictDataVo>, ApiErr> {
    if ctx.into_inner().principal.is_none() {
        return Err(unauthorized());
    }
    let row = DictDataEntity::find_by_id(dict_code)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    Ok(Json(DictDataVo::from(row)))
}

#[utoipa::path(
    post,
    path = "/api/v1/system/dict/data",
    request_body = DictDataUpsert,
    responses((status = 201, description = "已创建", body = DictDataVo))
)]
#[require_perm("system:dict:add")]
#[audit(module = "system", action = "dict:data:add", kind = "insert")]
pub async fn create_dict_data(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<DictDataUpsert>,
) -> Result<(StatusCode, HeaderMap, Json<DictDataVo>), ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    if req.dict_label.trim().is_empty()
        || req.dict_value.trim().is_empty()
        || req.dict_type.trim().is_empty()
    {
        return Err(bad_request("字典标签、键值与字典类型不能为空"));
    }
    let row = DictDataActiveModel {
        dict_sort: Set(req.dict_sort),
        dict_label: Set(req.dict_label.trim().to_string()),
        dict_value: Set(req.dict_value.trim().to_string()),
        dict_type: Set(req.dict_type.trim().to_string()),
        css_class: Set(req.css_class.clone()),
        list_class: Set(req.list_class.clone()),
        is_default: Set(req.is_default.clone()),
        status: Set(req.status),
        remark: Set(req.remark.clone()),
        create_by: Set(Some(principal.user_name.clone())),
        create_time: Set(chrono::Utc::now()),
        ..Default::default()
    }
    .insert(state.db.conn())
    .await
    .map_err(db_error)?;
    // 缓存设计：字典是全局表，任何一条写都要让整域的读重新回源。
    cicada_system::invalidate(&state.sys_cache, cicada_system::CacheDomain::Dict, None).await;
    Ok(created(
        format!("/api/v1/system/dict/data/{}", row.dict_code),
        DictDataVo::from(row),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/system/dict/data/{dict_code}",
    request_body = DictDataUpsert,
    responses((status = 200, description = "已更新", body = DictDataVo), (status = 404, description = "不存在"))
)]
#[require_perm("system:dict:edit")]
#[audit(module = "system", action = "dict:data:edit", kind = "update")]
pub async fn update_dict_data(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(dict_code): Path<i64>,
    Json(req): Json<DictDataUpsert>,
) -> Result<Json<DictDataVo>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    if req.dict_label.trim().is_empty()
        || req.dict_value.trim().is_empty()
        || req.dict_type.trim().is_empty()
    {
        return Err(bad_request("字典标签、键值与字典类型不能为空"));
    }
    let existing = DictDataEntity::find_by_id(dict_code)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    // 留痕快照（审计设计）：本接口能改的列。
    let before = serde_json::json!({
        "dict_sort": existing.dict_sort,
        "dict_label": existing.dict_label,
        "dict_value": existing.dict_value,
        "dict_type": existing.dict_type,
        "css_class": existing.css_class,
        "list_class": existing.list_class,
        "is_default": existing.is_default,
        "status": existing.status,
        "remark": existing.remark,
    });

    DictDataActiveModel {
        dict_code: Set(dict_code),
        dict_sort: Set(req.dict_sort),
        dict_label: Set(req.dict_label.trim().to_string()),
        dict_value: Set(req.dict_value.trim().to_string()),
        dict_type: Set(req.dict_type.trim().to_string()),
        css_class: Set(req.css_class.clone()),
        list_class: Set(req.list_class.clone()),
        is_default: Set(req.is_default.clone()),
        status: Set(req.status),
        remark: Set(req.remark.clone()),
        create_time: Set(existing.create_time),
        update_by: Set(Some(principal.user_name.clone())),
        update_time: Set(Some(chrono::Utc::now())),
        ..Default::default()
    }
    .update(state.db.conn())
    .await
    .map_err(db_error)?;
    let updated = DictDataEntity::find_by_id(dict_code)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    crate::audit::record_change(
        &state.audit,
        &ctx,
        "sys_dict_data",
        dict_code.to_string(),
        before,
        serde_json::json!({
            "dict_sort": updated.dict_sort,
            "dict_label": updated.dict_label,
            "dict_value": updated.dict_value,
            "dict_type": updated.dict_type,
            "css_class": updated.css_class,
            "list_class": updated.list_class,
            "is_default": updated.is_default,
            "status": updated.status,
            "remark": updated.remark,
        }),
    )
    .await;
    // 缓存设计：字典是全局表，任何一条写都要让整域的读重新回源。
    cicada_system::invalidate(&state.sys_cache, cicada_system::CacheDomain::Dict, None).await;
    Ok(Json(DictDataVo::from(updated)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/system/dict/data/{dict_code}",
    params(("dict_code" = i64, Path, description = "字典数据 id")),
    responses((status = 204, description = "已删除"), (status = 404, description = "不存在"))
)]
#[require_perm("system:dict:remove")]
#[audit(module = "system", action = "dict:data:remove", kind = "delete")]
pub async fn delete_dict_data(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(dict_code): Path<i64>,
) -> Result<StatusCode, ApiErr> {
    if ctx.into_inner().principal.is_none() {
        return Err(unauthorized());
    }
    DictDataEntity::find_by_id(dict_code)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    DictDataEntity::delete_by_id(dict_code)
        .exec(state.db.conn())
        .await
        .map_err(db_error)?;
    // 缓存设计：字典是全局表，任何一条写都要让整域的读重新回源。
    cicada_system::invalidate(&state.sys_cache, cicada_system::CacheDomain::Dict, None).await;
    Ok(StatusCode::NO_CONTENT)
}

const TYPE_EXPORT_HEADERS: &[&str] = &[
    "字典主键",
    "字典名称",
    "字典类型",
    "状态",
    "备注",
    "创建时间",
];
const TYPE_EXPORT_WIDTHS: &[f64] = &[10.0, 20.0, 24.0, 8.0, 24.0, 22.0];
const DATA_EXPORT_HEADERS: &[&str] = &[
    "字典编码",
    "字典排序",
    "字典标签",
    "字典键值",
    "字典类型",
    "是否默认",
    "状态",
    "备注",
];
const DATA_EXPORT_WIDTHS: &[f64] = &[10.0, 10.0, 20.0, 20.0, 24.0, 10.0, 8.0, 24.0];

#[utoipa::path(
    get,
    path = "/api/v1/system/dict/types/export",
    params(DictTypeListQuery),
    responses((status = 200, description = "字典类型 xlsx 导出"))
)]
#[require_perm("system:dict:export")]
#[audit(module = "system", action = "dict:type:export", kind = "export")]
pub async fn export_dict_types(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<DictTypeListQuery>,
) -> Result<Response, ApiErr> {
    if ctx.into_inner().principal.is_none() {
        return Err(unauthorized());
    }
    let conn = state.db.conn().clone();
    let name = q.dict_name.filter(|s| !s.is_empty());
    let ty = q.dict_type.filter(|s| !s.is_empty());
    let status = q.status;

    let bytes = export::build(
        "字典类型",
        TYPE_EXPORT_HEADERS,
        TYPE_EXPORT_WIDTHS,
        |sheet| {
            Box::pin(async move {
                let mut row: u32 = 1;
                let mut offset: u64 = 0;
                while u64::from(row) - 1 < export::MAX_EXPORT_ROWS {
                    let mut query = DictTypeEntity::find();
                    if let Some(n) = &name {
                        query = query.filter(DictTypeColumn::DictName.fuzzy(n));
                    }
                    if let Some(t) = &ty {
                        query = query.filter(DictTypeColumn::DictType.fuzzy(t));
                    }
                    if let Some(s) = status {
                        query = query.filter(DictTypeColumn::Status.eq(s));
                    }
                    let batch = query
                        .order_by_asc(DictTypeColumn::DictId)
                        .limit(export::EXPORT_BATCH)
                        .offset(offset)
                        .all(&conn)
                        .await?;
                    if batch.is_empty() {
                        break;
                    }
                    for d in &batch {
                        sheet.write_string(row, 0, d.dict_id.to_string())?;
                        sheet.write_string(row, 1, &d.dict_name)?;
                        sheet.write_string(row, 2, &d.dict_type)?;
                        sheet.write_string(row, 3, if d.status == 0 { "正常" } else { "停用" })?;
                        sheet.write_string(row, 4, d.remark.clone().unwrap_or_default())?;
                        sheet.write_string(row, 5, d.create_time.to_rfc3339())?;
                        row += 1;
                    }
                    offset += batch.len() as u64;
                }
                Ok(())
            })
        },
    )
    .await
    .map_err(db_error)?;

    Ok(export::xlsx_response("dict_types.xlsx", bytes))
}

#[utoipa::path(
    get,
    path = "/api/v1/system/dict/data/export",
    params(DictDataListQuery),
    responses((status = 200, description = "字典数据 xlsx 导出"))
)]
#[require_perm("system:dict:export")]
#[audit(module = "system", action = "dict:data:export", kind = "export")]
pub async fn export_dict_data(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<DictDataListQuery>,
) -> Result<Response, ApiErr> {
    if ctx.into_inner().principal.is_none() {
        return Err(unauthorized());
    }
    let conn = state.db.conn().clone();
    let ty = q.dict_type.filter(|s| !s.is_empty());
    let label = q.dict_label.filter(|s| !s.is_empty());
    let status = q.status;

    let bytes = export::build(
        "字典数据",
        DATA_EXPORT_HEADERS,
        DATA_EXPORT_WIDTHS,
        |sheet| {
            Box::pin(async move {
                let mut row: u32 = 1;
                let mut offset: u64 = 0;
                while u64::from(row) - 1 < export::MAX_EXPORT_ROWS {
                    let mut query = DictDataEntity::find();
                    if let Some(t) = &ty {
                        query = query.filter(DictDataColumn::DictType.eq(t));
                    }
                    if let Some(l) = &label {
                        query = query.filter(DictDataColumn::DictLabel.fuzzy(l));
                    }
                    if let Some(s) = status {
                        query = query.filter(DictDataColumn::Status.eq(s));
                    }
                    let batch = query
                        .order_by_asc(DictDataColumn::DictSort)
                        .order_by_asc(DictDataColumn::DictCode)
                        .limit(export::EXPORT_BATCH)
                        .offset(offset)
                        .all(&conn)
                        .await?;
                    if batch.is_empty() {
                        break;
                    }
                    for d in &batch {
                        sheet.write_string(row, 0, d.dict_code.to_string())?;
                        sheet.write_string(row, 1, d.dict_sort.to_string())?;
                        sheet.write_string(row, 2, &d.dict_label)?;
                        sheet.write_string(row, 3, &d.dict_value)?;
                        sheet.write_string(row, 4, &d.dict_type)?;
                        sheet.write_string(
                            row,
                            5,
                            if d.is_default == "Y" { "是" } else { "否" },
                        )?;
                        sheet.write_string(row, 6, if d.status == 0 { "正常" } else { "停用" })?;
                        sheet.write_string(row, 7, d.remark.clone().unwrap_or_default())?;
                        row += 1;
                    }
                    offset += batch.len() as u64;
                }
                Ok(())
            })
        },
    )
    .await
    .map_err(db_error)?;

    Ok(export::xlsx_response("dict_data.xlsx", bytes))
}

pub fn routes(_state: AppState) -> Routes {
    Routes::new()
        .route(
            "/api/v1/system/dict/types",
            get(list_dict_types),
            list_dict_types__perms(),
        )
        .route_audited(
            "/api/v1/system/dict/types/export",
            get(export_dict_types),
            export_dict_types__perms(),
            export_dict_types__audit(),
        )
        .route(
            "/api/v1/system/dict/types/{dict_id}",
            get(get_dict_type),
            get_dict_type__perms(),
        )
        .route_audited(
            "/api/v1/system/dict/types",
            post(create_dict_type),
            create_dict_type__perms(),
            create_dict_type__audit(),
        )
        .route_audited(
            "/api/v1/system/dict/types/{dict_id}",
            put(update_dict_type),
            update_dict_type__perms(),
            update_dict_type__audit(),
        )
        .route_audited(
            "/api/v1/system/dict/types/{dict_id}",
            delete(delete_dict_type),
            delete_dict_type__perms(),
            delete_dict_type__audit(),
        )
        .route(
            "/api/v1/system/dict/data",
            get(list_dict_data),
            list_dict_data__perms(),
        )
        .route_audited(
            "/api/v1/system/dict/data/export",
            get(export_dict_data),
            export_dict_data__perms(),
            export_dict_data__audit(),
        )
        // 登录即可读（不声明权限点）：表单下拉框的参考数据，见 handler 的说明。
        .route(
            "/api/v1/system/dict/data/by-type/{dict_type}",
            get(list_dict_data_by_type),
            &[],
        )
        .route(
            "/api/v1/system/dict/data/{dict_code}",
            get(get_dict_data),
            get_dict_data__perms(),
        )
        .route_audited(
            "/api/v1/system/dict/data",
            post(create_dict_data),
            create_dict_data__perms(),
            create_dict_data__audit(),
        )
        .route_audited(
            "/api/v1/system/dict/data/{dict_code}",
            put(update_dict_data),
            update_dict_data__perms(),
            update_dict_data__audit(),
        )
        .route_audited(
            "/api/v1/system/dict/data/{dict_code}",
            delete(delete_dict_data),
            delete_dict_data__perms(),
            delete_dict_data__audit(),
        )
}
