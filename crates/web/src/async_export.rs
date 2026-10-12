//! 异步导出任务：`system:export:*`。
//!
//! 大表导出不在请求里同步做完：POST 落 `sys_export_task`（含数据范围的条件快照，
//! 保证后台执行与发起请求时的可见范围一致），`tokio::spawn` 就地跑注册表里的执行器，
//! 产物走文件模块链路落 `sys_file`，完成后由任务下载端点给回 xlsx。
//! 执行器是**注册表 key**（白名单枚举，同定时任务的调用目标），不接受反射命名。

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicI64, Ordering};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use cicada_kernel::scope::Query as ScopedQuery;
use cicada_macros::{audit, require_perm};
use cicada_notify::{ChannelColumn, ChannelEntity, SendRequest};
use cicada_system::entity::{
    ExportActiveModel, ExportColumn, ExportEntity, ExportModel, FileActiveModel, FileColumn,
    FileEntity, UserColumn, UserEntity,
};
use cicada_system::{
    EXPORT_STATUS_DONE, EXPORT_STATUS_FAILED, EXPORT_STATUS_PENDING, EXPORT_STATUS_RUNNING,
};

use crate::export;
use crate::extractor::RequestContextExt;
use crate::file::storage_error;
use crate::module::{Manifest, Module};
use crate::response::created;
use crate::routes::Routes;
use crate::sort::SortExt;
use crate::state::AppState;
use crate::system::user::{UserExportFilters, build_users_xlsx};
use crate::system::{ApiErr, bad_request, conflict, db_error, not_found, unauthorized};

pub const PERMS: &[&str] = &["system:export:list", "system:export:add"];

#[derive(Debug, Deserialize, IntoParams)]
pub struct ExportTaskListQuery {
    pub module: Option<String>,
    pub status: Option<i16>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    /// 排序列名（排序白名单，见 `crate::sort`）；不在清单里则忽略。
    pub sort_by: Option<String>,
    /// `asc` 升序，其余按 `desc`。
    pub sort_order: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ExportRequest {
    /// 导出模块标识（注册表 key，见 `executors`）。
    pub module: String,
    /// 过滤条件快照；缺省 = 无过滤（仍受数据范围约束）。
    #[serde(default)]
    pub query: serde_json::Value,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExportTaskVo {
    pub id: i64,
    pub module: String,
    /// 发起时的条件快照（含数据范围），JSON 文本透出。
    pub query: serde_json::Value,
    /// 0 Pending / 1 Running / 2 Done / 3 Failed。
    pub status: i16,
    pub file_id: Option<i64>,
    pub total: i64,
    pub processed: i64,
    pub error: Option<String>,
    pub finish_time: Option<String>,
    pub create_time: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExportTaskPage {
    pub items: Vec<ExportTaskVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

impl From<ExportModel> for ExportTaskVo {
    fn from(m: ExportModel) -> Self {
        Self {
            id: m.id,
            module: m.module,
            query: m.query,
            status: m.status,
            file_id: m.file_id,
            total: m.total,
            processed: m.processed,
            error: m.error,
            finish_time: m.finish_time.map(|t| t.to_rfc3339()),
            create_time: m.create_time.to_rfc3339(),
        }
    }
}

/// `sys_export_task.query` 的固定信封：条件 + 发起请求的数据范围快照（审计日志表）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub scope: cicada_kernel::Scope,
    pub filters: serde_json::Value,
}

/// 进度回调：已写行数单调递增；执行器按需调用，`run_task` 只记内存计数。
pub(crate) type Progress = Box<dyn FnMut(i64) + Send + 'static>;
/// 执行器签名：连接 + 快照（均按值，任务需在 `tokio::spawn` 里独立存活）+ 进度回调。
pub(crate) type ExecutorFn =
    fn(
        sea_orm::DatabaseConnection,
        Snapshot,
        Progress,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, export::ExportError>> + Send + 'static>>;

/// 模块注册表（白名单，「调用目标不是字符串反射」的同一原则）。
fn executor(module: &str) -> Option<ExecutorFn> {
    match module {
        "system:user" => Some(run_system_user),
        _ => None,
    }
}

fn run_system_user(
    conn: sea_orm::DatabaseConnection,
    snap: Snapshot,
    progress: Progress,
) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, export::ExportError>> + Send + 'static>> {
    let filters: UserExportFilters =
        serde_json::from_value(snap.filters.clone()).unwrap_or_default();
    Box::pin(async move { build_users_xlsx(conn, snap.scope, filters, progress).await })
}

/// 执行器模块标识清单（错误提示与文档用）。
const KNOWN_MODULES: &[&str] = &["system:user"];

/// 导出完成通知的模板码。
///
/// 种子里另有一份同值常量（`cicada_migrations::seed::EXPORT_DONE_TEMPLATE_CODE`）：
/// `web`（L4）不依赖 `migrations`，两边各写一次、由
/// `crates/web/tests/async_export.rs` 断言一致——同审计设计里框架任务目标字符串的处理。
pub const EXPORT_DONE_TEMPLATE_CODE: &str = "export.done";

async fn set_status(
    state: &AppState,
    id: i64,
    fields: impl FnOnce(&mut ExportActiveModel),
) -> Result<(), sea_orm::DbErr> {
    let mut am = ExportActiveModel {
        id: Set(id),
        ..Default::default()
    };
    fields(&mut am);
    let _ = am.update(state.db.conn()).await;
    Ok(())
}

/// 该租户第一个启用的 `email` 渠道。
///
/// 只按 `tenant_id + kind + enabled` 挑，不套数据范围：渠道是系统配置，不是业务数据，
/// 「谁能看渠道」与该不该收到导出通知是两件事（缓存读接口另有权限点）。
async fn first_enabled_email_channel(state: &AppState, tenant_id: i64) -> Option<String> {
    ChannelEntity::find()
        .filter(ChannelColumn::TenantId.eq(tenant_id))
        .filter(ChannelColumn::Kind.eq("email"))
        .filter(ChannelColumn::Enabled.eq(true))
        .order_by_asc(ChannelColumn::Id)
        .one(state.db.conn())
        .await
        .ok()
        .flatten()
        .map(|c| c.code)
}

/// 导入导出「大表走异步任务，完成后经通知中心给下载链接」的那一次投递。
///
/// 三种「送不出去」都只记日志、**不改任务状态**：没有归属用户、创建者没填邮箱、
/// 租户还没配邮件渠道（渠道带凭据，按配置的边界不进种子，只能由运营建）。
/// 理由与审计设计「审计写入不阻塞主流程」同一条：产物已经落 `sys_file`，
/// 列表页与下载端点照常工作，一条送不出去的邮件不该把成功的导出改成 failed。
///
/// 收件人查询只按 `tenant_id + user_id` 过滤，不套 `snap.scope` 的数据范围：
/// 这是「给动作的发起者回话」，不是业务取数——数据范围把发起者自己筛掉了，
/// 通知就永远发不出去，而这恰恰是最需要它的场景（大表导出的人往往看不到全表）。
async fn notify_export_done(
    state: &AppState,
    snap: &Snapshot,
    module: &str,
    task_id: i64,
    processed: i64,
) {
    let Some(user_id) = snap.scope.user_id else {
        tracing::debug!(task_id, "导出任务没有归属用户，跳过完成通知");
        return;
    };
    let creator = UserEntity::find()
        .filter(UserColumn::TenantId.eq(snap.scope.tenant_id.get()))
        .filter(UserColumn::UserId.eq(user_id.get()))
        .one(state.db.conn())
        .await
        .ok()
        .flatten();
    let Some(email) = creator
        .as_ref()
        .and_then(|u| u.email.clone())
        .filter(|e| !e.trim().is_empty())
    else {
        tracing::debug!(
            task_id,
            user_id = user_id.get(),
            "创建者没有邮箱，跳过完成通知"
        );
        return;
    };
    let Some(channel_code) = first_enabled_email_channel(state, snap.scope.tenant_id.get()).await
    else {
        tracing::info!(
            task_id,
            tenant = snap.scope.tenant_id.get(),
            "租户没有启用的邮件渠道，导出完成通知未发送（下载链接见任务列表）"
        );
        return;
    };
    let req = SendRequest {
        channel_code,
        template_code: EXPORT_DONE_TEMPLATE_CODE.to_string(),
        recipient: email,
        locale: None,
        payload: serde_json::json!({
            "task_id": task_id,
            "module": module,
            "processed": processed,
        }),
        operator: creator.map(|u| u.user_name),
    };
    match state.notify.send(snap.scope.tenant_id, req).await {
        Ok(outcome) => tracing::info!(
            task_id,
            record_id = outcome.record_id,
            status = outcome.status,
            "导出完成通知已投递"
        ),
        Err(e) => tracing::warn!(error = %e, task_id, "导出完成通知投递失败"),
    }
}

/// 后台执行体：Running → 执行 → 产物落 `sys_file` → Done（失败落 Failed + 原因）。
async fn run_task(state: AppState, task_id: i64, module: String, snap: Snapshot) {
    if let Err(e) = set_status(&state, task_id, |am| {
        am.status = Set(EXPORT_STATUS_RUNNING);
        am.update_time = Set(Some(chrono::Utc::now()));
    })
    .await
    {
        tracing::error!(error = %e, task_id, "异步导出状态推进失败");
        return;
    }
    // 进度回调只写内存计数，避免每批一行 UPDATE 打爆连接池；结束时统一落库。
    let rows = std::sync::Arc::new(AtomicI64::new(0));
    let progress: Progress = {
        let rows_weak = std::sync::Arc::downgrade(&rows);
        Box::new(move |processed: i64| {
            if let Some(rows) = rows_weak.upgrade() {
                rows.store(processed, Ordering::Relaxed);
            }
        })
    };
    let result = match executor(&module) {
        Some(run) => run(state.db.conn().clone(), snap.clone(), progress).await,
        None => Err(export::ExportError::Db(format!(
            "未知的导出模块：{module}（可用：{KNOWN_MODULES:?}）"
        ))),
    };
    match result {
        Ok(bytes) => {
            let finish = chrono::Utc::now();
            let original_name = format!("{module}_async_export_{task_id}.xlsx");
            let path = cicada_storage::object_path(snap.scope.tenant_id.get(), &original_name);
            match state.storage.put(&path, &bytes).await {
                Ok(()) => {
                    let saved = FileActiveModel {
                        tenant_id: Set(snap.scope.tenant_id.get()),
                        storage: Set(state.storage.driver().to_string()),
                        path: Set(path),
                        original_name: Set(original_name),
                        size: Set(bytes.len() as i64),
                        mime: Set(cicada_storage::detect_mime(&bytes).to_string()),
                        sha256: Set(cicada_storage::sha256_hex(&bytes)),
                        biz_type: Set(Some("export_task".to_string())),
                        biz_id: Set(Some(task_id)),
                        create_time: Set(finish),
                        ..Default::default()
                    }
                    .insert(state.db.conn())
                    .await;
                    match saved {
                        Ok(f) => {
                            let _ = set_status(&state, task_id, |am| {
                                am.status = Set(EXPORT_STATUS_DONE);
                                am.file_id = Set(Some(f.file_id));
                                am.processed = Set(rows.load(Ordering::Relaxed));
                                am.total = Set(rows.load(Ordering::Relaxed));
                                am.finish_time = Set(Some(finish));
                                am.update_time = Set(Some(finish));
                            })
                            .await;
                            // 导入导出：完成后经通知中心给下载链接（送不出去只记日志）。
                            notify_export_done(
                                &state,
                                &snap,
                                &module,
                                task_id,
                                rows.load(Ordering::Relaxed),
                            )
                            .await;
                        }
                        Err(e) => {
                            tracing::error!(error = %e, task_id, "导出产物落 sys_file 失败");
                            let _ = set_status(&state, task_id, |am| {
                                am.status = Set(EXPORT_STATUS_FAILED);
                                am.error = Set(Some(format!("文件元数据写入失败：{e}")));
                                am.finish_time = Set(Some(finish));
                                am.update_time = Set(Some(finish));
                            })
                            .await;
                        }
                    }
                }
                Err(e) => {
                    let _ = set_status(&state, task_id, |am| {
                        am.status = Set(EXPORT_STATUS_FAILED);
                        am.error = Set(Some(format!("存储写入失败：{e}")));
                        am.finish_time = Set(Some(finish));
                        am.update_time = Set(Some(finish));
                    })
                    .await;
                }
            }
        }
        Err(e) => {
            let finish = chrono::Utc::now();
            let _ = set_status(&state, task_id, |am| {
                am.status = Set(EXPORT_STATUS_FAILED);
                am.error = Set(Some(e.to_string()));
                am.finish_time = Set(Some(finish));
                am.update_time = Set(Some(finish));
            })
            .await;
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/system/export-tasks",
    request_body = ExportRequest,
    responses(
        (status = 201, description = "已受理（后台执行）", body = ExportTaskVo),
        (status = 422, description = "未知导出模块或快照非法")
    )
)]
#[require_perm("system:export:add")]
#[audit(module = "system", action = "export-task:add", kind = "insert")]
pub async fn create_export_task(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<ExportRequest>,
) -> Result<(StatusCode, HeaderMap, Json<ExportTaskVo>), ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    if executor(&req.module).is_none() {
        return Err(bad_request(format!(
            "未知的导出模块：{}（可用：{KNOWN_MODULES:?}）",
            req.module
        )));
    }
    let snap = Snapshot {
        scope: ctx.scope(),
        filters: req.query,
    };
    let saved = ExportActiveModel {
        tenant_id: Set(ctx.tenant_id.get()),
        user_id: Set(principal.user_id.get()),
        module: Set(req.module.clone()),
        query: Set(serde_json::to_value(&snap).map_err(|e| bad_request(e.to_string()))?),
        status: Set(EXPORT_STATUS_PENDING),
        create_time: Set(chrono::Utc::now()),
        ..Default::default()
    }
    .insert(state.db.conn())
    .await
    .map_err(db_error)?;
    tokio::spawn(run_task(state.clone(), saved.id, req.module, snap));
    Ok(created(
        format!("/api/v1/system/export-tasks/{}", saved.id),
        ExportTaskVo::from(saved),
    ))
}

/// `sys_export_task` 列表允许排序的列（排序白名单）。
///
/// 只放「列表页真的会点排序」的列：每多一个可排序列，就多一条能被利用的
/// `ORDER BY` 路径（大表上非索引列排序会把排序成本推给每一次翻页）。
const EXPORT_TASK_SORT: crate::sort::SortSpec = crate::sort::SortSpec::new(&[
    ("module", "sys_export_task", "module"),
    ("status", "sys_export_task", "status"),
    ("processed", "sys_export_task", "processed"),
    ("create_time", "sys_export_task", "create_time"),
]);

#[utoipa::path(
    get,
    path = "/api/v1/system/export-tasks",
    params(ExportTaskListQuery),
    responses((status = 200, description = "任务分页（仅本人发起）", body = ExportTaskPage))
)]
#[require_perm("system:export:list")]
pub async fn list_export_tasks(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<ExportTaskListQuery>,
) -> Result<Json<ExportTaskPage>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let mut query = ScopedQuery::<ExportEntity>::open().scoped(&ctx.scope());
    if let Some(m) = q.module.filter(|s| !s.is_empty()) {
        query = query.filter(ExportColumn::Module.eq(m));
    }
    if let Some(s) = q.status {
        query = query.filter(ExportColumn::Status.eq(s));
    }
    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;
    let paged = query
        // 排序白名单：只认这张表声明过的列，客户端字符串进不了标识符位置。
        // 显式排序必须排在前面才真的生效：默认次序退化成同值行的收尾键（id），
        // 既不改变「不传 sort_by 时的结果」，又保证翻页不重不漏。
        .sorted_by(
            &EXPORT_TASK_SORT,
            q.sort_by.as_deref(),
            q.sort_order.as_deref(),
        )
        .order_by_desc(ExportColumn::Id)
        .fetch_page(state.db.conn(), page, page_size)
        .await
        .map_err(db_error)?;
    Ok(Json(ExportTaskPage {
        items: paged.items.into_iter().map(ExportTaskVo::from).collect(),
        total: paged.total,
        page: paged.page,
        page_size: paged.page_size,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/system/export-tasks/{id}",
    params(("id" = i64, Path, description = "任务 id")),
    responses(
        (status = 200, description = "任务状态（轮询进度）", body = ExportTaskVo),
        (status = 404, description = "不存在")
    )
)]
#[require_perm("system:export:list")]
pub async fn get_export_task(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(id): Path<i64>,
) -> Result<Json<ExportTaskVo>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let model = scoped_task(&state, &ctx, id).await?;
    Ok(Json(ExportTaskVo::from(model)))
}

#[utoipa::path(
    get,
    path = "/api/v1/system/export-tasks/{id}/download",
    params(("id" = i64, Path, description = "任务 id")),
    responses(
        (status = 200, description = "产物 xlsx"),
        (status = 404, description = "任务或产物不存在"),
        (status = 409, description = "任务尚未完成")
    )
)]
#[require_perm("system:export:list")]
pub async fn download_export_task(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(id): Path<i64>,
) -> Result<Response, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let task = scoped_task(&state, &ctx, id).await?;
    if task.status != EXPORT_STATUS_DONE {
        return Err(conflict("导出任务尚未完成"));
    }
    let Some(file_id) = task.file_id else {
        return Err(not_found());
    };
    let file = ScopedQuery::<FileEntity>::open()
        .scoped(&ctx.scope())
        .filter(FileColumn::FileId.eq(file_id))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    let bytes = state.storage.get(&file.path).await.map_err(storage_error)?;
    Ok(export::xlsx_response(&file.original_name, bytes))
}

/// 取数据范围内的任务（OwnerOnly：列表/状态/下载都只见自己的任务）。
async fn scoped_task(
    state: &AppState,
    ctx: &cicada_kernel::RequestContext,
    id: i64,
) -> Result<ExportModel, ApiErr> {
    ScopedQuery::<ExportEntity>::open()
        .scoped(&ctx.scope())
        .filter(ExportColumn::Id.eq(id))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)
}

pub struct ExportTaskModule;

impl Module for ExportTaskModule {
    fn manifest(self: Box<Self>) -> Manifest {
        Manifest::new("cicada:system-export")
            .perms(PERMS.iter().map(|p| crate::Permission::new(*p)))
            .routes(routes)
    }
}

pub fn routes(_state: AppState) -> Routes {
    Routes::new()
        .route(
            "/api/v1/system/export-tasks",
            get(list_export_tasks),
            list_export_tasks__perms(),
        )
        .route_audited(
            "/api/v1/system/export-tasks",
            post(create_export_task),
            create_export_task__perms(),
            create_export_task__audit(),
        )
        .route(
            "/api/v1/system/export-tasks/{id}",
            get(get_export_task),
            get_export_task__perms(),
        )
        .route(
            "/api/v1/system/export-tasks/{id}/download",
            get(download_export_task),
            download_export_task__perms(),
        )
}
