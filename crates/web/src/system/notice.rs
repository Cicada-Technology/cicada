//! 通知公告管理（M2，权限点 `system:notice:*`）。
//!
//! `sys_notice` 是全局表（无 `tenant_id`、无软删）。`sys_notice_read` 记录
//! 某用户对某公告的已读状态（复合主键 `(notice_id, user_id)`），已读接口复用
//! `system:notice:list` 权限点。

use std::collections::{HashMap, HashSet};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post, put};
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use cicada_macros::{audit, require_perm};
use cicada_system::entity::{
    NoticeActiveModel, NoticeColumn, NoticeEntity, NoticeModel, NoticeReadActiveModel,
    NoticeReadColumn, NoticeReadEntity, UserColumn, UserEntity,
};

use super::{ApiErr, bad_request, db_error, not_found, unauthorized};
use crate::extractor::RequestContextExt;
use crate::response::created;
use crate::routes::Routes;
use crate::search::Fuzzy;
use crate::sort_select::SelectSortExt;
use crate::state::AppState;

pub const PERMS: &[&str] = &[
    "system:notice:list",
    "system:notice:add",
    "system:notice:edit",
    "system:notice:remove",
];

#[derive(Debug, Deserialize, IntoParams)]
pub struct NoticeListQuery {
    pub notice_title: Option<String>,
    pub notice_type: Option<i16>,
    pub status: Option<i16>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    /// 排序列名（排序白名单，见 `crate::sort`）；不在清单里则忽略。
    pub sort_by: Option<String>,
    /// `asc` 升序，其余按 `desc`。
    pub sort_order: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NoticeVo {
    pub notice_id: i64,
    pub notice_title: String,
    pub notice_type: i16,
    pub notice_content: Option<String>,
    pub status: i16,
    pub remark: Option<String>,
    pub create_time: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NoticePage {
    pub items: Vec<NoticeVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct NoticeUpsert {
    pub notice_title: String,
    #[serde(default = "default_notice_type")]
    pub notice_type: i16,
    pub notice_content: Option<String>,
    #[serde(default)]
    pub status: i16,
    pub remark: Option<String>,
}

fn default_notice_type() -> i16 {
    1
}

impl From<NoticeModel> for NoticeVo {
    fn from(m: NoticeModel) -> Self {
        Self {
            notice_id: m.notice_id,
            notice_title: m.notice_title,
            notice_type: m.notice_type,
            notice_content: m.notice_content,
            status: m.status,
            remark: m.remark,
            create_time: m.create_time.to_rfc3339(),
        }
    }
}

/// `sys_notice` 列表允许排序的列（排序白名单）。
///
/// 只放「列表页真的会点排序」的列：每多一个可排序列，就多一条能被利用的
/// `ORDER BY` 路径（大表上非索引列排序会把排序成本推给每一次翻页）。
const NOTICE_SORT: crate::sort::SortSpec = crate::sort::SortSpec::new(&[
    ("notice_title", "sys_notice", "notice_title"),
    ("notice_type", "sys_notice", "notice_type"),
    ("status", "sys_notice", "status"),
    ("create_time", "sys_notice", "create_time"),
]);

#[utoipa::path(
    get,
    path = "/api/v1/system/notices",
    params(NoticeListQuery),
    responses((status = 200, description = "公告分页", body = NoticePage))
)]
#[require_perm("system:notice:list")]
pub async fn list_notices(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<NoticeListQuery>,
) -> Result<Json<NoticePage>, ApiErr> {
    if ctx.into_inner().principal.is_none() {
        return Err(unauthorized());
    }
    let mut query = NoticeEntity::find();
    if let Some(title) = q.notice_title.filter(|s| !s.is_empty()) {
        query = query.filter(NoticeColumn::NoticeTitle.fuzzy(&title));
    }
    if let Some(ty) = q.notice_type {
        query = query.filter(NoticeColumn::NoticeType.eq(ty));
    }
    if let Some(status) = q.status {
        query = query.filter(NoticeColumn::Status.eq(status));
    }
    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;
    let paginator = query
        // 排序白名单：只认这张表声明过的列，客户端字符串进不了标识符位置。
        // 显式排序必须排在前面才真的生效：默认次序退化成同值行的收尾键（notice_id），
        // 既不改变「不传 sort_by 时的结果」，又保证翻页不重不漏。
        .sorted_by(&NOTICE_SORT, q.sort_by.as_deref(), q.sort_order.as_deref())
        .order_by_desc(NoticeColumn::NoticeId)
        .paginate(state.db.conn(), page_size);
    let total = paginator.num_items().await.map_err(db_error)?;
    let items = paginator
        .fetch_page(page - 1)
        .await
        .map_err(db_error)?
        .into_iter()
        .map(NoticeVo::from)
        .collect();
    Ok(Json(NoticePage {
        items,
        total,
        page,
        page_size,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/system/notices/{notice_id}",
    params(("notice_id" = i64, Path, description = "公告 id")),
    responses((status = 200, description = "公告详情", body = NoticeVo), (status = 404, description = "不存在"))
)]
#[require_perm("system:notice:list")]
pub async fn get_notice(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(notice_id): Path<i64>,
) -> Result<Json<NoticeVo>, ApiErr> {
    if ctx.into_inner().principal.is_none() {
        return Err(unauthorized());
    }
    let row = NoticeEntity::find_by_id(notice_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    Ok(Json(NoticeVo::from(row)))
}

#[utoipa::path(
    post,
    path = "/api/v1/system/notices",
    request_body = NoticeUpsert,
    responses((status = 201, description = "已创建", body = NoticeVo))
)]
#[require_perm("system:notice:add")]
#[audit(module = "system", action = "notice:add", kind = "insert")]
pub async fn create_notice(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<NoticeUpsert>,
) -> Result<(StatusCode, HeaderMap, Json<NoticeVo>), ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    if req.notice_title.trim().is_empty() {
        return Err(bad_request("公告标题不能为空"));
    }
    let row = NoticeActiveModel {
        notice_title: Set(req.notice_title.trim().to_string()),
        notice_type: Set(req.notice_type),
        // 内容安全：富文本入库前先过白名单（`cicada-system` 是这张表的属主，清洗规则跟着它）。
        notice_content: Set(req
            .notice_content
            .as_deref()
            .map(cicada_system::sanitize_html)),
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
        format!("/api/v1/system/notices/{}", row.notice_id),
        NoticeVo::from(row),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/system/notices/{notice_id}",
    request_body = NoticeUpsert,
    responses((status = 200, description = "已更新", body = NoticeVo), (status = 404, description = "不存在"))
)]
#[require_perm("system:notice:edit")]
#[audit(module = "system", action = "notice:edit", kind = "update")]
pub async fn update_notice(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(notice_id): Path<i64>,
    Json(req): Json<NoticeUpsert>,
) -> Result<Json<NoticeVo>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    if req.notice_title.trim().is_empty() {
        return Err(bad_request("公告标题不能为空"));
    }
    let existing = NoticeEntity::find_by_id(notice_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    // 留痕快照（审计设计）：本接口能改的列。
    let before = serde_json::json!({
        "notice_title": existing.notice_title,
        "notice_type": existing.notice_type,
        "notice_content": existing.notice_content,
        "status": existing.status,
        "remark": existing.remark,
    });

    NoticeActiveModel {
        notice_id: Set(notice_id),
        notice_title: Set(req.notice_title.trim().to_string()),
        notice_type: Set(req.notice_type),
        notice_content: Set(req
            .notice_content
            .as_deref()
            .map(cicada_system::sanitize_html)),
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
    let updated = NoticeEntity::find_by_id(notice_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    crate::audit::record_change(
        &state.audit,
        &ctx,
        "sys_notice",
        notice_id.to_string(),
        before,
        serde_json::json!({
            "notice_title": updated.notice_title,
            "notice_type": updated.notice_type,
            "notice_content": updated.notice_content,
            "status": updated.status,
            "remark": updated.remark,
        }),
    )
    .await;
    Ok(Json(NoticeVo::from(updated)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/system/notices/{notice_id}",
    params(("notice_id" = i64, Path, description = "公告 id")),
    responses((status = 204, description = "已删除"), (status = 404, description = "不存在"))
)]
#[require_perm("system:notice:remove")]
#[audit(module = "system", action = "notice:remove", kind = "delete")]
pub async fn delete_notice(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(notice_id): Path<i64>,
) -> Result<StatusCode, ApiErr> {
    if ctx.into_inner().principal.is_none() {
        return Err(unauthorized());
    }
    NoticeEntity::find_by_id(notice_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    NoticeEntity::delete_by_id(notice_id)
        .exec(state.db.conn())
        .await
        .map_err(db_error)?;
    NoticeReadEntity::delete_many()
        .filter(NoticeReadColumn::NoticeId.eq(notice_id))
        .exec(state.db.conn())
        .await
        .map_err(db_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/system/notices/{notice_id}/read",
    params(("notice_id" = i64, Path, description = "公告 id")),
    responses((status = 204, description = "已标记为已读"), (status = 404, description = "公告不存在"))
)]
#[require_perm("system:notice:list")]
pub async fn mark_notice_read(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(notice_id): Path<i64>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    NoticeEntity::find_by_id(notice_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    let user_id: i64 = principal.user_id.get();
    // 幂等：已读则直接返回；未读则写入，冲突（并发重复点击）忽略。
    let already = NoticeReadEntity::find()
        .filter(NoticeReadColumn::NoticeId.eq(notice_id))
        .filter(NoticeReadColumn::UserId.eq(user_id))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .is_some();
    if !already {
        NoticeReadEntity::insert(NoticeReadActiveModel {
            notice_id: Set(notice_id),
            user_id: Set(user_id),
        })
        .on_conflict(
            OnConflict::columns([NoticeReadColumn::NoticeId, NoticeReadColumn::UserId])
                .do_nothing()
                .to_owned(),
        )
        .exec_without_returning(state.db.conn())
        .await
        .map_err(db_error)?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// 已读名单（接口清单的 `notices/{id}/readers`，`system:notice:list`）。
#[derive(Debug, Serialize, ToSchema)]
pub struct NoticeReaderVo {
    pub user_id: i64,
    pub user_name: String,
    pub nick_name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NoticeReaderPage {
    pub items: Vec<NoticeReaderVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

#[utoipa::path(
    get,
    path = "/api/v1/system/notices/{notice_id}/readers",
    params(
        ("notice_id" = i64, Path, description = "公告 id"),
        ("page" = Option<u64>, Query),
        ("page_size" = Option<u64>, Query)
    ),
    responses((status = 200, description = "已读名单分页", body = NoticeReaderPage), (status = 404, description = "公告不存在"))
)]
#[require_perm("system:notice:list")]
pub async fn list_notice_readers(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(notice_id): Path<i64>,
    Query(q): Query<NoticeListQuery>,
) -> Result<Json<NoticeReaderPage>, ApiErr> {
    if ctx.into_inner().principal.is_none() {
        return Err(unauthorized());
    }
    NoticeEntity::find_by_id(notice_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;

    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;
    let reads = NoticeReadEntity::find()
        .filter(NoticeReadColumn::NoticeId.eq(notice_id))
        .order_by_asc(NoticeReadColumn::UserId)
        .paginate(state.db.conn(), page_size);
    let total = reads.num_items().await.map_err(db_error)?;
    let ids: Vec<i64> = reads
        .fetch_page(page - 1)
        .await
        .map_err(db_error)?
        .into_iter()
        .map(|r| r.user_id)
        .collect();
    // 用户名从 `sys_user` 取（已读记录只有 id）；软删用户也照样显示，
    // 「谁读过这条公告」是历史事实，不能因为账号被删就从名单里消失。
    let names = UserEntity::find()
        .filter(UserColumn::UserId.is_in(ids.clone()))
        .all(state.db.conn())
        .await
        .map_err(db_error)?
        .into_iter()
        .map(|u| (u.user_id, u))
        .collect::<HashMap<_, _>>();
    let items = ids
        .into_iter()
        .filter_map(|id| {
            names.get(&id).map(|u| NoticeReaderVo {
                user_id: id,
                user_name: u.user_name.clone(),
                nick_name: u.nick_name.clone(),
            })
        })
        .collect();
    Ok(Json(NoticeReaderPage {
        items,
        total,
        page,
        page_size,
    }))
}

/// 顶栏公告（接口清单的 `notices/top`，仅需登录）。
///
/// 不给权限点：这是「登录后看到有几条公告」的个人视图，跟 `system:notice:list`
/// （管理页全量列表）是两件事。带当前用户的已读标记，前端据此显示红点。
#[derive(Debug, Deserialize, IntoParams)]
pub struct TopQuery {
    /// 条数，默认 5，上限 20。
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TopNoticeVo {
    pub notice_id: i64,
    pub notice_title: String,
    pub notice_type: i16,
    pub create_time: String,
    /// 当前用户是否已读。
    pub read: bool,
}

#[utoipa::path(
    get,
    path = "/api/v1/system/notices/top",
    params(TopQuery),
    responses((status = 200, description = "最新公告", body = [TopNoticeVo]))
)]
pub async fn top_notices(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<TopQuery>,
) -> Result<Json<Vec<TopNoticeVo>>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    let limit = q.limit.unwrap_or(5).clamp(1, 20);
    let rows = NoticeEntity::find()
        .filter(NoticeColumn::Status.eq(0))
        .order_by_desc(NoticeColumn::NoticeId)
        .limit(limit as u64)
        .all(state.db.conn())
        .await
        .map_err(db_error)?;
    let mine = NoticeReadEntity::find()
        .filter(NoticeReadColumn::UserId.eq(principal.user_id.get()))
        .all(state.db.conn())
        .await
        .map_err(db_error)?
        .into_iter()
        .map(|r| r.notice_id)
        .collect::<HashSet<_>>();
    Ok(Json(
        rows.into_iter()
            .map(|n| TopNoticeVo {
                notice_id: n.notice_id,
                notice_title: n.notice_title,
                notice_type: n.notice_type,
                create_time: n.create_time.to_rfc3339(),
                read: mine.contains(&n.notice_id),
            })
            .collect(),
    ))
}

pub fn routes(_state: AppState) -> Routes {
    Routes::new()
        .route(
            "/api/v1/system/notices",
            get(list_notices),
            list_notices__perms(),
        )
        .route(
            "/api/v1/system/notices/{notice_id}",
            get(get_notice),
            get_notice__perms(),
        )
        .route_audited(
            "/api/v1/system/notices",
            post(create_notice),
            create_notice__perms(),
            create_notice__audit(),
        )
        .route_audited(
            "/api/v1/system/notices/{notice_id}",
            put(update_notice),
            update_notice__perms(),
            update_notice__audit(),
        )
        .route_audited(
            "/api/v1/system/notices/{notice_id}",
            delete(delete_notice),
            delete_notice__perms(),
            delete_notice__audit(),
        )
        .route(
            "/api/v1/system/notices/{notice_id}/read",
            post(mark_notice_read),
            mark_notice_read__perms(),
        )
        .route(
            "/api/v1/system/notices/{notice_id}/readers",
            get(list_notice_readers),
            list_notice_readers__perms(),
        )
        // 仅需登录：`&[]` 不挂权限守卫（接口清单把 `/notices/top` 标成个人视图）。
        .route("/api/v1/system/notices/top", get(top_notices), &[])
}
