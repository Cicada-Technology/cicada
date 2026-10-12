//! 在线用户管理：`monitor:online:*`。
//!
//! 权威源是 Redis 会话（`SessionStore`），`sys_user_online` 只是异步快照，不参与判定。
//! 强退 = 撤销会话；中间件按 `jti` 校验会话存在性，故 access_token 立即失效。

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{
    Json,
    routing::{delete, get},
};
use sea_orm::ColumnTrait;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use cicada_auth::online::{
    Column as UserOnlineColumn, Entity as UserOnlineEntity, Model as UserOnlineModel,
};
use cicada_auth::{AuthError, Session};
use cicada_kernel::scope::{Query as ScopedQuery, QueryState};
use cicada_macros::require_perm;

use crate::Permission;
use crate::extractor::RequestContextExt;
use crate::module::{Manifest, Module};
use crate::routes::Routes;
use crate::state::AppState;

use crate::problem::{ApiErr, BizCode, Problem};
use crate::search::Fuzzy;
use crate::sort::SortExt;

#[derive(Debug, Deserialize, IntoParams)]
pub struct ListQuery {
    pub user_name: Option<String>,
    pub ip: Option<String>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OnlineUserVo {
    pub session_id: String,
    pub user_id: i64,
    pub tenant_id: i64,
    pub user_name: String,
    pub ip: String,
    pub user_agent: String,
    pub login_time: i64,
    pub last_seen: i64,
}

/// 集合响应形状：`{ items, total, page, page_size }`。
#[derive(Debug, Serialize, ToSchema)]
pub struct OnlineUserPage {
    pub items: Vec<OnlineUserVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct BatchForceLogoutRequest {
    pub session_ids: Vec<String>,
}

fn to_vo(session_id: String, s: Session) -> OnlineUserVo {
    OnlineUserVo {
        session_id,
        user_id: s.user_id,
        tenant_id: s.tenant_id,
        user_name: s.username,
        ip: s.ip,
        user_agent: s.user_agent,
        login_time: s.login_time,
        last_seen: s.last_seen,
    }
}

fn map_err(e: &AuthError) -> ApiErr {
    match e {
        AuthError::Session(_) => Problem::new(BizCode::CacheUnavailable, e.to_string()),
        _ => ApiErr::from_status(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

fn unauthorized() -> ApiErr {
    ApiErr::from_status(StatusCode::UNAUTHORIZED, "未认证")
}

fn db_error(e: impl std::fmt::Display) -> ApiErr {
    tracing::error!(error = %e, "在线会话快照查询失败");
    ApiErr::from_status(
        StatusCode::INTERNAL_SERVER_ERROR,
        "在线会话查询失败，请稍后重试",
    )
}

fn forbidden(msg: &str) -> ApiErr {
    Problem::new(BizCode::RbacForbidden, msg.to_string())
}

#[utoipa::path(
    get,
    path = "/api/v1/monitor/online-users",
    params(ListQuery),
    responses(
        (status = 200, description = "在线用户分页", body = OnlineUserPage),
        (status = 403, description = "无权限", body = Problem)
    )
)]
#[require_perm("monitor:online:list")]
pub async fn list_online(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<ListQuery>,
) -> Result<Json<OnlineUserPage>, ApiErr> {
    let ctx = ctx.into_inner();
    let mut all = state.sessions.list_all().await.map_err(|e| map_err(&e))?;
    // 非超管只看本租户在线会话（水平越权防护）。
    if !ctx.is_super_admin {
        all.retain(|(_, s)| s.tenant_id == ctx.tenant_id.get());
    }
    if let Some(name) = q.user_name.as_deref().filter(|s| !s.is_empty()) {
        all.retain(|(_, s)| s.username.contains(name));
    }
    if let Some(ip) = q.ip.as_deref().filter(|s| !s.is_empty()) {
        all.retain(|(_, s)| s.ip == ip);
    }
    all.sort_by_key(|(_, s)| std::cmp::Reverse(s.last_seen));

    let total = all.len() as u64;
    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;
    let start = ((page - 1) * page_size) as usize;
    let items = all
        .into_iter()
        .skip(start)
        .take(page_size as usize)
        .map(|(jti, s)| to_vo(jti, s))
        .collect();

    Ok(Json(OnlineUserPage {
        items,
        total,
        page,
        page_size,
    }))
}

#[utoipa::path(
    delete,
    path = "/api/v1/monitor/online-users/{session_id}",
    params(("session_id" = String, Path, description = "会话 id（jti）")),
    responses(
        (status = 204, description = "已强退"),
        (status = 403, description = "无权限或跨租户", body = Problem)
    )
)]
#[require_perm("monitor:online:forceLogout")]
pub async fn force_logout(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(session_id): Path<String>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    if let Some(session) = state
        .sessions
        .get(&session_id)
        .await
        .map_err(|e| map_err(&e))?
    {
        if !ctx.is_super_admin && session.tenant_id != ctx.tenant_id.get() {
            return Err(forbidden("目标会话不在当前租户"));
        }
        state
            .sessions
            .revoke(&session_id)
            .await
            .map_err(|e| map_err(&e))?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/api/v1/monitor/online-users",
    request_body = BatchForceLogoutRequest,
    responses(
        (status = 204, description = "已批量强退"),
        (status = 403, description = "无权限或跨租户", body = Problem)
    )
)]
#[require_perm("monitor:online:batchForceLogout")]
pub async fn batch_force_logout(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<BatchForceLogoutRequest>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    for session_id in &req.session_ids {
        if let Some(session) = state
            .sessions
            .get(session_id)
            .await
            .map_err(|e| map_err(&e))?
        {
            if !ctx.is_super_admin && session.tenant_id != ctx.tenant_id.get() {
                return Err(forbidden("目标会话不在当前租户"));
            }
            state
                .sessions
                .revoke(session_id)
                .await
                .map_err(|e| map_err(&e))?;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// 会话历史查询（`sys_user_online` 快照，「历史审计读快照表」）。
#[derive(Debug, Deserialize, IntoParams)]
pub struct SessionHistoryQuery {
    pub user_name: Option<String>,
    pub ip: Option<String>,
    /// 0 在线 / 1 已退出；留空不过滤。
    pub status: Option<i16>,
    /// `login_time` 下界（RFC 3339）。
    pub begin_time: Option<String>,
    /// `login_time` 上界（RFC 3339）。
    pub end_time: Option<String>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    /// 排序列名（排序白名单，见 `crate::sort`）；不在清单里则忽略。
    pub sort_by: Option<String>,
    /// `asc` 升序，其余按 `desc`。
    pub sort_order: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OnlineSessionVo {
    pub session_id: String,
    pub tenant_id: i64,
    pub user_id: i64,
    pub user_name: String,
    pub ip: String,
    pub user_agent: String,
    pub login_time: String,
    pub last_access_time: String,
    /// 0 在线 / 1 已退出。
    pub status: i16,
    /// 退出时刻；仍在线为 `null`。
    pub end_time: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OnlineSessionPage {
    pub items: Vec<OnlineSessionVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

impl From<UserOnlineModel> for OnlineSessionVo {
    fn from(m: UserOnlineModel) -> Self {
        Self {
            session_id: m.session_id,
            tenant_id: m.tenant_id,
            user_id: m.user_id,
            user_name: m.user_name,
            ip: m.ip,
            user_agent: m.user_agent,
            login_time: m.login_time.to_rfc3339(),
            last_access_time: m.last_access_time.to_rfc3339(),
            status: m.status,
            end_time: m.end_time.map(|t| t.to_rfc3339()),
        }
    }
}

fn apply_session_filters<S: QueryState>(
    mut query: ScopedQuery<UserOnlineEntity, S>,
    q: &SessionHistoryQuery,
) -> ScopedQuery<UserOnlineEntity, S> {
    if let Some(name) = q.user_name.as_deref().filter(|v| !v.is_empty()) {
        query = query.filter(UserOnlineColumn::UserName.fuzzy(name));
    }
    if let Some(ip) = q.ip.as_deref().filter(|v| !v.is_empty()) {
        query = query.filter(UserOnlineColumn::Ip.eq(ip));
    }
    if let Some(status) = q.status {
        query = query.filter(UserOnlineColumn::Status.eq(status));
    }
    if let Some(begin) = crate::logs::parse_time(&q.begin_time) {
        query = query.filter(UserOnlineColumn::LoginTime.gte(begin));
    }
    if let Some(end) = crate::logs::parse_time(&q.end_time) {
        query = query.filter(UserOnlineColumn::LoginTime.lte(end));
    }
    query
}

/// `sys_user_online` 快照列表允许排序的列（排序白名单）。
///
/// 只放「列表页真的会点排序」的列：每多一个可排序列，就多一条能被利用的
/// `ORDER BY` 路径（大表上非索引列排序会把排序成本推给每一次翻页）。
const ONLINE_SESSION_SORT: crate::sort::SortSpec = crate::sort::SortSpec::new(&[
    ("user_name", "sys_user_online", "user_name"),
    ("ip", "sys_user_online", "ip"),
    ("status", "sys_user_online", "status"),
    ("login_time", "sys_user_online", "login_time"),
    ("last_access_time", "sys_user_online", "last_access_time"),
]);

#[utoipa::path(
    get,
    path = "/api/v1/monitor/online-sessions",
    params(SessionHistoryQuery),
    responses((status = 200, description = "会话历史分页（快照表）", body = OnlineSessionPage))
)]
#[require_perm("monitor:online:history")]
pub async fn list_online_sessions(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<SessionHistoryQuery>,
) -> Result<Json<OnlineSessionPage>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let scope = ctx.scope();
    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;

    let paged = apply_session_filters(ScopedQuery::<UserOnlineEntity>::open().scoped(&scope), &q)
        // 排序白名单：只认这张表声明过的列，客户端字符串进不了标识符位置。
        // 显式排序必须排在前面才真的生效：默认次序退化成同值行的收尾键（login_time），
        // 既不改变「不传 sort_by 时的结果」，又保证翻页不重不漏。
        .sorted_by(
            &ONLINE_SESSION_SORT,
            q.sort_by.as_deref(),
            q.sort_order.as_deref(),
        )
        .order_by_desc(UserOnlineColumn::LoginTime)
        .fetch_page(state.db.conn(), page, page_size)
        .await
        .map_err(db_error)?;
    Ok(Json(OnlineSessionPage {
        items: paged.items.into_iter().map(OnlineSessionVo::from).collect(),
        total: paged.total,
        page: paged.page,
        page_size: paged.page_size,
    }))
}

/// 在线用户模块：权限点与路由一并声明，装配期对账。
pub struct OnlineModule;

impl Module for OnlineModule {
    fn manifest(self: Box<Self>) -> Manifest {
        Manifest::new("cicada:monitor-online")
            .perms([
                Permission::new("monitor:online:list"),
                Permission::new("monitor:online:history"),
                Permission::new("monitor:online:forceLogout"),
                Permission::new("monitor:online:batchForceLogout"),
            ])
            .routes(routes)
    }
}

pub fn routes(_state: AppState) -> Routes {
    Routes::new()
        .route(
            "/api/v1/monitor/online-users",
            get(list_online),
            list_online__perms(),
        )
        .route(
            "/api/v1/monitor/online-users",
            delete(batch_force_logout),
            batch_force_logout__perms(),
        )
        .route(
            "/api/v1/monitor/online-users/{session_id}",
            delete(force_logout),
            force_logout__perms(),
        )
        .route(
            "/api/v1/monitor/online-sessions",
            get(list_online_sessions),
            list_online_sessions__perms(),
        )
}
