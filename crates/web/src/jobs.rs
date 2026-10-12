//! 定时任务管理（M3，权限点 `monitor:job:*`）。
//!
//! 定义存 `sys_job`、执行日志存 `sys_job_log`，两张表属主是 `cicada-jobs`。
//! 调用目标 `invoke_target` 是 **注册表 key**——写库时只做格式校验，真正的
//! handler 由 `cicada-worker` 的 `JobRegistry` 在显式注册时决定，不存在字符串反射。
//!
//! 人工 `/run` 走 `AppState.job_runtime`，仍受任务的并发策略约束。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{delete, get, post, put};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use cicada_jobs::{
    JobActiveModel, JobColumn, JobEntity, JobLogColumn, JobLogEntity, JobLogModel, JobModel,
    validate_cron,
};
use cicada_kernel::{JobId, TenantId, TriggerKind};
use cicada_macros::{audit, require_perm};
use cicada_ports::TriggerOutcome;

use crate::Permission;
use crate::export;
use crate::extractor::RequestContextExt;
use crate::module::{Manifest, Module};
use crate::response::created;
use crate::routes::Routes;
use crate::state::AppState;

use crate::problem::ApiErr;
use crate::search::Fuzzy;
use crate::sort_select::SelectSortExt;

fn err(status: StatusCode, msg: impl Into<String>) -> ApiErr {
    ApiErr::from_status(status, msg)
}

fn unauthorized() -> ApiErr {
    err(StatusCode::UNAUTHORIZED, "未认证")
}

fn not_found() -> ApiErr {
    err(StatusCode::NOT_FOUND, "资源不存在")
}

fn bad_request(msg: impl Into<String>) -> ApiErr {
    err(StatusCode::UNPROCESSABLE_ENTITY, msg)
}

fn conflict(msg: impl Into<String>) -> ApiErr {
    err(StatusCode::CONFLICT, msg)
}

fn db_error(e: impl std::fmt::Display) -> ApiErr {
    tracing::error!(error = %e, "定时任务模块数据库错误");
    err(
        StatusCode::INTERNAL_SERVER_ERROR,
        "定时任务操作失败，请稍后重试",
    )
}

pub const PERMS: &[&str] = &[
    "monitor:job:list",
    "monitor:job:add",
    "monitor:job:edit",
    "monitor:job:remove",
    "monitor:job:changeStatus",
    "monitor:job:detail",
    "monitor:job:export",
];

fn default_group() -> String {
    "DEFAULT".to_string()
}

fn default_concurrency() -> i16 {
    1
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct JobListQuery {
    pub job_name: Option<String>,
    pub job_group: Option<String>,
    pub status: Option<i16>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    /// 排序列名（排序白名单，见 `crate::sort`）；不在清单里则忽略。
    pub sort_by: Option<String>,
    /// `asc` 升序，其余按 `desc`。
    pub sort_order: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct JobVo {
    pub job_id: i64,
    pub job_name: String,
    pub job_group: String,
    pub invoke_target: String,
    pub cron_expression: String,
    /// 并发策略：0 允许 / 1 禁止 / 2 队列。
    pub concurrency: i16,
    pub timeout_secs: i64,
    pub max_retry: i32,
    /// 0 正常 / 1 暂停。
    pub status: i16,
    pub remark: Option<String>,
    pub create_time: String,
}

impl From<JobModel> for JobVo {
    fn from(m: JobModel) -> Self {
        Self {
            job_id: m.job_id,
            job_name: m.job_name,
            job_group: m.job_group,
            invoke_target: m.invoke_target,
            cron_expression: m.cron_expression,
            concurrency: m.concurrency,
            timeout_secs: m.timeout_secs,
            max_retry: m.max_retry,
            status: m.status,
            remark: m.remark,
            create_time: m.create_time.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct JobPage {
    pub items: Vec<JobVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct JobUpsert {
    pub job_name: String,
    #[serde(default = "default_group")]
    pub job_group: String,
    pub invoke_target: String,
    pub cron_expression: String,
    #[serde(default = "default_concurrency")]
    pub concurrency: i16,
    #[serde(default)]
    pub timeout_secs: i64,
    #[serde(default)]
    pub max_retry: i32,
    #[serde(default)]
    pub status: i16,
    pub remark: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct JobStatusUpdate {
    /// 0 正常 / 1 暂停。
    pub status: i16,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct JobLogVo {
    pub job_log_id: i64,
    pub job_id: i64,
    pub job_name: String,
    pub job_group: String,
    pub invoke_target: String,
    pub instance_id: String,
    /// 0 调度触发 / 1 手动触发。
    pub trigger_kind: i16,
    pub start_time: String,
    pub end_time: Option<String>,
    pub duration_ms: Option<i64>,
    /// 0 成功 / 1 失败。
    pub status: i16,
    pub result: Option<String>,
    pub trace_id: Option<String>,
}

impl From<JobLogModel> for JobLogVo {
    fn from(m: JobLogModel) -> Self {
        Self {
            job_log_id: m.job_log_id,
            job_id: m.job_id,
            job_name: m.job_name,
            job_group: m.job_group,
            invoke_target: m.invoke_target,
            instance_id: m.instance_id,
            trigger_kind: m.trigger_kind,
            start_time: m.start_time.to_rfc3339(),
            end_time: m.end_time.map(|t| t.to_rfc3339()),
            duration_ms: m.duration_ms,
            status: m.status,
            result: m.result,
            trace_id: m.trace_id,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct JobLogPage {
    pub items: Vec<JobLogVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

/// 人工触发的结果：`executed` / `enqueued` / `skipped`。
#[derive(Debug, Serialize, ToSchema)]
pub struct JobRunResult {
    pub outcome: String,
    pub success: Option<bool>,
    pub duration_ms: Option<i64>,
    pub message: Option<String>,
}

impl From<TriggerOutcome> for JobRunResult {
    fn from(o: TriggerOutcome) -> Self {
        match o {
            TriggerOutcome::Executed {
                success,
                duration_ms,
                message,
            } => Self {
                outcome: "executed".to_string(),
                success: Some(success),
                duration_ms: Some(duration_ms),
                message,
            },
            TriggerOutcome::Enqueued => Self {
                outcome: "enqueued".to_string(),
                success: None,
                duration_ms: None,
                message: None,
            },
            TriggerOutcome::Skipped { .. } => Self {
                outcome: "skipped".to_string(),
                success: None,
                duration_ms: None,
                message: Some("上一次执行尚未结束（并发策略为禁止并发）".to_string()),
            },
        }
    }
}

fn validate_upsert(req: &JobUpsert) -> Result<(), String> {
    if req.job_name.trim().is_empty() {
        return Err("任务名称不能为空".into());
    }
    if req.invoke_target.trim().is_empty() {
        return Err("调用目标不能为空".into());
    }
    if !(0..=2).contains(&req.concurrency) {
        return Err("并发策略取值必须是 0/1/2".into());
    }
    if !(0..=1).contains(&req.status) {
        return Err("任务状态取值必须是 0/1".into());
    }
    if req.timeout_secs < 0 || req.max_retry < 0 {
        return Err("超时与重试次数不能为负".into());
    }
    validate_cron(&req.cron_expression)
}

async fn load_job(state: &AppState, tenant: TenantId, job_id: i64) -> Result<JobModel, ApiErr> {
    JobEntity::find_by_id(job_id)
        .filter(JobColumn::TenantId.eq(tenant.get()))
        .filter(JobColumn::DeletedAt.is_null())
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)
}

/// `sys_job` 列表允许排序的列（排序白名单）。
///
/// 只放「列表页真的会点排序」的列：每多一个可排序列，就多一条能被利用的
/// `ORDER BY` 路径（大表上非索引列排序会把排序成本推给每一次翻页）。
const JOB_SORT: crate::sort::SortSpec = crate::sort::SortSpec::new(&[
    ("job_name", "sys_job", "job_name"),
    ("job_group", "sys_job", "job_group"),
    ("status", "sys_job", "status"),
    ("create_time", "sys_job", "create_time"),
]);

#[utoipa::path(
    get,
    path = "/api/v1/monitor/jobs",
    params(JobListQuery),
    responses((status = 200, description = "定时任务分页", body = JobPage))
)]
#[require_perm("monitor:job:list")]
pub async fn list_jobs(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<JobListQuery>,
) -> Result<Json<JobPage>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let mut query = JobEntity::find()
        .filter(JobColumn::TenantId.eq(ctx.tenant_id.get()))
        .filter(JobColumn::DeletedAt.is_null());
    if let Some(name) = q.job_name.filter(|s| !s.is_empty()) {
        query = query.filter(JobColumn::JobName.fuzzy(&name));
    }
    if let Some(group) = q.job_group.filter(|s| !s.is_empty()) {
        query = query.filter(JobColumn::JobGroup.eq(group));
    }
    if let Some(status) = q.status {
        query = query.filter(JobColumn::Status.eq(status));
    }
    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;
    let paginator = query
        // 排序白名单：只认这张表声明过的列，客户端字符串进不了标识符位置。
        // 显式排序必须排在前面才真的生效：默认次序退化成同值行的收尾键（job_id），
        // 既不改变「不传 sort_by 时的结果」，又保证翻页不重不漏。
        .sorted_by(&JOB_SORT, q.sort_by.as_deref(), q.sort_order.as_deref())
        .order_by_asc(JobColumn::JobId)
        .paginate(state.db.conn(), page_size);
    let total = paginator.num_items().await.map_err(db_error)?;
    let items = paginator
        .fetch_page(page - 1)
        .await
        .map_err(db_error)?
        .into_iter()
        .map(JobVo::from)
        .collect();
    Ok(Json(JobPage {
        items,
        total,
        page,
        page_size,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/monitor/jobs/export",
    params(JobListQuery),
    responses((status = 200, description = "定时任务导出 xlsx"))
)]
#[require_perm("monitor:job:export")]
#[audit(module = "monitor", action = "job:export", kind = "export")]
pub async fn export_jobs(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<JobListQuery>,
) -> Result<Response, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let tenant = ctx.tenant_id.get();
    let name = q.job_name.filter(|s| !s.is_empty());
    let group = q.job_group.filter(|s| !s.is_empty());
    let status = q.status;

    let headers = &[
        "任务编号",
        "任务名称",
        "任务组",
        "调用目标",
        "cron 表达式",
        "并发策略",
        "超时(秒)",
        "重试次数",
        "状态",
        "备注",
        "创建时间",
    ];
    let widths = &[
        12.0, 24.0, 14.0, 30.0, 20.0, 12.0, 10.0, 10.0, 8.0, 30.0, 26.0,
    ];

    let bytes = export::build("定时任务", headers, widths, |sheet| {
        Box::pin(async move {
            let conn = state.db.conn().clone();
            let mut row: u32 = 1;
            let mut offset: u64 = 0;
            while u64::from(row) - 1 < export::MAX_EXPORT_ROWS {
                let mut query = JobEntity::find()
                    .filter(JobColumn::TenantId.eq(tenant))
                    .filter(JobColumn::DeletedAt.is_null());
                if let Some(n) = &name {
                    query = query.filter(JobColumn::JobName.fuzzy(n));
                }
                if let Some(g) = &group {
                    query = query.filter(JobColumn::JobGroup.eq(g));
                }
                if let Some(s) = status {
                    query = query.filter(JobColumn::Status.eq(s));
                }
                let batch = query
                    .order_by_asc(JobColumn::JobId)
                    .limit(export::EXPORT_BATCH)
                    .offset(offset)
                    .all(&conn)
                    .await?;
                if batch.is_empty() {
                    break;
                }
                for j in &batch {
                    sheet.write_string(row, 0, j.job_id.to_string())?;
                    sheet.write_string(row, 1, &j.job_name)?;
                    sheet.write_string(row, 2, &j.job_group)?;
                    sheet.write_string(row, 3, &j.invoke_target)?;
                    sheet.write_string(row, 4, &j.cron_expression)?;
                    sheet.write_string(row, 5, concurrency_label(j.concurrency))?;
                    sheet.write_string(row, 6, j.timeout_secs.to_string())?;
                    sheet.write_string(row, 7, j.max_retry.to_string())?;
                    sheet.write_string(row, 8, if j.status == 0 { "正常" } else { "暂停" })?;
                    sheet.write_string(row, 9, j.remark.clone().unwrap_or_default())?;
                    sheet.write_string(row, 10, j.create_time.to_rfc3339())?;
                    row += 1;
                }
                offset += batch.len() as u64;
            }
            Ok(())
        })
    })
    .await
    .map_err(db_error)?;

    Ok(export::xlsx_response("jobs.xlsx", bytes))
}

fn concurrency_label(code: i16) -> &'static str {
    match code {
        0 => "允许并发",
        2 => "队列",
        _ => "禁止并发",
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/monitor/jobs/{job_id}",
    params(("job_id" = i64, Path, description = "任务 id")),
    responses((status = 200, description = "任务详情", body = JobVo), (status = 404, description = "不存在"))
)]
#[require_perm("monitor:job:detail")]
pub async fn get_job(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(job_id): Path<i64>,
) -> Result<Json<JobVo>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let row = load_job(&state, ctx.tenant_id, job_id).await?;
    Ok(Json(JobVo::from(row)))
}

#[utoipa::path(
    get,
    path = "/api/v1/monitor/jobs/{job_id}/logs",
    params(("job_id" = i64, Path, description = "任务 id"), ("page" = Option<u64>, Query, description = "页码"), ("page_size" = Option<u64>, Query, description = "每页条数")),
    responses((status = 200, description = "任务执行日志分页", body = JobLogPage))
)]
#[require_perm("monitor:job:detail")]
pub async fn list_job_logs(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(job_id): Path<i64>,
    Query(q): Query<JobListQuery>,
) -> Result<Json<JobLogPage>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;
    let paginator = JobLogEntity::find()
        .filter(JobLogColumn::TenantId.eq(ctx.tenant_id.get()))
        .filter(JobLogColumn::JobId.eq(job_id))
        .order_by_desc(JobLogColumn::JobLogId)
        .paginate(state.db.conn(), page_size);
    let total = paginator.num_items().await.map_err(db_error)?;
    let items = paginator
        .fetch_page(page - 1)
        .await
        .map_err(db_error)?
        .into_iter()
        .map(JobLogVo::from)
        .collect();
    Ok(Json(JobLogPage {
        items,
        total,
        page,
        page_size,
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/monitor/jobs",
    request_body = JobUpsert,
    responses((status = 201, description = "已创建", body = JobVo), (status = 409, description = "同名任务重复"))
)]
#[require_perm("monitor:job:add")]
#[audit(module = "monitor", action = "job:add", kind = "insert")]
pub async fn create_job(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<JobUpsert>,
) -> Result<(StatusCode, HeaderMap, Json<JobVo>), ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    validate_upsert(&req).map_err(bad_request)?;

    let dup = JobEntity::find()
        .filter(JobColumn::TenantId.eq(ctx.tenant_id.get()))
        .filter(JobColumn::JobName.eq(req.job_name.trim()))
        .filter(JobColumn::JobGroup.eq(req.job_group.trim()))
        .filter(JobColumn::DeletedAt.is_null())
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .is_some();
    if dup {
        return Err(conflict("同任务组下任务名称已存在"));
    }

    let row = JobActiveModel {
        tenant_id: Set(ctx.tenant_id.get()),
        job_name: Set(req.job_name.trim().to_string()),
        job_group: Set(req.job_group.trim().to_string()),
        invoke_target: Set(req.invoke_target.trim().to_string()),
        cron_expression: Set(req.cron_expression.trim().to_string()),
        concurrency: Set(req.concurrency),
        timeout_secs: Set(req.timeout_secs),
        max_retry: Set(req.max_retry),
        status: Set(req.status),
        remark: Set(req.remark.clone()),
        create_by: Set(Some(principal.user_name.clone())),
        create_time: Set(Utc::now()),
        ..Default::default()
    }
    .insert(state.db.conn())
    .await
    .map_err(db_error)?;
    let _ = state.job_runtime.sync().await;
    Ok(created(
        format!("/api/v1/monitor/jobs/{}", row.job_id),
        JobVo::from(row),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/monitor/jobs/{job_id}",
    params(("job_id" = i64, Path, description = "任务 id")),
    request_body = JobUpsert,
    responses((status = 200, description = "已更新", body = JobVo), (status = 404, description = "不存在"))
)]
#[require_perm("monitor:job:edit")]
#[audit(module = "monitor", action = "job:edit", kind = "update")]
pub async fn update_job(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(job_id): Path<i64>,
    Json(req): Json<JobUpsert>,
) -> Result<Json<JobVo>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    validate_upsert(&req).map_err(bad_request)?;
    let existing = load_job(&state, ctx.tenant_id, job_id).await?;

    // 留痕快照（审计设计）：调度配置变更直接影响线上执行节奏，必须可追溯。
    let before = serde_json::json!({
        "job_name": existing.job_name,
        "job_group": existing.job_group,
        "invoke_target": existing.invoke_target,
        "cron_expression": existing.cron_expression,
        "concurrency": existing.concurrency,
        "timeout_secs": existing.timeout_secs,
        "max_retry": existing.max_retry,
        "status": existing.status,
        "remark": existing.remark,
    });

    let row = JobActiveModel {
        job_id: Set(existing.job_id),
        job_name: Set(req.job_name.trim().to_string()),
        job_group: Set(req.job_group.trim().to_string()),
        invoke_target: Set(req.invoke_target.trim().to_string()),
        cron_expression: Set(req.cron_expression.trim().to_string()),
        concurrency: Set(req.concurrency),
        timeout_secs: Set(req.timeout_secs),
        max_retry: Set(req.max_retry),
        status: Set(req.status),
        remark: Set(req.remark.clone()),
        update_by: Set(Some(principal.user_name.clone())),
        update_time: Set(Some(Utc::now())),
        ..Default::default()
    }
    .update(state.db.conn())
    .await
    .map_err(db_error)?;
    crate::audit::record_change(
        &state.audit,
        &ctx,
        "sys_job",
        job_id.to_string(),
        before,
        serde_json::json!({
            "job_name": row.job_name,
            "job_group": row.job_group,
            "invoke_target": row.invoke_target,
            "cron_expression": row.cron_expression,
            "concurrency": row.concurrency,
            "timeout_secs": row.timeout_secs,
            "max_retry": row.max_retry,
            "status": row.status,
            "remark": row.remark,
        }),
    )
    .await;
    let _ = state.job_runtime.sync().await;
    Ok(Json(JobVo::from(row)))
}

#[utoipa::path(
    put,
    path = "/api/v1/monitor/jobs/{job_id}/status",
    params(("job_id" = i64, Path, description = "任务 id")),
    request_body = JobStatusUpdate,
    responses((status = 200, description = "已更新", body = JobVo))
)]
#[require_perm("monitor:job:changeStatus")]
#[audit(module = "monitor", action = "job:changeStatus", kind = "update")]
pub async fn change_job_status(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(job_id): Path<i64>,
    Json(req): Json<JobStatusUpdate>,
) -> Result<Json<JobVo>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    if !(0..=1).contains(&req.status) {
        return Err(bad_request("任务状态取值必须是 0/1"));
    }
    let existing = load_job(&state, ctx.tenant_id, job_id).await?;
    let row = JobActiveModel {
        job_id: Set(existing.job_id),
        status: Set(req.status),
        update_by: Set(ctx.principal.as_ref().map(|p| p.user_name.clone())),
        update_time: Set(Some(Utc::now())),
        ..Default::default()
    }
    .update(state.db.conn())
    .await
    .map_err(db_error)?;
    let _ = state.job_runtime.sync().await;
    Ok(Json(JobVo::from(row)))
}

#[utoipa::path(
    put,
    path = "/api/v1/monitor/jobs/{job_id}/run",
    params(("job_id" = i64, Path, description = "任务 id")),
    responses((status = 200, description = "触发结果", body = JobRunResult))
)]
#[require_perm("monitor:job:changeStatus")]
#[audit(module = "monitor", action = "job:run", kind = "other")]
pub async fn run_job(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(job_id): Path<i64>,
) -> Result<Json<JobRunResult>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    // 确认任务存在且属于本租户，再交给调度后端。
    load_job(&state, ctx.tenant_id, job_id).await?;
    let outcome = state
        .job_runtime
        .submit(ctx.tenant_id, JobId::new(job_id), TriggerKind::Manual)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "人工触发任务失败");
            err(StatusCode::INTERNAL_SERVER_ERROR, "触发任务失败")
        })?;
    Ok(Json(JobRunResult::from(outcome)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/monitor/jobs/{job_id}",
    params(("job_id" = i64, Path, description = "任务 id")),
    responses((status = 204, description = "已删除"), (status = 404, description = "不存在"))
)]
#[require_perm("monitor:job:remove")]
#[audit(module = "monitor", action = "job:remove", kind = "delete")]
pub async fn delete_job(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(job_id): Path<i64>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let existing = load_job(&state, ctx.tenant_id, job_id).await?;
    JobActiveModel {
        job_id: Set(existing.job_id),
        deleted_at: Set(Some(Utc::now())),
        update_by: Set(ctx.principal.as_ref().map(|p| p.user_name.clone())),
        update_time: Set(Some(Utc::now())),
        ..Default::default()
    }
    .update(state.db.conn())
    .await
    .map_err(db_error)?;
    let _ = state.job_runtime.sync().await;
    Ok(StatusCode::NO_CONTENT)
}

/// 定时任务模块：`monitor:job:*` 七个权限点与路由一并声明，装配期双向对账。
pub struct JobModule;

impl Module for JobModule {
    fn manifest(self: Box<Self>) -> Manifest {
        let perms: Vec<Permission> = PERMS.iter().map(|p| Permission::new(*p)).collect();
        Manifest::new("cicada:monitor-jobs")
            .owns([
                crate::module::TableName("sys_job"),
                crate::module::TableName("sys_job_log"),
            ])
            .perms(perms)
            .routes(routes)
    }
}

pub fn routes(_state: AppState) -> Routes {
    Routes::new()
        .route("/api/v1/monitor/jobs", get(list_jobs), list_jobs__perms())
        .route_audited(
            "/api/v1/monitor/jobs/export",
            get(export_jobs),
            export_jobs__perms(),
            export_jobs__audit(),
        )
        .route(
            "/api/v1/monitor/jobs/{job_id}",
            get(get_job),
            get_job__perms(),
        )
        .route(
            "/api/v1/monitor/jobs/{job_id}/logs",
            get(list_job_logs),
            list_job_logs__perms(),
        )
        .route_audited(
            "/api/v1/monitor/jobs",
            post(create_job),
            create_job__perms(),
            create_job__audit(),
        )
        .route_audited(
            "/api/v1/monitor/jobs/{job_id}",
            put(update_job),
            update_job__perms(),
            update_job__audit(),
        )
        .route_audited(
            "/api/v1/monitor/jobs/{job_id}/status",
            put(change_job_status),
            change_job_status__perms(),
            change_job_status__audit(),
        )
        .route_audited(
            "/api/v1/monitor/jobs/{job_id}/run",
            put(run_job),
            run_job__perms(),
            run_job__audit(),
        )
        .route_audited(
            "/api/v1/monitor/jobs/{job_id}",
            delete(delete_job),
            delete_job__perms(),
            delete_job__audit(),
        )
}
