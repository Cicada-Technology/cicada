//! 商品 接口（由 cicada 代码生成器产出）。
//!
//! 权限点前缀 `business:goods`；数据范围由 `ScopedQuery` 在查询层叠加
//! （租户 + 软删 + 归属一次到位），handler 里不写任何租户条件。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use sea_orm::entity::prelude::DateTimeUtc;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, Set};

use cicada_kernel::scope::Query as ScopedQuery;
use cicada_web::AppState;
use cicada_web::problem::{ApiErr, BizCode, Problem};
use cicada_web::extractor::RequestContextExt;
use cicada_web::routes::Routes;

use super::dto::{
    GoodsListQuery, GoodsPage, GoodsUpsert, GoodsVo,
};
use super::entity::{ActiveModel, Column, Entity};

fn err(code: BizCode, msg: impl Into<String>) -> ApiErr {
    Problem::new(code, msg)
}

fn unauthorized() -> ApiErr {
    err(BizCode::AuthUnauthorized, "未认证")
}

fn not_found() -> ApiErr {
    err(BizCode::SysNotFound, "资源不存在")
}

fn bad_request(msg: impl Into<String>) -> ApiErr {
    err(BizCode::ValidationFields, msg)
}

fn db_error(e: impl std::fmt::Display) -> ApiErr {
    tracing::error!(error = %e, "goods 数据库错误");
    err(BizCode::Internal, "内部错误")
}

#[allow(dead_code)]
fn parse_datetime(s: &str) -> Result<DateTimeUtc, ApiErr> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&chrono::Utc))
        .map_err(|_| bad_request(format!("时间格式应为 RFC3339：{s}")))
}

#[allow(dead_code)]
fn parse_datetime_opt(s: Option<&str>) -> Result<Option<DateTimeUtc>, ApiErr> {
    match s.filter(|v| !v.is_empty()) {
        Some(v) => parse_datetime(v).map(Some),
        None => Ok(None),
    }
}

#[allow(dead_code)]
fn parse_date(s: &str) -> Result<chrono::NaiveDate, ApiErr> {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| bad_request(format!("日期格式应为 YYYY-MM-DD：{s}")))
}

#[allow(dead_code)]
fn parse_date_opt(s: Option<&str>) -> Result<Option<chrono::NaiveDate>, ApiErr> {
    match s.filter(|v| !v.is_empty()) {
        Some(v) => parse_date(v).map(Some),
        None => Ok(None),
    }
}

#[allow(dead_code)]
fn parse_time(s: &str) -> Result<chrono::NaiveTime, ApiErr> {
    chrono::NaiveTime::parse_from_str(s, "%H:%M:%S")
        .map_err(|_| bad_request(format!("时间格式应为 HH:MM:SS：{s}")))
}

#[allow(dead_code)]
fn parse_time_opt(s: Option<&str>) -> Result<Option<chrono::NaiveTime>, ApiErr> {
    match s.filter(|v| !v.is_empty()) {
        Some(v) => parse_time(v).map(Some),
        None => Ok(None),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/business/goods",
    params(GoodsListQuery),
    responses((status = 200, description = "商品分页（受数据范围约束）", body = GoodsPage))
)]
#[cicada_macros::require_perm("business:goods:list")]
pub async fn list_models(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<GoodsListQuery>,
) -> Result<Json<GoodsPage>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let mut query = ScopedQuery::<Entity>::open().scoped(&ctx.scope());
    if let Some(v) = q.dept_id {
        query = query.filter(Column::DeptId.eq(v));
    }
    if let Some(v) = q.goods_name.filter(|s| !s.is_empty()) {
        query = query.filter(Column::GoodsName.contains(&v));
    }
    if let Some(v) = q.type_col.filter(|s| !s.is_empty()) {
        query = query.filter(Column::TypeCol.eq(v));
    }
    if let Some(v) = q.price {
        query = query.filter(Column::Price.eq(v));
    }
    if let Some(v) = q.quantity {
        query = query.filter(Column::Quantity.eq(v));
    }
    if let Some(v) = q.online {
        query = query.filter(Column::Online.eq(v));
    }
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).clamp(1, 200);
    let paged = query
        .order_by(Column::GoodsId)
        .fetch_page(state.db.conn(), page, page_size)
        .await
        .map_err(db_error)?;
    Ok(Json(GoodsPage {
        items: paged.items.into_iter().map(GoodsVo::from).collect(),
        total: paged.total,
        page: paged.page,
        page_size: paged.page_size,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/business/goods/{goods_id}",
    params(("goods_id" = i64, Path, description = "商品 id")),
    responses((status = 200, description = "商品详情", body = GoodsVo), (status = 404, description = "不存在"))
)]
#[cicada_macros::require_perm("business:goods:list")]
pub async fn get_model(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(pk): Path<i64>,
) -> Result<Json<GoodsVo>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let model = ScopedQuery::<Entity>::open()
        .scoped(&ctx.scope())
        .filter(Column::GoodsId.eq(pk))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    Ok(Json(GoodsVo::from(model)))
}

#[utoipa::path(
    post,
    path = "/api/v1/business/goods",
    request_body = GoodsUpsert,
    responses((status = 201, description = "已创建", body = GoodsVo))
)]
#[cicada_macros::require_perm("business:goods:add")]
#[cicada_macros::audit(module = "business", action = "goods:add", kind = "insert")]
pub async fn create_model(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<GoodsUpsert>,
) -> Result<(StatusCode, Json<GoodsVo>), ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    let active = ActiveModel {
        tenant_id: Set(ctx.tenant_id.get()),
        dept_id: Set(req.dept_id),
        goods_name: Set(req.goods_name.clone()),
        type_col: Set(req.type_col.clone()),
        price: Set(req.price),
        quantity: Set(req.quantity),
        online: Set(req.online),
        publish_date: Set(parse_date_opt(req.publish_date.as_deref())?),
        publish_at: Set(parse_datetime_opt(req.publish_at.as_deref())?),
        remark: Set(req.remark.clone()),
        create_by: Set(Some(principal.user_name.clone())),
        create_time: Set(chrono::Utc::now()),
        ..Default::default()
    };
    let model = active.insert(state.db.conn()).await.map_err(db_error)?;
    Ok((StatusCode::CREATED, Json(GoodsVo::from(model))))
}

#[utoipa::path(
    put,
    path = "/api/v1/business/goods/{goods_id}",
    params(("goods_id" = i64, Path, description = "商品 id")),
    request_body = GoodsUpsert,
    responses((status = 200, description = "已更新", body = GoodsVo), (status = 404, description = "不存在"))
)]
#[cicada_macros::require_perm("business:goods:edit")]
#[cicada_macros::audit(module = "business", action = "goods:edit", kind = "update")]
pub async fn update_model(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(pk): Path<i64>,
    Json(req): Json<GoodsUpsert>,
) -> Result<Json<GoodsVo>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    let existing = ScopedQuery::<Entity>::open()
        .scoped(&ctx.scope())
        .filter(Column::GoodsId.eq(pk))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    ActiveModel {
        goods_id: Set(pk),
        tenant_id: Set(existing.tenant_id),
        dept_id: Set(req.dept_id),
        goods_name: Set(req.goods_name.clone()),
        type_col: Set(req.type_col.clone()),
        price: Set(req.price),
        quantity: Set(req.quantity),
        online: Set(req.online),
        publish_date: Set(parse_date_opt(req.publish_date.as_deref())?),
        publish_at: Set(parse_datetime_opt(req.publish_at.as_deref())?),
        remark: Set(req.remark.clone()),
        update_by: Set(Some(principal.user_name.clone())),
        update_time: Set(Some(chrono::Utc::now())),
        ..Default::default()
    }
    .update(state.db.conn())
    .await
    .map_err(db_error)?;
    let updated = Entity::find_by_id(pk)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    Ok(Json(GoodsVo::from(updated)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/business/goods/{goods_id}",
    params(("goods_id" = i64, Path, description = "商品 id")),
    responses((status = 204, description = "已删除"), (status = 404, description = "不存在"))
)]
#[cicada_macros::require_perm("business:goods:remove")]
#[cicada_macros::audit(module = "business", action = "goods:remove", kind = "delete")]
pub async fn delete_model(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(pk): Path<i64>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    ScopedQuery::<Entity>::open()
        .scoped(&ctx.scope())
        .filter(Column::GoodsId.eq(pk))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    let now = chrono::Utc::now();
    ActiveModel {
        goods_id: Set(pk),
        deleted_at: Set(Some(now)),
        update_time: Set(Some(now)),
        ..Default::default()
    }
    .update(state.db.conn())
    .await
    .map_err(db_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// 路由声明。`*__perms()` / `*__audit()` 由 `#[require_perm]` / `#[audit]` 生成，
/// 交给装配器在启动期与 `Manifest.perms` 双向对账。
pub fn routes(_state: AppState) -> Routes {
    Routes::new()
        .route("/api/v1/business/goods", get(list_models), list_models__perms())
        .route(
            "/api/v1/business/goods/{goods_id}",
            get(get_model),
            get_model__perms(),
        )
        .route_audited(
            "/api/v1/business/goods",
            post(create_model),
            create_model__perms(),
            create_model__audit(),
        )
        .route_audited(
            "/api/v1/business/goods/{goods_id}",
            put(update_model),
            update_model__perms(),
            update_model__audit(),
        )
        .route_audited(
            "/api/v1/business/goods/{goods_id}",
            delete(delete_model),
            delete_model__perms(),
            delete_model__audit(),
        )
}
