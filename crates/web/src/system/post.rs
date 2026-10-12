//! 岗位管理（M2，权限点 `system:post:*`）。
//!
//! `sys_post` 租户隔离但无归属维度：列表走受控查询 `Query::<PostEntity>`，
//! 非「全部」数据范围按 `NoOwnership` 规则失败关闭；`(tenant_id, post_code)` 唯一。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{delete, get, post, put};
use sea_orm::{ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use cicada_kernel::scope::Query as ScopedQuery;
use cicada_macros::{audit, require_perm};
use cicada_system::entity::{PostActiveModel, PostColumn, PostEntity, PostModel};

use super::{ApiErr, bad_request, conflict, db_error, not_found, unauthorized, version_conflict};
use crate::export;
use crate::extractor::RequestContextExt;
use crate::response::created;
use crate::routes::Routes;
use crate::search::Fuzzy;
use crate::sort::SortExt;
use crate::state::AppState;

pub const PERMS: &[&str] = &[
    "system:post:list",
    "system:post:add",
    "system:post:edit",
    "system:post:remove",
    "system:post:export",
];

#[derive(Debug, Deserialize, IntoParams)]
pub struct PostListQuery {
    pub post_code: Option<String>,
    pub post_name: Option<String>,
    pub status: Option<i16>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    /// 排序列名（排序白名单，见 `crate::sort`）；不在清单里则忽略。
    pub sort_by: Option<String>,
    /// `asc` 升序，其余按 `desc`。
    pub sort_order: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PostVo {
    pub post_id: i64,
    pub post_code: String,
    pub post_name: String,
    pub post_sort: i32,
    pub status: i16,
    pub remark: Option<String>,
    pub create_time: String,
    /// 乐观锁版本（建模规范）。读接口回传，`PUT` 原样带回才做冲突检测。
    pub version: i32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PostPage {
    pub items: Vec<PostVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PostUpsert {
    pub post_code: String,
    pub post_name: String,
    #[serde(default)]
    pub post_sort: i32,
    #[serde(default)]
    pub status: i16,
    pub remark: Option<String>,
    /// 乐观锁期望版本（建模规范），只有 `PUT` 读它；`POST` 建号时新版本恒为 0。
    pub version: Option<i32>,
}

impl From<PostModel> for PostVo {
    fn from(m: PostModel) -> Self {
        Self {
            post_id: m.post_id,
            post_code: m.post_code,
            post_name: m.post_name,
            post_sort: m.post_sort,
            status: m.status,
            remark: m.remark,
            create_time: m.create_time.to_rfc3339(),
            version: m.version,
        }
    }
}

async fn code_taken<C: ConnectionTrait>(
    db: &C,
    tenant: i64,
    code: &str,
    exclude: Option<i64>,
) -> Result<bool, ApiErr> {
    let mut q = PostEntity::find()
        .filter(PostColumn::TenantId.eq(tenant))
        .filter(PostColumn::PostCode.eq(code))
        .filter(PostColumn::DeletedAt.is_null());
    if let Some(id) = exclude {
        q = q.filter(PostColumn::PostId.ne(id));
    }
    Ok(q.one(db).await.map_err(db_error)?.is_some())
}

/// `sys_post` 列表允许排序的列（排序白名单）。
///
/// 只放「列表页真的会点排序」的列：每多一个可排序列，就多一条能被利用的
/// `ORDER BY` 路径（大表上非索引列排序会把排序成本推给每一次翻页）。
const POST_SORT: crate::sort::SortSpec = crate::sort::SortSpec::new(&[
    ("post_code", "sys_post", "post_code"),
    ("post_name", "sys_post", "post_name"),
    ("post_sort", "sys_post", "post_sort"),
    ("status", "sys_post", "status"),
]);

#[utoipa::path(
    get,
    path = "/api/v1/system/posts",
    params(PostListQuery),
    responses((status = 200, description = "岗位分页（受数据范围约束）", body = PostPage))
)]
#[require_perm("system:post:list")]
pub async fn list_posts(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<PostListQuery>,
) -> Result<Json<PostPage>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let mut query = ScopedQuery::<PostEntity>::open().scoped(&ctx.scope());
    if let Some(code) = q.post_code.filter(|s| !s.is_empty()) {
        query = query.filter(PostColumn::PostCode.fuzzy(&code));
    }
    if let Some(name) = q.post_name.filter(|s| !s.is_empty()) {
        query = query.filter(PostColumn::PostName.fuzzy(&name));
    }
    if let Some(status) = q.status {
        query = query.filter(PostColumn::Status.eq(status));
    }

    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;
    let paged = query
        // 排序白名单：只认这张表声明过的列，客户端字符串进不了标识符位置。
        // 显式排序必须排在前面才真的生效：默认次序退化成同值行的收尾键（post_sort → post_id），
        // 既不改变「不传 sort_by 时的结果」，又保证翻页不重不漏。
        .sorted_by(&POST_SORT, q.sort_by.as_deref(), q.sort_order.as_deref())
        .order_by(PostColumn::PostSort)
        .order_by(PostColumn::PostId)
        .fetch_page(state.db.conn(), page, page_size)
        .await
        .map_err(db_error)?;
    Ok(Json(PostPage {
        items: paged.items.into_iter().map(PostVo::from).collect(),
        total: paged.total,
        page: paged.page,
        page_size: paged.page_size,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/system/posts/{post_id}",
    params(("post_id" = i64, Path, description = "岗位 id")),
    responses((status = 200, description = "岗位详情", body = PostVo), (status = 404, description = "不存在"))
)]
#[require_perm("system:post:list")]
pub async fn get_post(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(post_id): Path<i64>,
) -> Result<Json<PostVo>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let post = ScopedQuery::<PostEntity>::open()
        .scoped(&ctx.scope())
        .filter(PostColumn::PostId.eq(post_id))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    Ok(Json(PostVo::from(post)))
}

#[utoipa::path(
    post,
    path = "/api/v1/system/posts",
    request_body = PostUpsert,
    responses((status = 201, description = "已创建", body = PostVo), (status = 409, description = "岗位编码重复"))
)]
#[require_perm("system:post:add")]
#[audit(module = "system", action = "post:add", kind = "insert")]
pub async fn create_post(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<PostUpsert>,
) -> Result<(StatusCode, HeaderMap, Json<PostVo>), ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    if req.post_code.trim().is_empty() || req.post_name.trim().is_empty() {
        return Err(bad_request("岗位编码与岗位名称不能为空"));
    }
    let tenant = ctx.tenant_id.get();
    if code_taken(state.db.conn(), tenant, req.post_code.trim(), None).await? {
        return Err(conflict("岗位编码已存在"));
    }
    let post = PostActiveModel {
        tenant_id: Set(tenant),
        post_code: Set(req.post_code.trim().to_string()),
        post_name: Set(req.post_name.trim().to_string()),
        post_sort: Set(req.post_sort),
        status: Set(req.status),
        version: Set(0),
        remark: Set(req.remark.clone()),
        create_by: Set(Some(principal.user_name.clone())),
        create_time: Set(chrono::Utc::now()),
        ..Default::default()
    }
    .insert(state.db.conn())
    .await
    .map_err(db_error)?;
    Ok(created(
        format!("/api/v1/system/posts/{}", post.post_id),
        PostVo::from(post),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/system/posts/{post_id}",
    request_body = PostUpsert,
    responses((status = 200, description = "已更新", body = PostVo), (status = 404, description = "不存在"), (status = 409, description = "岗位编码重复"))
)]
#[require_perm("system:post:edit")]
#[audit(module = "system", action = "post:edit", kind = "update")]
pub async fn update_post(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(post_id): Path<i64>,
    Json(req): Json<PostUpsert>,
) -> Result<Json<PostVo>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    if req.post_code.trim().is_empty() || req.post_name.trim().is_empty() {
        return Err(bad_request("岗位编码与岗位名称不能为空"));
    }
    let tenant = ctx.tenant_id.get();
    let existing = ScopedQuery::<PostEntity>::open()
        .scoped(&ctx.scope())
        .filter(PostColumn::PostId.eq(post_id))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    if code_taken(state.db.conn(), tenant, req.post_code.trim(), Some(post_id)).await? {
        return Err(conflict("岗位编码已存在"));
    }
    // 留痕快照（审计设计）：本接口能改的列。
    let before = serde_json::json!({
        "post_code": existing.post_code,
        "post_name": existing.post_name,
        "post_sort": existing.post_sort,
        "status": existing.status,
        "remark": existing.remark,
    });

    // 建模规范乐观锁：手写 CAS（SeaORM 2.0.4 没有原生 opt-lock）。0 行受影响 = 版本不匹配或行没了。
    let expected = req.version.unwrap_or(existing.version);
    let applied = PostEntity::update_many()
        .col_expr(
            PostColumn::PostCode,
            sea_orm::sea_query::Expr::value(req.post_code.trim().to_string()),
        )
        .col_expr(
            PostColumn::PostName,
            sea_orm::sea_query::Expr::value(req.post_name.trim().to_string()),
        )
        .col_expr(
            PostColumn::PostSort,
            sea_orm::sea_query::Expr::value(req.post_sort),
        )
        .col_expr(
            PostColumn::Status,
            sea_orm::sea_query::Expr::value(req.status),
        )
        .col_expr(
            PostColumn::Remark,
            sea_orm::sea_query::Expr::value(req.remark.clone()),
        )
        .col_expr(
            PostColumn::UpdateBy,
            sea_orm::sea_query::Expr::value(Some(principal.user_name.clone())),
        )
        .col_expr(
            PostColumn::UpdateTime,
            sea_orm::sea_query::Expr::value(Some(chrono::Utc::now())),
        )
        .col_expr(
            PostColumn::Version,
            sea_orm::sea_query::Expr::value(expected + 1),
        )
        .filter(PostColumn::PostId.eq(post_id))
        .filter(PostColumn::Version.eq(expected))
        .exec(state.db.conn())
        .await
        .map_err(db_error)?;
    if applied.rows_affected == 0 {
        return Err(version_conflict());
    }
    let updated = PostEntity::find_by_id(post_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    crate::audit::record_change(
        &state.audit,
        &ctx,
        "sys_post",
        post_id.to_string(),
        before,
        serde_json::json!({
            "post_code": updated.post_code,
            "post_name": updated.post_name,
            "post_sort": updated.post_sort,
            "status": updated.status,
            "remark": updated.remark,
        }),
    )
    .await;
    Ok(Json(PostVo::from(updated)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/system/posts/{post_id}",
    params(("post_id" = i64, Path, description = "岗位 id")),
    responses((status = 204, description = "已删除"), (status = 404, description = "不存在"))
)]
#[require_perm("system:post:remove")]
#[audit(module = "system", action = "post:remove", kind = "delete")]
pub async fn delete_post(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(post_id): Path<i64>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    ScopedQuery::<PostEntity>::open()
        .scoped(&ctx.scope())
        .filter(PostColumn::PostId.eq(post_id))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    let now = chrono::Utc::now();
    PostActiveModel {
        post_id: Set(post_id),
        deleted_at: Set(Some(now)),
        update_time: Set(Some(now)),
        ..Default::default()
    }
    .update(state.db.conn())
    .await
    .map_err(db_error)?;
    Ok(StatusCode::NO_CONTENT)
}

const EXPORT_HEADERS: &[&str] = &[
    "岗位编号",
    "岗位编码",
    "岗位名称",
    "岗位排序",
    "状态",
    "备注",
    "创建时间",
];
const EXPORT_WIDTHS: &[f64] = &[10.0, 16.0, 18.0, 10.0, 8.0, 24.0, 22.0];

#[utoipa::path(
    get,
    path = "/api/v1/system/posts/export",
    params(PostListQuery),
    responses((status = 200, description = "岗位 xlsx 导出（受数据范围约束）"))
)]
#[require_perm("system:post:export")]
#[audit(module = "system", action = "post:export", kind = "export")]
pub async fn export_posts(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<PostListQuery>,
) -> Result<Response, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let scope = ctx.scope();
    let conn = state.db.conn().clone();
    let code = q.post_code.filter(|s| !s.is_empty());
    let name = q.post_name.filter(|s| !s.is_empty());
    let status = q.status;

    let bytes = export::build("岗位", EXPORT_HEADERS, EXPORT_WIDTHS, |sheet| {
        Box::pin(async move {
            let mut row: u32 = 1;
            let mut offset: u64 = 0;
            while u64::from(row) - 1 < export::MAX_EXPORT_ROWS {
                let mut query = ScopedQuery::<PostEntity>::open().scoped(&scope);
                if let Some(c) = &code {
                    query = query.filter(PostColumn::PostCode.fuzzy(c));
                }
                if let Some(n) = &name {
                    query = query.filter(PostColumn::PostName.fuzzy(n));
                }
                if let Some(s) = status {
                    query = query.filter(PostColumn::Status.eq(s));
                }
                let batch = query
                    .order_by(PostColumn::PostSort)
                    .order_by(PostColumn::PostId)
                    .limit(export::EXPORT_BATCH)
                    .offset(offset)
                    .all(&conn)
                    .await?;
                if batch.is_empty() {
                    break;
                }
                for p in &batch {
                    sheet.write_string(row, 0, p.post_id.to_string())?;
                    sheet.write_string(row, 1, &p.post_code)?;
                    sheet.write_string(row, 2, &p.post_name)?;
                    sheet.write_string(row, 3, p.post_sort.to_string())?;
                    sheet.write_string(row, 4, if p.status == 0 { "正常" } else { "停用" })?;
                    sheet.write_string(row, 5, p.remark.clone().unwrap_or_default())?;
                    sheet.write_string(row, 6, p.create_time.to_rfc3339())?;
                    row += 1;
                }
                offset += batch.len() as u64;
            }
            Ok(())
        })
    })
    .await
    .map_err(db_error)?;

    Ok(export::xlsx_response("posts.xlsx", bytes))
}

pub fn routes(_state: AppState) -> Routes {
    Routes::new()
        .route("/api/v1/system/posts", get(list_posts), list_posts__perms())
        .route_audited(
            "/api/v1/system/posts/export",
            get(export_posts),
            export_posts__perms(),
            export_posts__audit(),
        )
        .route(
            "/api/v1/system/posts/{post_id}",
            get(get_post),
            get_post__perms(),
        )
        .route_audited(
            "/api/v1/system/posts",
            post(create_post),
            create_post__perms(),
            create_post__audit(),
        )
        .route_audited(
            "/api/v1/system/posts/{post_id}",
            put(update_post),
            update_post__perms(),
            update_post__audit(),
        )
        .route_audited(
            "/api/v1/system/posts/{post_id}",
            delete(delete_post),
            delete_post__perms(),
            delete_post__audit(),
        )
}
