//! 代码生成（M3，权限点 `tool:gen:*`）。
//!
//! 流程：列库表 → 导入元数据（`gen_table` / `gen_table_column`）→ 在线改配置 →
//! 预览 / 打包下载。**不在线写源码树**：`gen_type = 1` 明确拒绝（见本文件的 `resolve_artifacts`），
//! 生成物一律以 zip 交付，避免管理接口变成任意路径写。
//!
//! 权限点只有清单里的那 5 个，不新增命名空间：
//! 列表/详情/候选表 → `tool:gen:list`；导入同样落 `tool:gen:list`（参考实现语义），
//! 写操作走 `edit` / `remove`，产物读取走 `preview` / `code`。

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::{delete, get, post, put};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

use cicada_codegen::{
    Artifact, CodegenError, GenTableActiveModel, GenTableColumnActiveModel, GenTableColumnCol,
    GenTableColumnEntity, GenTableColumnModel, GenTableModel, RenderInput, TablePlan, TplCategory,
    build_plan, describe_table, list_tables, naming, plan_from_models, previewable_templates,
    render_all, render_one, store, validate_plan, zip_artifacts,
};
use cicada_macros::{audit, require_perm};

use crate::Permission;
use crate::extractor::RequestContextExt;
use crate::module::{Manifest, Module, TableName};
use crate::routes::Routes;
use crate::state::AppState;

use crate::problem::ApiErr;

pub const PERMS: &[&str] = &[
    "tool:gen:list",
    "tool:gen:edit",
    "tool:gen:remove",
    "tool:gen:preview",
    "tool:gen:code",
];

/// sea-orm 错误（落库失败）统一 500。
fn db_to_api(e: sea_orm::DbErr) -> ApiErr {
    tracing::error!(error = %e, "代码生成数据库操作失败");
    err(
        StatusCode::INTERNAL_SERVER_ERROR,
        "代码生成失败，请稍后重试",
    )
}

fn err(status: StatusCode, msg: impl Into<String>) -> ApiErr {
    ApiErr::from_status(status, msg)
}

fn unauthorized() -> ApiErr {
    err(StatusCode::UNAUTHORIZED, "未认证")
}

fn not_found() -> ApiErr {
    err(StatusCode::NOT_FOUND, "资源不存在")
}

/// 生成器错误 → HTTP：参数/拒绝类 422，未支持 422，未找到 404，其余 500。
fn map_err(e: CodegenError) -> ApiErr {
    match e {
        CodegenError::NotFound(m) => err(StatusCode::NOT_FOUND, m),
        CodegenError::Rejected(m) | CodegenError::Unsupported(m) => {
            err(StatusCode::UNPROCESSABLE_ENTITY, m)
        }
        other => {
            tracing::error!(error = %other, "代码生成失败");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "代码生成失败，请稍后重试",
            )
        }
    }
}

fn tenant_of(ctx: &cicada_kernel::RequestContext) -> Result<i64, ApiErr> {
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    Ok(ctx.tenant_id.get())
}

// ---------- 列表 ----------

#[derive(Debug, Deserialize, IntoParams)]
pub struct GenTableListQuery {
    pub table_name: Option<String>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GenColumnVo {
    pub column_id: i64,
    pub column_name: String,
    pub column_comment: String,
    pub column_type: String,
    pub rust_type: String,
    pub rust_field: String,
    pub is_pk: bool,
    pub is_increment: bool,
    pub is_required: bool,
    pub is_insert: bool,
    pub is_edit: bool,
    pub is_list: bool,
    pub is_query: bool,
    pub query_type: String,
    pub html_type: String,
    pub dict_type: Option<String>,
    pub sort: i32,
}

impl From<GenTableColumnModel> for GenColumnVo {
    fn from(m: GenTableColumnModel) -> Self {
        Self {
            column_id: m.column_id,
            column_name: m.column_name,
            column_comment: m.column_comment,
            column_type: m.column_type,
            rust_type: m.rust_type,
            rust_field: m.rust_field,
            is_pk: m.is_pk,
            is_increment: m.is_increment,
            is_required: m.is_required,
            is_insert: m.is_insert,
            is_edit: m.is_edit,
            is_list: m.is_list,
            is_query: m.is_query,
            query_type: m.query_type,
            html_type: m.html_type,
            dict_type: m.dict_type,
            sort: m.sort,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GenTableVo {
    pub table_id: i64,
    pub table_name: String,
    pub table_comment: String,
    pub class_name: String,
    pub module_name: String,
    pub business_name: String,
    pub function_name: String,
    pub function_author: String,
    pub tpl_category: String,
    pub form_col_num: i16,
    pub gen_type: String,
    pub remark: Option<String>,
    pub create_time: String,
    pub columns: Vec<GenColumnVo>,
}

async fn to_vo(state: &AppState, tenant: i64, m: GenTableModel) -> Result<GenTableVo, ApiErr> {
    let columns = store::list_columns(state.db.conn(), tenant, m.table_id)
        .await
        .map_err(map_err)?;
    Ok(GenTableVo {
        table_id: m.table_id,
        table_name: m.table_name,
        table_comment: m.table_comment,
        class_name: m.class_name,
        module_name: m.module_name,
        business_name: m.business_name,
        function_name: m.function_name,
        function_author: m.function_author,
        tpl_category: m.tpl_category,
        form_col_num: m.form_col_num,
        gen_type: m.gen_type,
        remark: m.remark,
        create_time: m.create_time.to_rfc3339(),
        columns: columns.into_iter().map(GenColumnVo::from).collect(),
    })
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GenTablePage {
    pub items: Vec<GenTableVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

#[utoipa::path(
    get,
    path = "/api/v1/tool/gen/tables",
    params(GenTableListQuery),
    responses((status = 200, description = "已导入的表", body = GenTablePage))
)]
#[require_perm("tool:gen:list")]
pub async fn list_gen_tables(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<GenTableListQuery>,
) -> Result<Json<GenTablePage>, ApiErr> {
    let ctx = ctx.into_inner();
    let tenant = tenant_of(&ctx)?;
    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;
    let (models, total) = store::list_tables(
        state.db.conn(),
        tenant,
        &store::GenTableQuery {
            table_name: q.table_name,
            page,
            page_size,
        },
    )
    .await
    .map_err(map_err)?;
    let mut items = Vec::with_capacity(models.len());
    for m in models {
        items.push(to_vo(&state, tenant, m).await?);
    }
    Ok(Json(GenTablePage {
        items,
        total,
        page,
        page_size,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/tool/gen/tables/{table_id}",
    params(("table_id" = i64, Path, description = "gen_table 主键")),
    responses((status = 200, description = "表详情", body = GenTableVo), (status = 404, description = "不存在"))
)]
#[require_perm("tool:gen:list")]
pub async fn get_gen_table(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(table_id): Path<i64>,
) -> Result<Json<GenTableVo>, ApiErr> {
    let ctx = ctx.into_inner();
    let tenant = tenant_of(&ctx)?;
    let model = store::get_table(state.db.conn(), tenant, table_id)
        .await
        .map_err(map_err)?
        .ok_or_else(not_found)?;
    Ok(Json(to_vo(&state, tenant, model).await?))
}

// ---------- 候选表 / 导入 ----------

#[derive(Debug, Serialize, ToSchema)]
pub struct DbTableVo {
    pub table_name: String,
    pub table_comment: String,
    pub column_count: i64,
    /// 是否已经导入过（前端据此禁用「导入」按钮）。
    pub imported: bool,
    /// 该表能否被生成（框架表 / 缺列 / 复合主键会被拒）。
    pub generatable: bool,
    /// 不可生成的原因。
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DbTableList {
    pub items: Vec<DbTableVo>,
    /// 配置里的准入前缀（空表示不限制）。
    pub allowed_prefixes: Vec<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/tool/gen/db/tables",
    responses((status = 200, description = "库里的候选表", body = DbTableList))
)]
#[require_perm("tool:gen:list")]
pub async fn list_db_tables(
    State(state): State<AppState>,
    ctx: RequestContextExt,
) -> Result<Json<DbTableList>, ApiErr> {
    let ctx = ctx.into_inner();
    let tenant = tenant_of(&ctx)?;
    let cfg = &state.config.codegen;
    let summaries = list_tables(&state.db).await.map_err(map_err)?;
    let (existing, _) = store::list_tables(
        state.db.conn(),
        tenant,
        &store::GenTableQuery {
            table_name: None,
            page: 1,
            page_size: 200,
        },
    )
    .await
    .map_err(map_err)?;
    let imported: Vec<String> = existing.into_iter().map(|m| m.table_name).collect();

    let mut items = Vec::with_capacity(summaries.len());
    for s in summaries {
        let (generatable, reason) = match describe_table(&state.db, &s.table_name).await {
            Ok(d) => match build_plan(&d, cfg, TplCategory::Crud) {
                Ok(_) => (true, None),
                Err(e) => (false, Some(e.to_string())),
            },
            Err(e) => (false, Some(e.to_string())),
        };
        items.push(DbTableVo {
            imported: imported.contains(&s.table_name),
            generatable,
            reason,
            table_name: s.table_name,
            table_comment: s.table_comment,
            column_count: s.column_count,
        });
    }
    Ok(Json(DbTableList {
        items,
        allowed_prefixes: cfg.allowed_prefixes.clone(),
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ImportRequest {
    pub tables: Vec<String>,
    /// `crud` / `tree` / `sub`，缺省 `crud`。
    pub tpl_category: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ImportFailure {
    pub table_name: String,
    pub error: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ImportResult {
    pub imported: Vec<String>,
    pub failed: Vec<ImportFailure>,
}

#[utoipa::path(
    post,
    path = "/api/v1/tool/gen/import",
    request_body = ImportRequest,
    responses((status = 200, description = "导入结果", body = ImportResult))
)]
#[require_perm("tool:gen:list")]
#[audit(module = "tool", action = "gen:import", kind = "import")]
pub async fn import_tables(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<ImportRequest>,
) -> Result<Json<ImportResult>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    let tenant = tenant_of(&ctx)?;
    let cfg = &state.config.codegen;
    if !cfg.enabled {
        return Err(err(StatusCode::FORBIDDEN, "代码生成已在配置中关闭"));
    }
    if req.tables.is_empty() {
        return Err(err(StatusCode::UNPROCESSABLE_ENTITY, "tables 不能为空"));
    }
    if req.tables.len() > cfg.max_import_tables {
        return Err(err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("一次最多导入 {} 张表", cfg.max_import_tables),
        ));
    }
    let category = match &req.tpl_category {
        Some(c) => TplCategory::parse(c).map_err(map_err)?,
        None => TplCategory::Crud,
    };

    let mut imported = Vec::new();
    let mut failed = Vec::new();
    for table in req.tables {
        let outcome = match describe_table(&state.db, &table).await {
            Ok(discovered) => match build_plan(&discovered, cfg, category) {
                Ok(plan) => store::save_plan(
                    state.db.conn(),
                    tenant,
                    &plan,
                    Some(principal.user_name.as_str()),
                )
                .await
                .map(|_| ()),
                Err(e) => Err(e),
            },
            Err(e) => Err(e),
        };
        match outcome {
            Ok(()) => imported.push(table),
            Err(e) => failed.push(ImportFailure {
                table_name: table,
                error: e.to_string(),
            }),
        }
    }
    Ok(Json(ImportResult { imported, failed }))
}

// ---------- 在线编辑 ----------

#[derive(Debug, Deserialize, ToSchema)]
pub struct GenColumnPatch {
    pub column_name: String,
    pub column_comment: Option<String>,
    pub is_required: Option<bool>,
    pub is_insert: Option<bool>,
    pub is_edit: Option<bool>,
    pub is_list: Option<bool>,
    pub is_query: Option<bool>,
    pub query_type: Option<String>,
    pub html_type: Option<String>,
    /// `null` 表示清空字典，`Some("")` 也视为清空。
    pub dict_type: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct GenTableUpsert {
    pub class_name: Option<String>,
    pub module_name: Option<String>,
    pub business_name: Option<String>,
    pub function_name: Option<String>,
    pub function_author: Option<String>,
    pub tpl_category: Option<String>,
    pub form_col_num: Option<i16>,
    pub remark: Option<String>,
    #[serde(default)]
    pub columns: Vec<GenColumnPatch>,
}

#[utoipa::path(
    put,
    path = "/api/v1/tool/gen/tables/{table_id}",
    params(("table_id" = i64, Path, description = "gen_table 主键")),
    request_body = GenTableUpsert,
    responses((status = 200, description = "已保存", body = GenTableVo), (status = 404, description = "不存在"))
)]
#[require_perm("tool:gen:edit")]
#[audit(module = "tool", action = "gen:edit", kind = "update")]
pub async fn update_gen_table(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(table_id): Path<i64>,
    Json(req): Json<GenTableUpsert>,
) -> Result<Json<GenTableVo>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    let tenant = tenant_of(&ctx)?;
    let model = store::get_table(state.db.conn(), tenant, table_id)
        .await
        .map_err(map_err)?
        .ok_or_else(not_found)?;

    // 命名会变成产物路径与 Rust 类型名：先校验再落库。
    let class_name = req.class_name.clone().unwrap_or(model.class_name.clone());
    let module_name = req.module_name.clone().unwrap_or(model.module_name.clone());
    let business_name = req
        .business_name
        .clone()
        .unwrap_or(model.business_name.clone());
    if !naming::is_valid_identifier(&module_name) || !naming::is_valid_identifier(&business_name) {
        return Err(err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "module_name / business_name 必须是合法标识符",
        ));
    }
    if class_name.is_empty()
        || class_name.len() > 63
        || !class_name.chars().all(|c| c.is_ascii_alphanumeric())
        || !class_name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase())
    {
        return Err(err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "class_name 必须是 PascalCase 标识符",
        ));
    }
    if let Some(c) = &req.tpl_category {
        TplCategory::parse(c).map_err(map_err)?;
    }

    let now = chrono::Utc::now();
    let updated = GenTableActiveModel {
        table_id: Set(table_id),
        class_name: Set(class_name),
        module_name: Set(module_name),
        business_name: Set(business_name),
        function_name: Set(req.function_name.clone().unwrap_or(model.function_name)),
        function_author: Set(req.function_author.clone().unwrap_or(model.function_author)),
        tpl_category: Set(req.tpl_category.clone().unwrap_or(model.tpl_category)),
        form_col_num: Set(req.form_col_num.unwrap_or(model.form_col_num)),
        remark: Set(req.remark.clone().or(model.remark)),
        update_by: Set(Some(principal.user_name.clone())),
        update_time: Set(Some(now)),
        ..Default::default()
    };
    updated.update(state.db.conn()).await.map_err(db_to_api)?;

    if !req.columns.is_empty() {
        for patch in &req.columns {
            if !naming::is_valid_identifier(&patch.column_name) {
                return Err(err(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("非法列名：{}", patch.column_name),
                ));
            }
            let Some(existing) = GenTableColumnEntity::find()
                .filter(GenTableColumnCol::TenantId.eq(tenant))
                .filter(GenTableColumnCol::TableId.eq(table_id))
                .filter(GenTableColumnCol::ColumnName.eq(patch.column_name.as_str()))
                .one(state.db.conn())
                .await
                .map_err(db_to_api)?
            else {
                return Err(err(
                    StatusCode::NOT_FOUND,
                    format!("列不存在：{}", patch.column_name),
                ));
            };
            let query_type = patch
                .query_type
                .clone()
                .unwrap_or(existing.query_type.clone());
            if !["EQ", "LIKE", "BETWEEN"].contains(&query_type.as_str()) {
                return Err(err(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("不支持的查询方式：{query_type}"),
                ));
            }
            let dict = match patch.dict_type.as_deref() {
                Some("") => None,
                Some(v) => Some(v.to_string()),
                None => existing.dict_type.clone(),
            };
            if let Some(v) = dict.as_deref()
                && !naming::is_valid_identifier(v)
            {
                return Err(err(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("非法字典类型：{v}"),
                ));
            }
            GenTableColumnActiveModel {
                column_id: Set(existing.column_id),
                column_comment: Set(patch
                    .column_comment
                    .clone()
                    .unwrap_or(existing.column_comment)),
                is_required: Set(patch.is_required.unwrap_or(existing.is_required)),
                is_insert: Set(patch.is_insert.unwrap_or(existing.is_insert)),
                is_edit: Set(patch.is_edit.unwrap_or(existing.is_edit)),
                is_list: Set(patch.is_list.unwrap_or(existing.is_list)),
                is_query: Set(patch.is_query.unwrap_or(existing.is_query)),
                query_type: Set(query_type),
                html_type: Set(patch.html_type.clone().unwrap_or(existing.html_type)),
                dict_type: Set(dict),
                update_time: Set(Some(now)),
                ..Default::default()
            }
            .update(state.db.conn())
            .await
            .map_err(db_to_api)?;
        }
    }

    let model = store::get_table(state.db.conn(), tenant, table_id)
        .await
        .map_err(map_err)?
        .ok_or_else(not_found)?;
    Ok(Json(to_vo(&state, tenant, model).await?))
}

// ---------- 删除 ----------

#[utoipa::path(
    delete,
    path = "/api/v1/tool/gen/tables/{table_id}",
    params(("table_id" = i64, Path, description = "gen_table 主键")),
    responses((status = 204, description = "已删除"), (status = 404, description = "不存在"))
)]
#[require_perm("tool:gen:remove")]
#[audit(module = "tool", action = "gen:remove", kind = "delete")]
pub async fn delete_gen_table(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(table_id): Path<i64>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    let tenant = tenant_of(&ctx)?;
    let removed = store::delete_table(
        state.db.conn(),
        tenant,
        table_id,
        Some(principal.user_name.as_str()),
    )
    .await
    .map_err(map_err)?;
    if !removed {
        return Err(not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---------- 预览 / 下载 ----------

/// 从元数据重建生成计划。
async fn load_plan(state: &AppState, tenant: i64, table_id: i64) -> Result<TablePlan, ApiErr> {
    let model = store::get_table(state.db.conn(), tenant, table_id)
        .await
        .map_err(map_err)?
        .ok_or_else(not_found)?;
    let columns = store::list_columns(state.db.conn(), tenant, table_id)
        .await
        .map_err(map_err)?;
    plan_from_models(&model, &columns).map_err(map_err)
}

/// `gen_type = 1`（写源码树）明确拒绝：管理接口不应有任意路径写能力。
fn resolve_artifacts(
    state: &AppState,
    plan: &TablePlan,
) -> Result<Vec<cicada_codegen::Artifact>, ApiErr> {
    if plan.gen_type == "1" {
        return Err(err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "写源码树（gen_type = 1）未开放：请使用打包下载",
        ));
    }
    let cfg = &state.config.codegen;
    let input = RenderInput {
        plan,
        date: chrono::Utc::now().format("%Y%m%d").to_string(),
        seq: format!("{:06}", plan.columns.len()),
        route_prefix: cfg.route_prefix.clone(),
        template_dir: cfg.template_dir.clone().map(std::path::PathBuf::from),
    };
    render_all(&input).map_err(map_err)
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewFile {
    pub name: String,
    pub path: String,
    pub content: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewResponse {
    pub files: Vec<PreviewFile>,
}

#[utoipa::path(
    get,
    path = "/api/v1/tool/gen/tables/{table_id}/preview",
    params(("table_id" = i64, Path, description = "gen_table 主键")),
    responses((status = 200, description = "渲染预览", body = PreviewResponse))
)]
#[require_perm("tool:gen:preview")]
pub async fn preview_gen_table(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(table_id): Path<i64>,
) -> Result<Json<PreviewResponse>, ApiErr> {
    let ctx = ctx.into_inner();
    let tenant = tenant_of(&ctx)?;
    let plan = load_plan(&state, tenant, table_id).await?;
    validate_plan(&plan).map_err(map_err)?;
    let cfg = &state.config.codegen;
    let input = RenderInput {
        plan: &plan,
        date: chrono::Utc::now().format("%Y%m%d").to_string(),
        seq: format!("{:06}", plan.columns.len()),
        route_prefix: cfg.route_prefix.clone(),
        template_dir: cfg.template_dir.clone().map(std::path::PathBuf::from),
    };
    let mut files = Vec::new();
    for name in previewable_templates() {
        let artifact = render_one(&input, name).map_err(map_err)?;
        files.push(PreviewFile {
            name: name.to_string(),
            path: artifact.path,
            content: artifact.content,
        });
    }
    Ok(Json(PreviewResponse { files }))
}

#[utoipa::path(
    get,
    path = "/api/v1/tool/gen/tables/{table_id}/download",
    params(("table_id" = i64, Path, description = "gen_table 主键")),
    responses((status = 200, description = "zip 产物"), (status = 404, description = "不存在"))
)]
#[require_perm("tool:gen:code")]
pub async fn download_gen_table(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(table_id): Path<i64>,
) -> Result<Response, ApiErr> {
    let ctx = ctx.into_inner();
    let tenant = tenant_of(&ctx)?;
    let plan = load_plan(&state, tenant, table_id).await?;
    let artifacts = resolve_artifacts(&state, &plan)?;
    zip_response(&plan.business_name, &artifacts)
}

#[utoipa::path(
    get,
    path = "/api/v1/tool/gen/tables/download",
    params(GenTableListQuery),
    responses((status = 200, description = "批量 zip 产物"))
)]
#[require_perm("tool:gen:code")]
pub async fn download_gen_tables(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<GenTableListQuery>,
) -> Result<Response, ApiErr> {
    let ctx = ctx.into_inner();
    let tenant = tenant_of(&ctx)?;
    let (models, _) = store::list_tables(
        state.db.conn(),
        tenant,
        &store::GenTableQuery {
            table_name: q.table_name.clone(),
            page: 1,
            page_size: 200,
        },
    )
    .await
    .map_err(map_err)?;
    if models.is_empty() {
        return Err(not_found());
    }
    let mut artifacts = Vec::new();
    for model in models {
        let plan = load_plan(&state, tenant, model.table_id).await?;
        artifacts.extend(resolve_artifacts(&state, &plan)?);
    }
    zip_response("cicada-gen-batch", &artifacts)
}

fn zip_response(name: &str, artifacts: &[Artifact]) -> Result<Response, ApiErr> {
    let bytes = zip_artifacts(artifacts).map_err(map_err)?;
    let disposition = format!("attachment; filename=\"{name}.zip\"");
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    if let Ok(value) = HeaderValue::from_str(&disposition) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
    Ok(response)
}

// ---------- 模块 ----------

/// 代码生成模块：权限点与路由一并声明，装配期双向对账。
pub struct CodeModule;

impl Module for CodeModule {
    fn manifest(self: Box<Self>) -> Manifest {
        Manifest::new("cicada:codegen")
            .perms(PERMS.iter().map(|p| Permission::new(*p)))
            .owns([TableName("gen_table"), TableName("gen_table_column")])
            .routes(routes)
    }
}

pub fn routes(_state: AppState) -> Routes {
    Routes::new()
        .route(
            "/api/v1/tool/gen/tables",
            get(list_gen_tables),
            list_gen_tables__perms(),
        )
        .route(
            "/api/v1/tool/gen/tables/{table_id}",
            get(get_gen_table),
            get_gen_table__perms(),
        )
        .route(
            "/api/v1/tool/gen/db/tables",
            get(list_db_tables),
            list_db_tables__perms(),
        )
        .route(
            "/api/v1/tool/gen/tables/download",
            get(download_gen_tables),
            download_gen_tables__perms(),
        )
        .route_audited(
            "/api/v1/tool/gen/import",
            post(import_tables),
            import_tables__perms(),
            import_tables__audit(),
        )
        .route_audited(
            "/api/v1/tool/gen/tables/{table_id}",
            put(update_gen_table),
            update_gen_table__perms(),
            update_gen_table__audit(),
        )
        .route(
            "/api/v1/tool/gen/tables/{table_id}/preview",
            get(preview_gen_table),
            preview_gen_table__perms(),
        )
        .route(
            "/api/v1/tool/gen/tables/{table_id}/download",
            get(download_gen_table),
            download_gen_table__perms(),
        )
        .route_audited(
            "/api/v1/tool/gen/tables/{table_id}",
            delete(delete_gen_table),
            delete_gen_table__perms(),
            delete_gen_table__audit(),
        )
}
