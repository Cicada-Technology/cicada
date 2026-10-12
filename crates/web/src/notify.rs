//! 通知中心：渠道 / 模板 / 记录 / 发送。
//!
//! 管理接口是租户内的配置面（`notify:*`）；发送接口走 `NotifyService`：
//! 渲染模板 → 按渠道 kind 投递 → 每次投递留痕（失败也留痕，见 `NotifyService`）。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post, put};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use cicada_kernel::scope::Query as ScopedQuery;
use cicada_macros::{audit, require_perm};
use cicada_notify::entity::{
    ChannelActiveModel, ChannelColumn, ChannelEntity, ChannelModel, RecordColumn, RecordEntity,
    RecordModel, TemplateActiveModel, TemplateColumn, TemplateEntity, TemplateModel,
};
use cicada_notify::{ChannelConfig, ChannelKind, NotifyError, SendOutcome, SendRequest};

use crate::Permission;
use crate::extractor::RequestContextExt;
use crate::module::{Manifest, Module};
use crate::response::created;
use crate::routes::Routes;
use crate::state::AppState;

use crate::problem::{ApiErr, BizCode, Problem};
use crate::search::Fuzzy;
use crate::sort::SortExt;

pub const PERMS: &[&str] = &[
    "notify:channel:list",
    "notify:channel:add",
    "notify:channel:edit",
    "notify:channel:remove",
    "notify:template:list",
    "notify:template:add",
    "notify:template:edit",
    "notify:template:remove",
    "notify:record:list",
    "notify:send",
];

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
    tracing::error!(error = %e, "通知中心数据库错误");
    err(
        StatusCode::INTERNAL_SERVER_ERROR,
        "通知中心操作失败，请稍后重试",
    )
}

// ---------------------------------------------------------------- 渠道

#[derive(Debug, Deserialize, IntoParams)]
pub struct ChannelListQuery {
    pub code: Option<String>,
    pub kind: Option<String>,
    pub enabled: Option<bool>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    /// 排序列名（排序白名单，见 `crate::sort`）；不在清单里则忽略。
    pub sort_by: Option<String>,
    /// `asc` 升序，其余按 `desc`。
    pub sort_order: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ChannelVo {
    pub id: i64,
    pub code: String,
    pub name: String,
    pub kind: String,
    pub config: serde_json::Value,
    pub enabled: bool,
    pub remark: Option<String>,
    pub create_time: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ChannelPage {
    pub items: Vec<ChannelVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ChannelUpsert {
    pub code: String,
    pub name: String,
    /// `sms` / `email` / `webhook` / `im`。
    pub kind: String,
    /// 驱动配置，见 `cicada_notify::ChannelConfig`。
    pub config: serde_json::Value,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub remark: Option<String>,
}

fn default_true() -> bool {
    true
}

impl From<ChannelModel> for ChannelVo {
    fn from(m: ChannelModel) -> Self {
        Self {
            id: m.id,
            code: m.code,
            name: m.name,
            kind: m.kind,
            config: mask_channel_secrets(m.config),
            enabled: m.enabled,
            remark: m.remark,
            create_time: m.create_time.to_rfc3339(),
        }
    }
}

/// 渠道配置里的凭据不回显（只读接口不该变成凭据读取口）。
///
/// 有 `system:notify:list` 的人已经能改渠道，回显口令只会把「能改」放大成
/// 「能拿走」；真要确认配了什么，去密钥管理或环境变量那一侧看。
fn mask_channel_secrets(config: serde_json::Value) -> serde_json::Value {
    const SECRETS: &[&str] = &["token", "password"];
    let mut v = config;
    if let Some(map) = v.as_object_mut() {
        for key in SECRETS {
            if let Some(val) = map.get_mut(*key)
                && val.as_str().is_some_and(|s| !s.is_empty())
            {
                *val = serde_json::Value::String("***".to_string());
            }
        }
    }
    v
}

fn validate_channel(req: &ChannelUpsert) -> Result<ChannelKind, ApiErr> {
    if req.code.trim().is_empty() || req.name.trim().is_empty() {
        return Err(bad_request("渠道编码与名称不能为空"));
    }
    let kind = ChannelKind::parse(&req.kind)
        .ok_or_else(|| bad_request("kind 只能是 sms / email / webhook / im"))?;
    let config: ChannelConfig = serde_json::from_value(req.config.clone())
        .map_err(|e| bad_request(format!("config 非法：{e}")))?;
    // 按渠道类型查必填项（横切能力）：邮件可以直连 SMTP（`host`）也可以走 HTTP 网关，
    // 其余通道只有网关一条路。规则放在 notify 侧，测试与接口用的是同一份。
    config
        .validate_for(kind)
        .map_err(|e| bad_request(format!("config 不完整：{e}")))?;
    Ok(kind)
}

/// `sys_notify_channel` 列表允许排序的列（排序白名单）。
///
/// 只放「列表页真的会点排序」的列：每多一个可排序列，就多一条能被利用的
/// `ORDER BY` 路径（大表上非索引列排序会把排序成本推给每一次翻页）。
const CHANNEL_SORT: crate::sort::SortSpec = crate::sort::SortSpec::new(&[
    ("code", "sys_notify_channel", "code"),
    ("name", "sys_notify_channel", "name"),
    ("kind", "sys_notify_channel", "kind"),
    ("enabled", "sys_notify_channel", "enabled"),
]);

#[utoipa::path(
    get,
    path = "/api/v1/notify/channels",
    params(ChannelListQuery),
    responses((status = 200, description = "渠道分页", body = ChannelPage))
)]
#[require_perm("notify:channel:list")]
pub async fn list_channels(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<ChannelListQuery>,
) -> Result<Json<ChannelPage>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let mut query = ScopedQuery::<ChannelEntity>::open().scoped(&ctx.scope());
    if let Some(code) = q.code.filter(|s| !s.is_empty()) {
        query = query.filter(ChannelColumn::Code.fuzzy(&code));
    }
    if let Some(kind) = q.kind.filter(|s| !s.is_empty()) {
        query = query.filter(ChannelColumn::Kind.eq(kind));
    }
    if let Some(enabled) = q.enabled {
        query = query.filter(ChannelColumn::Enabled.eq(enabled));
    }
    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;
    let paged = query
        // 排序白名单：只认这张表声明过的列，客户端字符串进不了标识符位置。
        // 显式排序必须排在前面才真的生效：默认次序退化成同值行的收尾键（id），
        // 既不改变「不传 sort_by 时的结果」，又保证翻页不重不漏。
        .sorted_by(&CHANNEL_SORT, q.sort_by.as_deref(), q.sort_order.as_deref())
        .order_by(ChannelColumn::Id)
        .fetch_page(state.db.conn(), page, page_size)
        .await
        .map_err(db_error)?;
    Ok(Json(ChannelPage {
        items: paged.items.into_iter().map(ChannelVo::from).collect(),
        total: paged.total,
        page: paged.page,
        page_size: paged.page_size,
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/notify/channels",
    request_body = ChannelUpsert,
    responses((status = 201, description = "已创建", body = ChannelVo), (status = 409, description = "渠道编码重复"))
)]
#[require_perm("notify:channel:add")]
#[audit(module = "notify", action = "channel:add", kind = "insert")]
pub async fn create_channel(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<ChannelUpsert>,
) -> Result<(StatusCode, HeaderMap, Json<ChannelVo>), ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    let kind = validate_channel(&req)?;
    let tenant = ctx.tenant_id.get();
    let taken = ChannelEntity::find()
        .filter(ChannelColumn::TenantId.eq(tenant))
        .filter(ChannelColumn::Code.eq(req.code.trim()))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .is_some();
    if taken {
        return Err(conflict("渠道编码已存在"));
    }
    let row = ChannelActiveModel {
        tenant_id: sea_orm::Set(tenant),
        code: sea_orm::Set(req.code.trim().to_string()),
        name: sea_orm::Set(req.name.trim().to_string()),
        kind: sea_orm::Set(kind.as_str().to_string()),
        config: sea_orm::Set(req.config.clone()),
        enabled: sea_orm::Set(req.enabled),
        remark: sea_orm::Set(req.remark.clone()),
        create_by: sea_orm::Set(Some(principal.user_name.clone())),
        create_time: sea_orm::Set(chrono::Utc::now()),
        ..Default::default()
    }
    .insert(state.db.conn())
    .await
    .map_err(db_error)?;
    Ok(created(
        format!("/api/v1/notify/channels/{}", row.id),
        ChannelVo::from(row),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/notify/channels/{id}",
    request_body = ChannelUpsert,
    params(("id" = i64, Path, description = "渠道 id")),
    responses((status = 200, description = "已更新", body = ChannelVo))
)]
#[require_perm("notify:channel:edit")]
#[audit(module = "notify", action = "channel:edit", kind = "update")]
pub async fn update_channel(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(id): Path<i64>,
    Json(req): Json<ChannelUpsert>,
) -> Result<Json<ChannelVo>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    let kind = validate_channel(&req)?;
    let current = ScopedQuery::<ChannelEntity>::open()
        .scoped(&ctx.scope())
        .filter(ChannelColumn::Id.eq(id))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;

    let row = ChannelActiveModel {
        id: sea_orm::Set(current.id),
        code: sea_orm::Set(req.code.trim().to_string()),
        name: sea_orm::Set(req.name.trim().to_string()),
        kind: sea_orm::Set(kind.as_str().to_string()),
        config: sea_orm::Set(req.config.clone()),
        enabled: sea_orm::Set(req.enabled),
        remark: sea_orm::Set(req.remark.clone()),
        update_by: sea_orm::Set(Some(principal.user_name.clone())),
        update_time: sea_orm::Set(Some(chrono::Utc::now())),
        ..Default::default()
    }
    .update(state.db.conn())
    .await
    .map_err(db_error)?;
    Ok(Json(ChannelVo::from(row)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/notify/channels/{id}",
    params(("id" = i64, Path, description = "渠道 id")),
    responses((status = 204, description = "已删除"))
)]
#[require_perm("notify:channel:remove")]
#[audit(module = "notify", action = "channel:remove", kind = "delete")]
pub async fn delete_channel(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let res = ChannelEntity::delete_many()
        .filter(ChannelColumn::Id.eq(id))
        .filter(ChannelColumn::TenantId.eq(ctx.tenant_id.get()))
        .exec(state.db.conn())
        .await
        .map_err(db_error)?;
    if res.rows_affected == 0 {
        return Err(not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------- 模板

#[derive(Debug, Deserialize, IntoParams)]
pub struct TemplateListQuery {
    pub code: Option<String>,
    pub channel_kind: Option<String>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    /// 排序列名（排序白名单，见 `crate::sort`）；不在清单里则忽略。
    pub sort_by: Option<String>,
    /// `asc` 升序，其余按 `desc`。
    pub sort_order: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TemplateVo {
    pub id: i64,
    pub code: String,
    pub name: String,
    pub channel_kind: String,
    pub locale: String,
    pub title: String,
    pub body: String,
    pub enabled: bool,
    pub remark: Option<String>,
    pub create_time: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TemplatePage {
    pub items: Vec<TemplateVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TemplateUpsert {
    pub code: String,
    pub name: String,
    pub channel_kind: String,
    #[serde(default = "default_locale")]
    pub locale: String,
    pub title: String,
    pub body: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub remark: Option<String>,
}

fn default_locale() -> String {
    "zh-CN".to_string()
}

impl From<TemplateModel> for TemplateVo {
    fn from(m: TemplateModel) -> Self {
        Self {
            id: m.id,
            code: m.code,
            name: m.name,
            channel_kind: m.channel_kind,
            locale: m.locale,
            title: m.title,
            body: m.body,
            enabled: m.enabled,
            remark: m.remark,
            create_time: m.create_time.to_rfc3339(),
        }
    }
}

fn validate_template(req: &TemplateUpsert) -> Result<(), ApiErr> {
    if req.code.trim().is_empty() || req.name.trim().is_empty() {
        return Err(bad_request("模板编码与名称不能为空"));
    }
    if ChannelKind::parse(&req.channel_kind).is_none() {
        return Err(bad_request(
            "channel_kind 只能是 sms / email / webhook / im",
        ));
    }
    // 保存时才校验模板语法，避免坏模板在发送时才发现。
    cicada_notify::validate(&req.title, &req.body).map_err(|e| bad_request(e.to_string()))
}

/// `sys_notify_template` 列表允许排序的列（排序白名单）。
///
/// 只放「列表页真的会点排序」的列：每多一个可排序列，就多一条能被利用的
/// `ORDER BY` 路径（大表上非索引列排序会把排序成本推给每一次翻页）。
const TEMPLATE_SORT: crate::sort::SortSpec = crate::sort::SortSpec::new(&[
    ("code", "sys_notify_template", "code"),
    ("name", "sys_notify_template", "name"),
    ("channel_kind", "sys_notify_template", "channel_kind"),
    ("locale", "sys_notify_template", "locale"),
]);

#[utoipa::path(
    get,
    path = "/api/v1/notify/templates",
    params(TemplateListQuery),
    responses((status = 200, description = "模板分页", body = TemplatePage))
)]
#[require_perm("notify:template:list")]
pub async fn list_templates(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<TemplateListQuery>,
) -> Result<Json<TemplatePage>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let mut query = ScopedQuery::<TemplateEntity>::open().scoped(&ctx.scope());
    if let Some(code) = q.code.filter(|s| !s.is_empty()) {
        query = query.filter(TemplateColumn::Code.fuzzy(&code));
    }
    if let Some(kind) = q.channel_kind.filter(|s| !s.is_empty()) {
        query = query.filter(TemplateColumn::ChannelKind.eq(kind));
    }
    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;
    let paged = query
        // 排序白名单：只认这张表声明过的列，客户端字符串进不了标识符位置。
        // 显式排序必须排在前面才真的生效：默认次序退化成同值行的收尾键（id），
        // 既不改变「不传 sort_by 时的结果」，又保证翻页不重不漏。
        .sorted_by(
            &TEMPLATE_SORT,
            q.sort_by.as_deref(),
            q.sort_order.as_deref(),
        )
        .order_by(TemplateColumn::Id)
        .fetch_page(state.db.conn(), page, page_size)
        .await
        .map_err(db_error)?;
    Ok(Json(TemplatePage {
        items: paged.items.into_iter().map(TemplateVo::from).collect(),
        total: paged.total,
        page: paged.page,
        page_size: paged.page_size,
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/notify/templates",
    request_body = TemplateUpsert,
    responses((status = 201, description = "已创建", body = TemplateVo), (status = 409, description = "同编码同渠道同语言已存在"))
)]
#[require_perm("notify:template:add")]
#[audit(module = "notify", action = "template:add", kind = "insert")]
pub async fn create_template(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<TemplateUpsert>,
) -> Result<(StatusCode, HeaderMap, Json<TemplateVo>), ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    validate_template(&req)?;
    let tenant = ctx.tenant_id.get();
    let taken = TemplateEntity::find()
        .filter(TemplateColumn::TenantId.eq(tenant))
        .filter(TemplateColumn::Code.eq(req.code.trim()))
        .filter(TemplateColumn::ChannelKind.eq(req.channel_kind.clone()))
        .filter(TemplateColumn::Locale.eq(req.locale.clone()))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .is_some();
    if taken {
        return Err(conflict("同编码/渠道/语言的模板已存在"));
    }
    let row = TemplateActiveModel {
        tenant_id: sea_orm::Set(tenant),
        code: sea_orm::Set(req.code.trim().to_string()),
        name: sea_orm::Set(req.name.trim().to_string()),
        channel_kind: sea_orm::Set(req.channel_kind.clone()),
        locale: sea_orm::Set(req.locale.clone()),
        title: sea_orm::Set(req.title.clone()),
        body: sea_orm::Set(req.body.clone()),
        enabled: sea_orm::Set(req.enabled),
        remark: sea_orm::Set(req.remark.clone()),
        create_by: sea_orm::Set(Some(principal.user_name.clone())),
        create_time: sea_orm::Set(chrono::Utc::now()),
        ..Default::default()
    }
    .insert(state.db.conn())
    .await
    .map_err(db_error)?;
    Ok(created(
        format!("/api/v1/notify/templates/{}", row.id),
        TemplateVo::from(row),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/notify/templates/{id}",
    request_body = TemplateUpsert,
    params(("id" = i64, Path, description = "模板 id")),
    responses((status = 200, description = "已更新", body = TemplateVo))
)]
#[require_perm("notify:template:edit")]
#[audit(module = "notify", action = "template:edit", kind = "update")]
pub async fn update_template(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(id): Path<i64>,
    Json(req): Json<TemplateUpsert>,
) -> Result<Json<TemplateVo>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    validate_template(&req)?;
    let current = ScopedQuery::<TemplateEntity>::open()
        .scoped(&ctx.scope())
        .filter(TemplateColumn::Id.eq(id))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    let row = TemplateActiveModel {
        id: sea_orm::Set(current.id),
        code: sea_orm::Set(req.code.trim().to_string()),
        name: sea_orm::Set(req.name.trim().to_string()),
        channel_kind: sea_orm::Set(req.channel_kind.clone()),
        locale: sea_orm::Set(req.locale.clone()),
        title: sea_orm::Set(req.title.clone()),
        body: sea_orm::Set(req.body.clone()),
        enabled: sea_orm::Set(req.enabled),
        remark: sea_orm::Set(req.remark.clone()),
        update_by: sea_orm::Set(Some(principal.user_name.clone())),
        update_time: sea_orm::Set(Some(chrono::Utc::now())),
        ..Default::default()
    }
    .update(state.db.conn())
    .await
    .map_err(db_error)?;
    Ok(Json(TemplateVo::from(row)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/notify/templates/{id}",
    params(("id" = i64, Path, description = "模板 id")),
    responses((status = 204, description = "已删除"))
)]
#[require_perm("notify:template:remove")]
#[audit(module = "notify", action = "template:remove", kind = "delete")]
pub async fn delete_template(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let res = TemplateEntity::delete_many()
        .filter(TemplateColumn::Id.eq(id))
        .filter(TemplateColumn::TenantId.eq(ctx.tenant_id.get()))
        .exec(state.db.conn())
        .await
        .map_err(db_error)?;
    if res.rows_affected == 0 {
        return Err(not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------- 记录与发送

#[derive(Debug, Deserialize, IntoParams)]
pub struct RecordListQuery {
    pub template_code: Option<String>,
    pub recipient: Option<String>,
    pub status: Option<i16>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    /// 排序列名（排序白名单，见 `crate::sort`）；不在清单里则忽略。
    pub sort_by: Option<String>,
    /// `asc` 升序，其余按 `desc`。
    pub sort_order: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RecordVo {
    pub id: i64,
    pub channel_id: i64,
    pub template_code: String,
    pub recipient: String,
    pub payload: serde_json::Value,
    pub status: i16,
    pub error: Option<String>,
    pub create_by: Option<String>,
    pub create_time: String,
    pub sent_at: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RecordPage {
    pub items: Vec<RecordVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SendBody {
    pub channel_code: String,
    pub template_code: String,
    pub recipient: String,
    pub locale: Option<String>,
    #[serde(default)]
    pub payload: serde_json::Value,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SendVo {
    pub record_id: i64,
    /// 1 已发 / 2 失败。
    pub status: i16,
    pub error: Option<String>,
    pub rendered_body: String,
}

impl From<RecordModel> for RecordVo {
    fn from(m: RecordModel) -> Self {
        Self {
            id: m.id,
            channel_id: m.channel_id,
            template_code: m.template_code,
            recipient: m.recipient,
            payload: m.payload,
            status: m.status,
            error: m.error,
            create_by: m.create_by,
            create_time: m.create_time.to_rfc3339(),
            sent_at: m.sent_at.map(|t| t.to_rfc3339()),
        }
    }
}

/// `sys_notify_record` 列表允许排序的列（排序白名单）。
///
/// 只放「列表页真的会点排序」的列：每多一个可排序列，就多一条能被利用的
/// `ORDER BY` 路径（大表上非索引列排序会把排序成本推给每一次翻页）。
const RECORD_SORT: crate::sort::SortSpec = crate::sort::SortSpec::new(&[
    ("recipient", "sys_notify_record", "recipient"),
    ("template_code", "sys_notify_record", "template_code"),
    ("status", "sys_notify_record", "status"),
    ("create_time", "sys_notify_record", "create_time"),
]);

#[utoipa::path(
    get,
    path = "/api/v1/notify/records",
    params(RecordListQuery),
    responses((status = 200, description = "投递记录分页", body = RecordPage))
)]
#[require_perm("notify:record:list")]
pub async fn list_records(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<RecordListQuery>,
) -> Result<Json<RecordPage>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let mut query = ScopedQuery::<RecordEntity>::open().scoped(&ctx.scope());
    if let Some(code) = q.template_code.filter(|s| !s.is_empty()) {
        query = query.filter(RecordColumn::TemplateCode.eq(code));
    }
    if let Some(recipient) = q.recipient.filter(|s| !s.is_empty()) {
        query = query.filter(RecordColumn::Recipient.fuzzy(&recipient));
    }
    if let Some(status) = q.status {
        query = query.filter(RecordColumn::Status.eq(status));
    }
    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;
    let paged = query
        // 排序白名单：只认这张表声明过的列，客户端字符串进不了标识符位置。
        // 显式排序必须排在前面才真的生效：默认次序退化成同值行的收尾键（id），
        // 既不改变「不传 sort_by 时的结果」，又保证翻页不重不漏。
        .sorted_by(&RECORD_SORT, q.sort_by.as_deref(), q.sort_order.as_deref())
        .order_by_desc(RecordColumn::Id)
        .fetch_page(state.db.conn(), page, page_size)
        .await
        .map_err(db_error)?;
    Ok(Json(RecordPage {
        items: paged.items.into_iter().map(RecordVo::from).collect(),
        total: paged.total,
        page: paged.page,
        page_size: paged.page_size,
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/notify/send",
    request_body = SendBody,
    responses(
        (status = 200, description = "已投递", body = SendVo),
        (status = 502, description = "网关投递失败（记录已留痕）", body = SendVo),
        (status = 404, description = "渠道或模板不存在", body = Problem)
    )
)]
#[require_perm("notify:send")]
#[audit(module = "notify", action = "send", kind = "other")]
pub async fn send_notify(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<SendBody>,
) -> Result<Json<SendVo>, ApiErr> {
    let ctx = ctx.into_inner();
    let operator = ctx.principal.as_ref().map(|p| p.user_name.clone());

    let outcome = state
        .notify
        .send(
            ctx.tenant_id,
            SendRequest {
                channel_code: req.channel_code.clone(),
                template_code: req.template_code.clone(),
                recipient: req.recipient.clone(),
                locale: req.locale.clone(),
                payload: req.payload.clone(),
                operator,
            },
        )
        .await
        .map_err(map_notify_err)?;

    let vo = SendVo {
        record_id: outcome.record_id,
        status: outcome.status,
        error: outcome.error.clone(),
        rendered_body: outcome.rendered_body,
    };
    if outcome.status == SendOutcome::FAILED {
        // 记录已留痕，接口返回 502 让调用方明确知道没发出去。
        // 业务码目录把「邮件等外部依赖失败」定为 9004/500；这里保留 502 让调用方
        // 明确区分「本服务坏了」与「上游渠道没送达」。
        return Err(Problem::new(
            BizCode::ExternalDependency,
            vo.error.clone().unwrap_or_else(|| "投递失败".to_string()),
        )
        .with_status(StatusCode::BAD_GATEWAY));
    }
    Ok(Json(vo))
}

fn map_notify_err(e: NotifyError) -> ApiErr {
    match e {
        NotifyError::NotFound(msg) => err(StatusCode::NOT_FOUND, msg),
        NotifyError::Disabled(msg) => err(StatusCode::CONFLICT, format!("渠道已停用：{msg}")),
        NotifyError::Config(msg) | NotifyError::Render(msg) => {
            err(StatusCode::UNPROCESSABLE_ENTITY, msg)
        }
        other => {
            tracing::error!(error = %other, "通知中心错误");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "通知中心操作失败，请稍后重试",
            )
        }
    }
}

/// 通知中心模块：权限点与路由一并声明，装配期对账。
pub struct NotifyModule;

impl Module for NotifyModule {
    fn manifest(self: Box<Self>) -> Manifest {
        Manifest::new("cicada:notify")
            .perms(PERMS.iter().map(|p| Permission::new(*p)))
            .routes(routes)
    }
}

pub fn routes(_state: AppState) -> Routes {
    Routes::new()
        .route(
            "/api/v1/notify/channels",
            get(list_channels),
            list_channels__perms(),
        )
        .route_audited(
            "/api/v1/notify/channels",
            post(create_channel),
            create_channel__perms(),
            create_channel__audit(),
        )
        .route_audited(
            "/api/v1/notify/channels/{id}",
            put(update_channel),
            update_channel__perms(),
            update_channel__audit(),
        )
        .route_audited(
            "/api/v1/notify/channels/{id}",
            delete(delete_channel),
            delete_channel__perms(),
            delete_channel__audit(),
        )
        .route(
            "/api/v1/notify/templates",
            get(list_templates),
            list_templates__perms(),
        )
        .route_audited(
            "/api/v1/notify/templates",
            post(create_template),
            create_template__perms(),
            create_template__audit(),
        )
        .route_audited(
            "/api/v1/notify/templates/{id}",
            put(update_template),
            update_template__perms(),
            update_template__audit(),
        )
        .route_audited(
            "/api/v1/notify/templates/{id}",
            delete(delete_template),
            delete_template__perms(),
            delete_template__audit(),
        )
        .route(
            "/api/v1/notify/records",
            get(list_records),
            list_records__perms(),
        )
        .route_audited(
            "/api/v1/notify/send",
            post(send_notify),
            send_notify__perms(),
            send_notify__audit(),
        )
}
