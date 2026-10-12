//! 操作日志 / 登录日志：`monitor:operlog:*`、`monitor:logininfor:*`。
//!
//! 只读 + 清理视图：日志由 `cicada-audit` 的 sink 写入，本模块不产生日志。
//! `sys_oper_log` / `sys_logininfor` 无归属维度，列表走受控查询
//! `Query::<..>::open().scoped(&scope)`：非「全部」数据范围按 `NoOwnership` 失败关闭。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{delete, get, put};
use chrono::{DateTime, Utc};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use cicada_auth::lockout_key;
use cicada_kernel::scope::{Query as ScopedQuery, QueryState};
use cicada_macros::{audit, require_perm};
use cicada_system::entity::{
    AuditLogColumn, AuditLogEntity, AuditLogModel, LoginInfoColumn, LoginInfoEntity,
    LoginInfoModel, OperLogColumn, OperLogEntity, OperLogModel,
};

use crate::Permission;
use crate::export;
use crate::extractor::RequestContextExt;
use crate::module::{Manifest, Module};
use crate::routes::Routes;
use crate::state::AppState;

use crate::problem::ApiErr;
use crate::search::Fuzzy;
use crate::sort::SortExt;

fn err(status: StatusCode, msg: impl Into<String>) -> ApiErr {
    ApiErr::from_status(status, msg)
}

fn unauthorized() -> ApiErr {
    err(StatusCode::UNAUTHORIZED, "未认证")
}

fn not_found() -> ApiErr {
    err(StatusCode::NOT_FOUND, "资源不存在")
}

fn db_error(e: impl std::fmt::Display) -> ApiErr {
    tracing::error!(error = %e, "日志模块数据库错误");
    err(
        StatusCode::INTERNAL_SERVER_ERROR,
        "日志查询失败，请稍后重试",
    )
}

pub const PERMS: &[&str] = &[
    "monitor:operlog:list",
    "monitor:operlog:detail",
    "monitor:operlog:remove",
    "monitor:operlog:export",
    "monitor:logininfor:list",
    "monitor:logininfor:remove",
    "monitor:logininfor:export",
    "monitor:logininfor:unlock",
    // 字段级变更留痕（审计设计，P1）。
    "monitor:auditlog:list",
];

pub(crate) fn parse_time(raw: &Option<String>) -> Option<DateTime<Utc>> {
    raw.as_deref()
        .filter(|s| !s.is_empty())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.with_timezone(&Utc))
}

fn business_label(v: i16) -> &'static str {
    match v {
        1 => "新增",
        2 => "修改",
        3 => "删除",
        4 => "授权",
        5 => "导出",
        6 => "导入",
        7 => "强退",
        8 => "生成代码",
        9 => "清空",
        _ => "其它",
    }
}

// ============================ 操作日志 ============================

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
pub struct OperLogListQuery {
    pub title: Option<String>,
    pub oper_name: Option<String>,
    pub business_type: Option<i16>,
    pub status: Option<i16>,
    /// RFC3339 起始时间（含）。
    pub begin_time: Option<String>,
    /// RFC3339 结束时间（含）。
    pub end_time: Option<String>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    /// 排序列名（排序白名单，见 `crate::sort`）；不在清单里则忽略。
    pub sort_by: Option<String>,
    /// `asc` 升序，其余按 `desc`。
    pub sort_order: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OperLogVo {
    pub oper_id: i64,
    pub title: String,
    pub business_type: i16,
    pub method: String,
    pub request_method: String,
    pub oper_name: Option<String>,
    pub oper_url: String,
    pub oper_ip: Option<String>,
    pub oper_param: Option<String>,
    pub json_result: Option<String>,
    pub status: i16,
    pub error_msg: Option<String>,
    pub oper_time: String,
    pub cost_ms: i64,
    /// 审计日志的三项：UA、业务码、脱敏后的请求体（旧行没有，故为 null）。
    pub user_agent: Option<String>,
    pub biz_code: Option<String>,
    pub req_body: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OperLogPage {
    pub items: Vec<OperLogVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

impl From<OperLogModel> for OperLogVo {
    fn from(m: OperLogModel) -> Self {
        Self {
            oper_id: m.oper_id,
            title: m.title,
            business_type: m.business_type,
            method: m.method,
            request_method: m.request_method,
            oper_name: m.oper_name,
            oper_url: m.oper_url,
            oper_ip: m.oper_ip,
            oper_param: m.oper_param,
            json_result: m.json_result,
            status: m.status,
            error_msg: m.error_msg,
            oper_time: m.oper_time.to_rfc3339(),
            cost_ms: m.cost_ms,
            user_agent: m.user_agent,
            biz_code: m.biz_code,
            req_body: m.req_body,
        }
    }
}

fn apply_oper_filters<S: QueryState>(
    mut query: ScopedQuery<OperLogEntity, S>,
    q: &OperLogListQuery,
) -> ScopedQuery<OperLogEntity, S> {
    if let Some(title) = q.title.clone().filter(|s| !s.is_empty()) {
        query = query.filter(OperLogColumn::Title.fuzzy(&title));
    }
    if let Some(name) = q.oper_name.clone().filter(|s| !s.is_empty()) {
        query = query.filter(OperLogColumn::OperName.fuzzy(&name));
    }
    if let Some(bt) = q.business_type {
        query = query.filter(OperLogColumn::BusinessType.eq(bt));
    }
    if let Some(status) = q.status {
        query = query.filter(OperLogColumn::Status.eq(status));
    }
    if let Some(begin) = parse_time(&q.begin_time) {
        query = query.filter(OperLogColumn::OperTime.gte(begin));
    }
    if let Some(end) = parse_time(&q.end_time) {
        query = query.filter(OperLogColumn::OperTime.lte(end));
    }
    query
}

/// `sys_oper_log` 列表允许排序的列（排序白名单）。
///
/// 只放「列表页真的会点排序」的列：每多一个可排序列，就多一条能被利用的
/// `ORDER BY` 路径（大表上非索引列排序会把排序成本推给每一次翻页）。
const OPER_LOG_SORT: crate::sort::SortSpec = crate::sort::SortSpec::new(&[
    ("title", "sys_oper_log", "title"),
    ("oper_name", "sys_oper_log", "oper_name"),
    ("business_type", "sys_oper_log", "business_type"),
    ("status", "sys_oper_log", "status"),
    ("oper_time", "sys_oper_log", "oper_time"),
]);

#[utoipa::path(
    get,
    path = "/api/v1/monitor/oper-logs",
    params(OperLogListQuery),
    responses((status = 200, description = "操作日志分页", body = OperLogPage))
)]
#[require_perm("monitor:operlog:list")]
pub async fn list_oper_logs(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<OperLogListQuery>,
) -> Result<Json<OperLogPage>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let scope = ctx.scope();
    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;

    let query = apply_oper_filters(ScopedQuery::<OperLogEntity>::open().scoped(&scope), &q);
    let paged = query
        // 排序白名单：只认这张表声明过的列，客户端字符串进不了标识符位置。
        // 显式排序必须排在前面才真的生效：默认次序退化成同值行的收尾键（oper_id），
        // 既不改变「不传 sort_by 时的结果」，又保证翻页不重不漏。
        .sorted_by(
            &OPER_LOG_SORT,
            q.sort_by.as_deref(),
            q.sort_order.as_deref(),
        )
        .order_by_desc(OperLogColumn::OperId)
        .fetch_page(state.db.conn(), page, page_size)
        .await
        .map_err(db_error)?;
    Ok(Json(OperLogPage {
        items: paged.items.into_iter().map(OperLogVo::from).collect(),
        total: paged.total,
        page: paged.page,
        page_size: paged.page_size,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/monitor/oper-logs/{oper_id}",
    params(("oper_id" = i64, Path, description = "日志 id")),
    responses((status = 200, description = "操作日志详情", body = OperLogVo), (status = 404, description = "不存在"))
)]
#[require_perm("monitor:operlog:detail")]
pub async fn get_oper_log(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(oper_id): Path<i64>,
) -> Result<Json<OperLogVo>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let row = ScopedQuery::<OperLogEntity>::open()
        .scoped(&ctx.scope())
        .filter(OperLogColumn::OperId.eq(oper_id))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    Ok(Json(OperLogVo::from(row)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/monitor/oper-logs/{oper_id}",
    params(("oper_id" = i64, Path, description = "日志 id")),
    responses((status = 204, description = "已删除"), (status = 404, description = "不存在"))
)]
#[require_perm("monitor:operlog:remove")]
#[audit(module = "monitor", action = "operlog:remove", kind = "delete")]
pub async fn remove_oper_log(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(oper_id): Path<i64>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    ScopedQuery::<OperLogEntity>::open()
        .scoped(&ctx.scope())
        .filter(OperLogColumn::OperId.eq(oper_id))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    OperLogEntity::delete_by_id(oper_id)
        .exec(state.db.conn())
        .await
        .map_err(db_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/api/v1/monitor/oper-logs",
    responses((status = 204, description = "已清空本租户操作日志"))
)]
#[require_perm("monitor:operlog:remove")]
#[audit(module = "monitor", action = "operlog:clean", kind = "clean")]
pub async fn clean_oper_logs(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    body: Option<Json<OperLogListQuery>>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let q = body.map(|Json(q)| q).unwrap_or_default();
    let mut delete =
        OperLogEntity::delete_many().filter(OperLogColumn::TenantId.eq(ctx.tenant_id.get()));
    if let Some(title) = q.title.filter(|s| !s.is_empty()) {
        delete = delete.filter(OperLogColumn::Title.fuzzy(&title));
    }
    if let Some(name) = q.oper_name.filter(|s| !s.is_empty()) {
        delete = delete.filter(OperLogColumn::OperName.fuzzy(&name));
    }
    if let Some(bt) = q.business_type {
        delete = delete.filter(OperLogColumn::BusinessType.eq(bt));
    }
    if let Some(status) = q.status {
        delete = delete.filter(OperLogColumn::Status.eq(status));
    }
    if let Some(begin) = parse_time(&q.begin_time) {
        delete = delete.filter(OperLogColumn::OperTime.gte(begin));
    }
    if let Some(end) = parse_time(&q.end_time) {
        delete = delete.filter(OperLogColumn::OperTime.lte(end));
    }
    delete.exec(state.db.conn()).await.map_err(db_error)?;
    Ok(StatusCode::NO_CONTENT)
}

const OPER_HEADERS: &[&str] = &[
    "日志编号",
    "系统模块",
    "操作类型",
    "请求方式",
    "操作人员",
    "请求地址",
    "操作IP",
    "状态",
    "操作时间",
    "耗时(ms)",
];
const OPER_WIDTHS: &[f64] = &[10.0, 20.0, 12.0, 10.0, 14.0, 34.0, 16.0, 8.0, 22.0, 10.0];

#[utoipa::path(
    get,
    path = "/api/v1/monitor/oper-logs/export",
    params(OperLogListQuery),
    responses((status = 200, description = "操作日志 xlsx 导出"))
)]
#[require_perm("monitor:operlog:export")]
#[audit(module = "monitor", action = "operlog:export", kind = "export")]
pub async fn export_oper_logs(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<OperLogListQuery>,
) -> Result<Response, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let scope = ctx.scope();
    let conn = state.db.conn().clone();
    let filter = q;

    let bytes = export::build("操作日志", OPER_HEADERS, OPER_WIDTHS, |sheet| {
        Box::pin(async move {
            let mut row: u32 = 1;
            let mut offset: u64 = 0;
            while u64::from(row) - 1 < export::MAX_EXPORT_ROWS {
                let query = apply_oper_filters(
                    ScopedQuery::<OperLogEntity>::open().scoped(&scope),
                    &filter,
                );
                let batch = query
                    .order_by_desc(OperLogColumn::OperId)
                    .limit(export::EXPORT_BATCH)
                    .offset(offset)
                    .all(&conn)
                    .await?;
                if batch.is_empty() {
                    break;
                }
                for l in &batch {
                    sheet.write_string(row, 0, l.oper_id.to_string())?;
                    sheet.write_string(row, 1, &l.title)?;
                    sheet.write_string(row, 2, business_label(l.business_type))?;
                    sheet.write_string(row, 3, &l.request_method)?;
                    sheet.write_string(row, 4, l.oper_name.clone().unwrap_or_default())?;
                    sheet.write_string(row, 5, &l.oper_url)?;
                    sheet.write_string(row, 6, l.oper_ip.clone().unwrap_or_default())?;
                    sheet.write_string(row, 7, if l.status == 0 { "正常" } else { "异常" })?;
                    sheet.write_string(row, 8, l.oper_time.to_rfc3339())?;
                    sheet.write_string(row, 9, l.cost_ms.to_string())?;
                    row += 1;
                }
                offset += batch.len() as u64;
            }
            Ok(())
        })
    })
    .await
    .map_err(db_error)?;

    Ok(export::xlsx_response("oper_logs.xlsx", bytes))
}

// ============================ 登录日志 ============================

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
pub struct LoginInfoListQuery {
    pub user_name: Option<String>,
    pub ipaddr: Option<String>,
    pub status: Option<i16>,
    /// RFC3339 起始时间（含）。
    pub begin_time: Option<String>,
    /// RFC3339 结束时间（含）。
    pub end_time: Option<String>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    /// 排序列名（排序白名单，见 `crate::sort`）；不在清单里则忽略。
    pub sort_by: Option<String>,
    /// `asc` 升序，其余按 `desc`。
    pub sort_order: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoginInfoVo {
    pub info_id: i64,
    pub user_name: String,
    pub ipaddr: Option<String>,
    pub login_location: Option<String>,
    pub browser: Option<String>,
    pub os: Option<String>,
    pub status: i16,
    pub msg: Option<String>,
    pub login_time: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoginInfoPage {
    pub items: Vec<LoginInfoVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

impl From<LoginInfoModel> for LoginInfoVo {
    fn from(m: LoginInfoModel) -> Self {
        Self {
            info_id: m.info_id,
            user_name: m.user_name,
            ipaddr: m.ipaddr,
            login_location: m.login_location,
            browser: m.browser,
            os: m.os,
            status: m.status,
            msg: m.msg,
            login_time: m.login_time.to_rfc3339(),
        }
    }
}

fn apply_login_filters<S: QueryState>(
    mut query: ScopedQuery<LoginInfoEntity, S>,
    q: &LoginInfoListQuery,
) -> ScopedQuery<LoginInfoEntity, S> {
    if let Some(name) = q.user_name.clone().filter(|s| !s.is_empty()) {
        query = query.filter(LoginInfoColumn::UserName.fuzzy(&name));
    }
    if let Some(ip) = q.ipaddr.clone().filter(|s| !s.is_empty()) {
        query = query.filter(LoginInfoColumn::Ipaddr.fuzzy(&ip));
    }
    if let Some(status) = q.status {
        query = query.filter(LoginInfoColumn::Status.eq(status));
    }
    if let Some(begin) = parse_time(&q.begin_time) {
        query = query.filter(LoginInfoColumn::LoginTime.gte(begin));
    }
    if let Some(end) = parse_time(&q.end_time) {
        query = query.filter(LoginInfoColumn::LoginTime.lte(end));
    }
    query
}

/// `sys_logininfor` 列表允许排序的列（排序白名单）。
///
/// 只放「列表页真的会点排序」的列：每多一个可排序列，就多一条能被利用的
/// `ORDER BY` 路径（大表上非索引列排序会把排序成本推给每一次翻页）。
const LOGIN_LOG_SORT: crate::sort::SortSpec = crate::sort::SortSpec::new(&[
    ("user_name", "sys_logininfor", "user_name"),
    ("ipaddr", "sys_logininfor", "ipaddr"),
    ("status", "sys_logininfor", "status"),
    ("login_time", "sys_logininfor", "login_time"),
]);

#[utoipa::path(
    get,
    path = "/api/v1/monitor/login-logs",
    params(LoginInfoListQuery),
    responses((status = 200, description = "登录日志分页", body = LoginInfoPage))
)]
#[require_perm("monitor:logininfor:list")]
pub async fn list_login_logs(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<LoginInfoListQuery>,
) -> Result<Json<LoginInfoPage>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let scope = ctx.scope();
    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;

    let query = apply_login_filters(ScopedQuery::<LoginInfoEntity>::open().scoped(&scope), &q);
    let paged = query
        // 排序白名单：只认这张表声明过的列，客户端字符串进不了标识符位置。
        // 显式排序必须排在前面才真的生效：默认次序退化成同值行的收尾键（info_id），
        // 既不改变「不传 sort_by 时的结果」，又保证翻页不重不漏。
        .sorted_by(
            &LOGIN_LOG_SORT,
            q.sort_by.as_deref(),
            q.sort_order.as_deref(),
        )
        .order_by_desc(LoginInfoColumn::InfoId)
        .fetch_page(state.db.conn(), page, page_size)
        .await
        .map_err(db_error)?;
    Ok(Json(LoginInfoPage {
        items: paged.items.into_iter().map(LoginInfoVo::from).collect(),
        total: paged.total,
        page: paged.page,
        page_size: paged.page_size,
    }))
}

#[utoipa::path(
    delete,
    path = "/api/v1/monitor/login-logs/{info_id}",
    params(("info_id" = i64, Path, description = "日志 id")),
    responses((status = 204, description = "已删除"), (status = 404, description = "不存在"))
)]
#[require_perm("monitor:logininfor:remove")]
#[audit(module = "monitor", action = "logininfor:remove", kind = "delete")]
pub async fn remove_login_log(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(info_id): Path<i64>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    ScopedQuery::<LoginInfoEntity>::open()
        .scoped(&ctx.scope())
        .filter(LoginInfoColumn::InfoId.eq(info_id))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    LoginInfoEntity::delete_by_id(info_id)
        .exec(state.db.conn())
        .await
        .map_err(db_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/api/v1/monitor/login-logs",
    responses((status = 204, description = "已清空本租户登录日志"))
)]
#[require_perm("monitor:logininfor:remove")]
#[audit(module = "monitor", action = "logininfor:clean", kind = "clean")]
pub async fn clean_login_logs(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    body: Option<Json<LoginInfoListQuery>>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let q = body.map(|Json(q)| q).unwrap_or_default();
    let mut delete =
        LoginInfoEntity::delete_many().filter(LoginInfoColumn::TenantId.eq(ctx.tenant_id.get()));
    if let Some(name) = q.user_name.filter(|s| !s.is_empty()) {
        delete = delete.filter(LoginInfoColumn::UserName.fuzzy(&name));
    }
    if let Some(ip) = q.ipaddr.filter(|s| !s.is_empty()) {
        delete = delete.filter(LoginInfoColumn::Ipaddr.fuzzy(&ip));
    }
    if let Some(status) = q.status {
        delete = delete.filter(LoginInfoColumn::Status.eq(status));
    }
    if let Some(begin) = parse_time(&q.begin_time) {
        delete = delete.filter(LoginInfoColumn::LoginTime.gte(begin));
    }
    if let Some(end) = parse_time(&q.end_time) {
        delete = delete.filter(LoginInfoColumn::LoginTime.lte(end));
    }
    delete.exec(state.db.conn()).await.map_err(db_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    put,
    path = "/api/v1/monitor/login-logs/unlock/{user_name}",
    params(("user_name" = String, Path, description = "登录账号")),
    responses(
        (status = 204, description = "已解锁"),
        (status = 404, description = "账号不存在")
    )
)]
#[require_perm("monitor:logininfor:unlock")]
#[audit(module = "monitor", action = "logininfor:unlock", kind = "other")]
pub async fn unlock_login(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(user_name): Path<String>,
) -> Result<StatusCode, ApiErr> {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let tenant_id = ctx.tenant_id.get();
    // 账号必须在租户内存在，避免对任意字符串写缓存键。
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT 1 FROM sys_user u WHERE u.tenant_id = $2 AND u.deleted_at IS NULL \
           AND (u.user_name = $1 OR EXISTS (SELECT 1 FROM sys_user_identity i \
                 WHERE i.user_id = u.user_id AND i.tenant_id = u.tenant_id AND i.identifier = $1)) \
         LIMIT 1",
        [user_name.clone().into(), tenant_id.into()],
    );
    let exists = state
        .db
        .conn()
        .query_all_raw(stmt)
        .await
        .map_err(db_error)?
        .into_iter()
        .next()
        .is_some();
    if !exists {
        return Err(not_found());
    }
    state
        .lockout
        .clear(&lockout_key(tenant_id, &user_name))
        .await
        .map_err(db_error)?;
    Ok(StatusCode::NO_CONTENT)
}

const LOGIN_HEADERS: &[&str] = &[
    "访问编号",
    "用户名称",
    "登录地址",
    "登录地点",
    "浏览器",
    "操作系统",
    "状态",
    "操作信息",
    "登录时间",
];
const LOGIN_WIDTHS: &[f64] = &[10.0, 16.0, 16.0, 16.0, 12.0, 12.0, 8.0, 26.0, 22.0];

#[utoipa::path(
    get,
    path = "/api/v1/monitor/login-logs/export",
    params(LoginInfoListQuery),
    responses((status = 200, description = "登录日志 xlsx 导出"))
)]
#[require_perm("monitor:logininfor:export")]
#[audit(module = "monitor", action = "logininfor:export", kind = "export")]
pub async fn export_login_logs(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<LoginInfoListQuery>,
) -> Result<Response, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let scope = ctx.scope();
    let conn = state.db.conn().clone();
    let filter = q;

    let bytes = export::build("登录日志", LOGIN_HEADERS, LOGIN_WIDTHS, |sheet| {
        Box::pin(async move {
            let mut row: u32 = 1;
            let mut offset: u64 = 0;
            while u64::from(row) - 1 < export::MAX_EXPORT_ROWS {
                let query = apply_login_filters(
                    ScopedQuery::<LoginInfoEntity>::open().scoped(&scope),
                    &filter,
                );
                let batch = query
                    .order_by_desc(LoginInfoColumn::InfoId)
                    .limit(export::EXPORT_BATCH)
                    .offset(offset)
                    .all(&conn)
                    .await?;
                if batch.is_empty() {
                    break;
                }
                for l in &batch {
                    sheet.write_string(row, 0, l.info_id.to_string())?;
                    sheet.write_string(row, 1, &l.user_name)?;
                    sheet.write_string(row, 2, l.ipaddr.clone().unwrap_or_default())?;
                    sheet.write_string(row, 3, l.login_location.clone().unwrap_or_default())?;
                    sheet.write_string(row, 4, l.browser.clone().unwrap_or_default())?;
                    sheet.write_string(row, 5, l.os.clone().unwrap_or_default())?;
                    sheet.write_string(row, 6, if l.status == 0 { "成功" } else { "失败" })?;
                    sheet.write_string(row, 7, l.msg.clone().unwrap_or_default())?;
                    sheet.write_string(row, 8, l.login_time.to_rfc3339())?;
                    row += 1;
                }
                offset += batch.len() as u64;
            }
            Ok(())
        })
    })
    .await
    .map_err(db_error)?;

    Ok(export::xlsx_response("login_logs.xlsx", bytes))
}

// ---------------------------------------------------------------------------
// 字段级变更留痕（审计设计，P1）：只读视图。写入方是 relay → `cicada_system::SysLogStore`，
// 本模块不产生留痕；`changed` 里只有变化字段，凭据类字段在生成端已被滤除。
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, IntoParams)]
pub struct AuditLogListQuery {
    /// 被改动的实体（表名），精确匹配。
    pub table_name: Option<String>,
    /// 记录主键，精确匹配。
    pub record_id: Option<String>,
    /// `insert` / `update` / `delete`。
    pub action: Option<String>,
    pub begin_time: Option<String>,
    pub end_time: Option<String>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuditLogVo {
    pub audit_id: i64,
    pub table_name: String,
    pub record_id: String,
    pub action: String,
    /// `{field: [old, new]}`。
    pub changed: serde_json::Value,
    pub oper_id: Option<i64>,
    pub trace_id: Option<String>,
    pub oper_time: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuditLogPage {
    pub items: Vec<AuditLogVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

impl From<AuditLogModel> for AuditLogVo {
    fn from(m: AuditLogModel) -> Self {
        Self {
            audit_id: m.audit_id,
            table_name: m.table_name,
            record_id: m.record_id,
            action: m.action,
            changed: m.changed,
            oper_id: m.oper_id,
            trace_id: m.trace_id,
            oper_time: m.oper_time.to_rfc3339(),
        }
    }
}

fn apply_audit_filters<S: QueryState>(
    mut query: ScopedQuery<AuditLogEntity, S>,
    q: &AuditLogListQuery,
) -> ScopedQuery<AuditLogEntity, S> {
    if let Some(table) = q.table_name.clone().filter(|v| !v.is_empty()) {
        query = query.filter(AuditLogColumn::TableName.eq(table));
    }
    if let Some(record) = q.record_id.clone().filter(|v| !v.is_empty()) {
        query = query.filter(AuditLogColumn::RecordId.eq(record));
    }
    if let Some(action) = q.action.clone().filter(|v| !v.is_empty()) {
        query = query.filter(AuditLogColumn::Action.eq(action));
    }
    if let Some(begin) = parse_time(&q.begin_time) {
        query = query.filter(AuditLogColumn::OperTime.gte(begin));
    }
    if let Some(end) = parse_time(&q.end_time) {
        query = query.filter(AuditLogColumn::OperTime.lte(end));
    }
    query
}

#[utoipa::path(
    get,
    path = "/api/v1/monitor/audit-logs",
    params(AuditLogListQuery),
    responses((status = 200, description = "字段级变更留痕分页", body = AuditLogPage))
)]
#[require_perm("monitor:auditlog:list")]
pub async fn list_audit_logs(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<AuditLogListQuery>,
) -> Result<Json<AuditLogPage>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let scope = ctx.scope();
    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;

    let query = apply_audit_filters(ScopedQuery::<AuditLogEntity>::open().scoped(&scope), &q);
    let paged = query
        .order_by_desc(AuditLogColumn::AuditId)
        .fetch_page(state.db.conn(), page, page_size)
        .await
        .map_err(db_error)?;
    Ok(Json(AuditLogPage {
        items: paged.items.into_iter().map(AuditLogVo::from).collect(),
        total: paged.total,
        page: paged.page,
        page_size: paged.page_size,
    }))
}

pub struct LogModule;

impl Module for LogModule {
    fn manifest(self: Box<Self>) -> Manifest {
        Manifest::new("cicada:monitor-logs")
            .perms(PERMS.iter().map(|p| Permission::new(*p)))
            .routes(routes)
    }
}

pub fn routes(_state: AppState) -> Routes {
    Routes::new()
        .route(
            "/api/v1/monitor/oper-logs",
            get(list_oper_logs),
            list_oper_logs__perms(),
        )
        .route_audited(
            "/api/v1/monitor/oper-logs/export",
            get(export_oper_logs),
            export_oper_logs__perms(),
            export_oper_logs__audit(),
        )
        .route(
            "/api/v1/monitor/oper-logs/{oper_id}",
            get(get_oper_log),
            get_oper_log__perms(),
        )
        .route_audited(
            "/api/v1/monitor/oper-logs",
            delete(clean_oper_logs),
            clean_oper_logs__perms(),
            clean_oper_logs__audit(),
        )
        .route_audited(
            "/api/v1/monitor/oper-logs/{oper_id}",
            delete(remove_oper_log),
            remove_oper_log__perms(),
            remove_oper_log__audit(),
        )
        .route(
            "/api/v1/monitor/login-logs",
            get(list_login_logs),
            list_login_logs__perms(),
        )
        .route_audited(
            "/api/v1/monitor/login-logs/unlock/{user_name}",
            put(unlock_login),
            unlock_login__perms(),
            unlock_login__audit(),
        )
        .route_audited(
            "/api/v1/monitor/login-logs/export",
            get(export_login_logs),
            export_login_logs__perms(),
            export_login_logs__audit(),
        )
        .route_audited(
            "/api/v1/monitor/login-logs",
            delete(clean_login_logs),
            clean_login_logs__perms(),
            clean_login_logs__audit(),
        )
        .route_audited(
            "/api/v1/monitor/login-logs/{info_id}",
            delete(remove_login_log),
            remove_login_log__perms(),
            remove_login_log__audit(),
        )
        .route(
            "/api/v1/monitor/audit-logs",
            get(list_audit_logs),
            list_audit_logs__perms(),
        )
}
