//! 文件上传下载：`system:file:*`。
//!
//! 上传：校验大小 + MIME（读 magic number，不信扩展名）→ 生成不可猜测路径
//! `tenant/uuid` → 落 `sys_file` → 返回引用 ID。
//! 下载两条路（文件存储）：
//! - 服务端代理：`GET /system/files/{id}/content`，先鉴权（权限点 + 数据范围）再回字节，
//!   任何情况下都不暴露物理路径；
//! - 短时链接：`POST /system/files/{id}/download-url`——对象存储驱动给桶端预签名 URL，
//!   本地驱动签发**一次性** token，由 `GET /api/v1/files/dl/{token}` 单次消费。
//!
//! 封死参考实现历史上的任意文件下载漏洞：路径不可猜 + 链接短命 + 用一次即废。
//!
//! 元数据实体 `sys_file` 无归属维度，列表/详情走受控查询
//! `Query::<..>::open().scoped(&scope)`：非「全部」数据范围按 `NoOwnership` 失败关闭。

use std::time::Duration;

use axum::Json;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, Set};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use cicada_kernel::scope::Query as ScopedQuery;
use cicada_macros::{audit, require_perm};
use cicada_storage::{DownloadGrant, StorageError, detect_mime, object_path, sha256_hex};
use cicada_system::entity::{FileActiveModel, FileColumn, FileEntity, FileModel};

use crate::Permission;
use crate::export;
use crate::extractor::RequestContextExt;
use crate::module::{Manifest, Module};
use crate::response::created;
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

fn bad_request(msg: impl Into<String>) -> ApiErr {
    err(StatusCode::UNPROCESSABLE_ENTITY, msg)
}

fn db_error(e: impl std::fmt::Display) -> ApiErr {
    tracing::error!(error = %e, "文件模块数据库错误");
    err(
        StatusCode::INTERNAL_SERVER_ERROR,
        "文件操作失败，请稍后重试",
    )
}

pub(crate) fn storage_error(e: StorageError) -> ApiErr {
    tracing::error!(error = %e, "文件存储错误");
    match e {
        StorageError::NotFound(_) => err(StatusCode::NOT_FOUND, "文件内容不存在"),
        _ => err(StatusCode::INTERNAL_SERVER_ERROR, "存储不可用"),
    }
}

pub const PERMS: &[&str] = &["system:file:add", "system:file:list", "system:file:remove"];

#[derive(Debug, Deserialize, IntoParams)]
pub struct FileListQuery {
    pub original_name: Option<String>,
    pub biz_type: Option<String>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    /// 排序列名（排序白名单，见 `crate::sort`）；不在清单里则忽略。
    pub sort_by: Option<String>,
    /// `asc` 升序，其余按 `desc`。
    pub sort_order: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FileVo {
    pub file_id: i64,
    pub storage: String,
    pub original_name: String,
    pub size: i64,
    pub mime: String,
    pub sha256: String,
    pub biz_type: Option<String>,
    pub biz_id: Option<i64>,
    pub create_time: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FilePage {
    pub items: Vec<FileVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

impl From<FileModel> for FileVo {
    fn from(m: FileModel) -> Self {
        Self {
            file_id: m.file_id,
            storage: m.storage,
            original_name: m.original_name,
            size: m.size,
            mime: m.mime,
            sha256: m.sha256,
            biz_type: m.biz_type,
            biz_id: m.biz_id,
            create_time: m.create_time.to_rfc3339(),
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/system/files",
    request_body(content = String, content_type = "multipart/form-data"),
    responses(
        (status = 201, description = "已上传", body = FileVo),
        (status = 422, description = "缺少文件或超过大小上限")
    )
)]
#[require_perm("system:file:add")]
#[audit(module = "system", action = "file:add", kind = "insert")]
pub async fn upload_file(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    mut multipart: Multipart,
) -> Result<(StatusCode, HeaderMap, Json<FileVo>), ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    let tenant_id = ctx.tenant_id.get();

    let mut payload: Option<(String, Vec<u8>)> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| bad_request(format!("上传解析失败：{e}")))?
    {
        if field.name() == Some("file") {
            let filename = field.file_name().unwrap_or("upload.bin").to_string();
            let data = field
                .bytes()
                .await
                .map_err(|e| bad_request(format!("读取文件失败：{e}")))?;
            payload = Some((filename, data.to_vec()));
            break;
        }
    }
    let Some((original_name, bytes)) = payload else {
        return Err(bad_request("缺少上传文件（表单字段名应为 file）"));
    };
    let max_bytes = state.config.storage.max_upload_mb * 1024 * 1024;
    if bytes.is_empty() {
        return Err(bad_request("空文件"));
    }
    if bytes.len() > max_bytes {
        return Err(bad_request(format!(
            "文件超过 {}MB 上限",
            state.config.storage.max_upload_mb
        )));
    }

    // 租户存储上限：本次写入不得越过套餐限额。
    state
        .quota
        .check_storage_slot(ctx.tenant_id, bytes.len() as i64)
        .await
        .map_err(|e| {
            tracing::info!(error = %e, "租户存储限额闸门拒绝写入");
            err(StatusCode::CONFLICT, e.to_string())
        })?;

    let mime = detect_mime(&bytes).to_string();
    let path = object_path(tenant_id, &original_name);
    state
        .storage
        .put(&path, &bytes)
        .await
        .map_err(storage_error)?;

    let now = chrono::Utc::now();
    let saved = FileActiveModel {
        tenant_id: Set(tenant_id),
        storage: Set(state.storage.driver().to_string()),
        path: Set(path),
        original_name: Set(original_name),
        size: Set(bytes.len() as i64),
        mime: Set(mime),
        sha256: Set(sha256_hex(&bytes)),
        create_by: Set(Some(principal.user_name.clone())),
        create_time: Set(now),
        ..Default::default()
    }
    .insert(state.db.conn())
    .await
    .map_err(db_error)?;

    Ok(created(
        format!("/api/v1/system/files/{}", saved.file_id),
        FileVo::from(saved),
    ))
}

/// `sys_file` 列表允许排序的列（排序白名单）。
///
/// 只放「列表页真的会点排序」的列：每多一个可排序列，就多一条能被利用的
/// `ORDER BY` 路径（大表上非索引列排序会把排序成本推给每一次翻页）。
const FILE_SORT: crate::sort::SortSpec = crate::sort::SortSpec::new(&[
    ("original_name", "sys_file", "original_name"),
    ("size", "sys_file", "size"),
    ("create_time", "sys_file", "create_time"),
]);

#[utoipa::path(
    get,
    path = "/api/v1/system/files",
    params(FileListQuery),
    responses((status = 200, description = "文件分页", body = FilePage))
)]
#[require_perm("system:file:list")]
pub async fn list_files(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Query(q): Query<FileListQuery>,
) -> Result<Json<FilePage>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let mut query = ScopedQuery::<FileEntity>::open().scoped(&ctx.scope());
    if let Some(name) = q.original_name.filter(|s| !s.is_empty()) {
        query = query.filter(FileColumn::OriginalName.fuzzy(&name));
    }
    if let Some(biz) = q.biz_type.filter(|s| !s.is_empty()) {
        query = query.filter(FileColumn::BizType.eq(biz));
    }
    // 分页约定：`page_size` 越界报 422（`CICADA_VALIDATION_4004`），不静默截断。
    let (page, page_size) = crate::page::resolve(q.page, q.page_size)?;
    let paged = query
        // 排序白名单：只认这张表声明过的列，客户端字符串进不了标识符位置。
        // 显式排序必须排在前面才真的生效：默认次序退化成同值行的收尾键（file_id），
        // 既不改变「不传 sort_by 时的结果」，又保证翻页不重不漏。
        .sorted_by(&FILE_SORT, q.sort_by.as_deref(), q.sort_order.as_deref())
        .order_by(FileColumn::FileId)
        .fetch_page(state.db.conn(), page, page_size)
        .await
        .map_err(db_error)?;
    Ok(Json(FilePage {
        items: paged.items.into_iter().map(FileVo::from).collect(),
        total: paged.total,
        page: paged.page,
        page_size: paged.page_size,
    }))
}

/// 取数据范围内的文件元数据（含被软删的排除）。
async fn scoped_file(
    state: &AppState,
    ctx: &cicada_kernel::RequestContext,
    file_id: i64,
) -> Result<FileModel, ApiErr> {
    ScopedQuery::<FileEntity>::open()
        .scoped(&ctx.scope())
        .filter(FileColumn::FileId.eq(file_id))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)
}

#[utoipa::path(
    get,
    path = "/api/v1/system/files/{file_id}",
    params(("file_id" = i64, Path, description = "文件 id")),
    responses((status = 200, description = "文件元数据", body = FileVo), (status = 404, description = "不存在"))
)]
#[require_perm("system:file:list")]
pub async fn get_file(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(file_id): Path<i64>,
) -> Result<Json<FileVo>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    Ok(Json(FileVo::from(
        scoped_file(&state, &ctx, file_id).await?,
    )))
}

#[utoipa::path(
    get,
    path = "/api/v1/system/files/{file_id}/content",
    params(("file_id" = i64, Path, description = "文件 id")),
    responses((status = 200, description = "文件内容"), (status = 404, description = "不存在"))
)]
#[require_perm("system:file:list")]
pub async fn download_file(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(file_id): Path<i64>,
) -> Result<Response, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let meta = scoped_file(&state, &ctx, file_id).await?;
    let bytes = state.storage.get(&meta.path).await.map_err(storage_error)?;
    Ok(content_response(&meta.mime, &meta.original_name, bytes))
}

#[utoipa::path(
    delete,
    path = "/api/v1/system/files/{file_id}",
    params(("file_id" = i64, Path, description = "文件 id")),
    responses((status = 204, description = "已删除"), (status = 404, description = "不存在"))
)]
#[require_perm("system:file:remove")]
#[audit(module = "system", action = "file:remove", kind = "delete")]
pub async fn delete_file(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(file_id): Path<i64>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    let meta = scoped_file(&state, &ctx, file_id).await?;
    let _ = state.storage.delete(&meta.path).await;
    FileActiveModel {
        file_id: Set(file_id),
        tenant_id: Set(meta.tenant_id),
        deleted_at: Set(Some(chrono::Utc::now())),
        update_by: Set(Some(principal.user_name.clone())),
        update_time: Set(Some(chrono::Utc::now())),
        ..Default::default()
    }
    .update(state.db.conn())
    .await
    .map_err(db_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// 短时链接的默认有效期（文件存储「短时 URL」）。
///
/// 300 秒是折中：够前端发起一次下载、也够用户点开一张头像，
/// 又短到「链接被复制到聊天里」不会变成长期通行证。
const DOWNLOAD_URL_TTL_SECS: u64 = 300;

/// 一条可用的短时下载链接。
#[derive(Debug, Serialize, ToSchema)]
pub struct DownloadUrlVo {
    /// 可直接访问的链接：预签名 URL 是绝对地址，一次性 token 是本站相对路径。
    pub url: String,
    /// `presigned`（桶端签名，直连对象存储）| `one_time_token`（本站签发，单次消费）。
    pub kind: String,
    /// 有效期（秒）。`presigned` 由桶端执行，`one_time_token` 由 Redis TTL 执行。
    pub expires_in: u64,
}

#[utoipa::path(
    post,
    path = "/api/v1/system/files/{file_id}/download-url",
    params(("file_id" = i64, Path, description = "文件 id")),
    responses(
        (status = 200, description = "短时下载链接", body = DownloadUrlVo),
        (status = 404, description = "不存在"),
        (status = 403, description = "无权限或越出数据范围")
    )
)]
#[require_perm("system:file:list")]
pub async fn issue_download_url(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(file_id): Path<i64>,
) -> Result<Json<DownloadUrlVo>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    // 授权发生在这一步：数据范围不过就 404，之后签出去的链接才允许脱离鉴权。
    let meta = scoped_file(&state, &ctx, file_id).await?;
    let ttl = Duration::from_secs(DOWNLOAD_URL_TTL_SECS);
    let url = match state.storage.presign_get(&meta.path, ttl).await {
        Ok(url) => {
            return Ok(Json(DownloadUrlVo {
                url,
                kind: "presigned".into(),
                expires_in: DOWNLOAD_URL_TTL_SECS,
            }));
        }
        // 本地与内存驱动签不出预签名 URL：退回一次性 token（文件存储的两条路径在此分岔）。
        Err(StorageError::Unsupported(_)) => state
            .grants
            .issue(
                &DownloadGrant {
                    file_id,
                    path: meta.path.clone(),
                },
                ttl,
            )
            .await
            .map_err(storage_error)?,
        Err(e) => return Err(storage_error(e)),
    };
    Ok(Json(DownloadUrlVo {
        url: format!("/api/v1/files/dl/{url}"),
        kind: "one_time_token".into(),
        expires_in: DOWNLOAD_URL_TTL_SECS,
    }))
}

/// 一次性 token 换文件内容——这条路由**不带鉴权**，凭证本身就是授权。
///
/// 安全性全靠三点：token 不可猜（UUID v4）、限时（Redis TTL）、用一次即废
/// （`redeem` 取出即删）。因此这里不再看权限点与数据范围，但必须复核
/// 「那一行还在、且路径与签发时一致」，否则链接就成了读取被替换内容的通道。
#[utoipa::path(
    get,
    path = "/api/v1/files/dl/{token}",
    params(("token" = String, Path, description = "一次性下载凭证")),
    responses(
        (status = 200, description = "文件内容"),
        (status = 404, description = "链接无效、已过期或已被用过")
    )
)]
pub async fn download_by_token(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Response, ApiErr> {
    let Some(grant) = state.grants.redeem(&token).await.map_err(storage_error)? else {
        return Err(not_found());
    };
    // 按主键取行：授权已在签发时做过，这里不再套数据范围。
    let meta = FileEntity::find_by_id(grant.file_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    if meta.deleted_at.is_some() || meta.path != grant.path {
        return Err(not_found());
    }
    let bytes = state.storage.get(&meta.path).await.map_err(storage_error)?;
    Ok(content_response(&meta.mime, &meta.original_name, bytes))
}

/// 附件响应：`Content-Type` 用 magic number 判定的 MIME，`Content-Disposition` 带原始名。
fn content_response(mime: &str, filename: &str, bytes: Vec<u8>) -> Response {
    let ascii: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let disposition = format!(
        "attachment; filename=\"{ascii}\"; filename*=UTF-8''{}",
        export::percent_encode(filename)
    );
    let mut resp = (StatusCode::OK, bytes).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition)
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    resp
}

pub struct FileModule;

impl Module for FileModule {
    fn manifest(self: Box<Self>) -> Manifest {
        Manifest::new("cicada:system-files")
            .perms(PERMS.iter().map(|p| Permission::new(*p)))
            .routes(routes)
    }
}

pub fn routes(_state: AppState) -> Routes {
    Routes::new()
        .route_audited(
            "/api/v1/system/files",
            post(upload_file),
            upload_file__perms(),
            upload_file__audit(),
        )
        .route("/api/v1/system/files", get(list_files), list_files__perms())
        .route(
            "/api/v1/system/files/{file_id}",
            get(get_file),
            get_file__perms(),
        )
        .route(
            "/api/v1/system/files/{file_id}/content",
            get(download_file),
            download_file__perms(),
        )
        .route(
            "/api/v1/system/files/{file_id}/download-url",
            post(issue_download_url),
            issue_download_url__perms(),
        )
        // 公开：凭证即授权（见 `download_by_token`）。空权限列表 = 不挂鉴权守卫。
        .route("/api/v1/files/dl/{token}", get(download_by_token), &[])
        .route_audited(
            "/api/v1/system/files/{file_id}",
            delete(delete_file),
            delete_file__perms(),
            delete_file__audit(),
        )
}
