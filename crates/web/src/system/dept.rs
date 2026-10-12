//! 部门管理（权限点 `system:dept:*`）。
//!
//! 部门是树：写操作维护 `ancestors`（`0,100,110` 形式），改父节点时级联重写子树
//! 路径，并阻止把节点挂到自身/后代下（防环）。`sys_dept` 无归属维度，列表走
//! 受控查询 `Query::<DeptEntity>`：非「全部」数据范围按 `NoOwnership` 规则失败关闭
//! （返回空集，见 datascope-typestate.md）。部门树变化会影响 `DeptAndChild` 数据范围
//! 的展开，故每次写后按版本号整体失效权限缓存。

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post, put};
use sea_orm::{ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use cicada_kernel::scope::Query as ScopedQuery;
use cicada_macros::{audit, require_perm};
use cicada_system::entity::{
    DeptActiveModel, DeptColumn, DeptEntity, DeptModel, UserColumn, UserEntity,
};

use super::{
    ApiErr, bad_request, db_error, field_error, not_found, referenced, tree_error, unauthorized,
    version_conflict,
};
use crate::Problem;
use crate::extractor::RequestContextExt;
use crate::response::created;
use crate::routes::Routes;
use crate::state::AppState;

pub const PERMS: &[&str] = &[
    "system:dept:list",
    "system:dept:add",
    "system:dept:edit",
    "system:dept:remove",
];

#[derive(Debug, Serialize, ToSchema)]
pub struct DeptVo {
    pub dept_id: i64,
    pub parent_id: i64,
    pub dept_name: String,
    pub ancestors: String,
    pub order_num: i32,
    pub leader: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub status: i16,
    /// 递归树：打破 OpenAPI schema 递归。
    #[schema(no_recursion)]
    pub children: Vec<DeptVo>,
    /// 乐观锁版本（建模规范）。读接口回传，`PUT` 原样带回才做冲突检测。
    pub version: i32,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DeptUpsert {
    #[serde(default)]
    pub parent_id: i64,
    pub dept_name: String,
    #[serde(default)]
    pub order_num: i32,
    pub leader: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    #[serde(default)]
    pub status: i16,
    pub remark: Option<String>,
    /// 乐观锁期望版本（建模规范），只有 `PUT` 读它；`POST` 建号时新版本恒为 0。
    pub version: Option<i32>,
}

fn leaf(m: DeptModel) -> DeptVo {
    DeptVo {
        dept_id: m.dept_id,
        parent_id: m.parent_id,
        dept_name: m.dept_name,
        ancestors: m.ancestors,
        order_num: m.order_num,
        leader: m.leader,
        phone: m.phone,
        email: m.email,
        status: m.status,
        children: Vec::new(),
        version: m.version,
    }
}

fn build_tree(nodes: Vec<DeptVo>) -> Vec<DeptVo> {
    let mut by_id: HashMap<i64, DeptVo> = HashMap::new();
    let mut children: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut roots: Vec<i64> = Vec::new();
    let ids: Vec<i64> = nodes.iter().map(|n| n.dept_id).collect();
    for n in nodes {
        children.entry(n.parent_id).or_default().push(n.dept_id);
        by_id.insert(n.dept_id, n);
    }
    for id in &ids {
        let parent = by_id.get(id).map(|n| n.parent_id).unwrap_or(0);
        if parent == 0 || !by_id.contains_key(&parent) {
            roots.push(*id);
        }
    }
    fn assemble(
        id: i64,
        by_id: &mut HashMap<i64, DeptVo>,
        children: &HashMap<i64, Vec<i64>>,
    ) -> DeptVo {
        let mut node = by_id.remove(&id).expect("节点存在");
        if let Some(kids) = children.get(&id) {
            for kid in kids {
                node.children.push(assemble(*kid, by_id, children));
            }
        }
        node
    }
    let roots: Vec<i64> = roots
        .into_iter()
        .filter(|id| by_id.contains_key(id))
        .collect();
    let mut out: Vec<DeptVo> = roots
        .into_iter()
        .map(|id| assemble(id, &mut by_id, &children))
        .collect();
    out.sort_by_key(|n| (n.order_num, n.dept_id));
    out
}

async fn load_tenant_depts(
    db: &impl ConnectionTrait,
    tenant_id: i64,
) -> Result<Vec<DeptModel>, ApiErr> {
    DeptEntity::find()
        .filter(DeptColumn::TenantId.eq(tenant_id))
        .filter(DeptColumn::DeletedAt.is_null())
        .all(db)
        .await
        .map_err(db_error)
}

/// 计算某父节点下的 `ancestors`：根为 `0`，否则 `父.ancestors,父.dept_id`。
fn ancestors_of(parent: &DeptModel) -> String {
    format!("{},{}", parent.ancestors, parent.dept_id)
}

#[utoipa::path(
    get,
    path = "/api/v1/system/depts",
    responses((status = 200, description = "部门树", body = [DeptVo]))
)]
#[require_perm("system:dept:list")]
pub async fn list_depts(
    State(state): State<AppState>,
    ctx: RequestContextExt,
) -> Result<Json<Vec<DeptVo>>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let rows = ScopedQuery::<DeptEntity>::open()
        .scoped(&ctx.scope())
        .order_by(DeptColumn::ParentId)
        .order_by(DeptColumn::OrderNum)
        .order_by(DeptColumn::DeptId)
        .all(state.db.conn())
        .await
        .map_err(db_error)?;
    Ok(Json(build_tree(rows.into_iter().map(leaf).collect())))
}

/// 接口清单同时承诺 `/system/depts`（列表）与 `/system/depts/tree`（树），而这里的响应
/// 本就是树，所以别名只是转调。单独一个 handler 是因为 `#[utoipa::path]` 一个函数
/// 只能挂一条路径——不写它，`/depts/tree` 就不在 OpenAPI 文档里（横切能力的快照看不见它）。
#[utoipa::path(
    get,
    path = "/api/v1/system/depts/tree",
    responses((status = 200, description = "部门树", body = [DeptVo]))
)]
#[require_perm("system:dept:list")]
pub async fn list_depts_tree(
    State(state): State<AppState>,
    ctx: RequestContextExt,
) -> Result<Json<Vec<DeptVo>>, ApiErr> {
    list_depts(State(state), ctx).await
}

#[utoipa::path(
    get,
    path = "/api/v1/system/depts/{dept_id}",
    params(("dept_id" = i64, Path, description = "部门 id")),
    responses((status = 200, description = "部门详情", body = DeptVo), (status = 404, description = "不存在"))
)]
#[require_perm("system:dept:list")]
pub async fn get_dept(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(dept_id): Path<i64>,
) -> Result<Json<DeptVo>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let dept = ScopedQuery::<DeptEntity>::open()
        .scoped(&ctx.scope())
        .filter(DeptColumn::DeptId.eq(dept_id))
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    Ok(Json(leaf(dept)))
}

#[utoipa::path(
    post,
    path = "/api/v1/system/depts",
    request_body = DeptUpsert,
    responses((status = 201, description = "已创建", body = DeptVo), (status = 422, description = "父部门不存在"))
)]
#[require_perm("system:dept:add")]
#[audit(module = "system", action = "dept:add", kind = "insert")]
pub async fn create_dept(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<DeptUpsert>,
) -> Result<(StatusCode, HeaderMap, Json<DeptVo>), ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    if req.dept_name.trim().is_empty() {
        return Err(bad_request("部门名称不能为空"));
    }
    let tenant = ctx.tenant_id.get();
    let txn = state.db.begin().await.map_err(db_error)?;

    let ancestors = if req.parent_id == 0 {
        "0".to_string()
    } else {
        let parent = DeptEntity::find()
            .filter(DeptColumn::DeptId.eq(req.parent_id))
            .filter(DeptColumn::TenantId.eq(tenant))
            .filter(DeptColumn::DeletedAt.is_null())
            .one(&txn)
            .await
            .map_err(db_error)?
            .ok_or_else(|| bad_request("父部门不存在"))?;
        ancestors_of(&parent)
    };

    let dept = DeptActiveModel {
        tenant_id: Set(tenant),
        parent_id: Set(req.parent_id),
        ancestors: Set(ancestors),
        dept_name: Set(req.dept_name.trim().to_string()),
        order_num: Set(req.order_num),
        leader: Set(req.leader.clone()),
        phone: Set(req.phone.clone()),
        email: Set(req.email.clone()),
        status: Set(req.status),
        version: Set(0),
        remark: Set(req.remark.clone()),
        create_by: Set(Some(principal.user_name.clone())),
        create_time: Set(chrono::Utc::now()),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(db_error)?;
    txn.commit().await.map_err(db_error)?;

    state.authz.invalidate_tenant(ctx.tenant_id).await;
    Ok(created(
        format!("/api/v1/system/depts/{}", dept.dept_id),
        leaf(dept),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/system/depts/{dept_id}",
    request_body = DeptUpsert,
    responses((status = 200, description = "已更新", body = DeptVo), (status = 404, description = "不存在"), (status = 422, description = "父部门非法"))
)]
#[require_perm("system:dept:edit")]
#[audit(module = "system", action = "dept:edit", kind = "update")]
pub async fn update_dept(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(dept_id): Path<i64>,
    Json(req): Json<DeptUpsert>,
) -> Result<Json<DeptVo>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    if req.dept_name.trim().is_empty() {
        return Err(bad_request("部门名称不能为空"));
    }
    let tenant = ctx.tenant_id.get();
    let txn = state.db.begin().await.map_err(db_error)?;

    let current = DeptEntity::find()
        .filter(DeptColumn::DeptId.eq(dept_id))
        .filter(DeptColumn::TenantId.eq(tenant))
        .filter(DeptColumn::DeletedAt.is_null())
        .one(&txn)
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;

    let all = load_tenant_depts(&txn, tenant).await?;
    let self_path = format!("{},{}", current.ancestors, current.dept_id);

    // 新父节点：0 合法；否则必须存在、不能是自身、不能是自己的后代（防环）。
    let new_ancestors = if req.parent_id == 0 {
        "0".to_string()
    } else {
        if req.parent_id == dept_id {
            return Err(tree_error("父部门不能是自身"));
        }
        let parent = all
            .iter()
            .find(|d| d.dept_id == req.parent_id)
            .ok_or_else(|| bad_request("父部门不存在"))?;
        let parent_path = format!("{},{}", parent.ancestors, parent.dept_id);
        if parent_path == self_path || parent_path.starts_with(&format!("{self_path},")) {
            return Err(tree_error("父部门不能是自己的下级部门"));
        }
        parent_path
    };

    // 留痕快照（审计设计）：本接口能改的列。
    let before = serde_json::json!({
        "parent_id": current.parent_id,
        "ancestors": current.ancestors,
        "dept_name": current.dept_name,
        "order_num": current.order_num,
        "leader": current.leader,
        "phone": current.phone,
        "email": current.email,
        "status": current.status,
        "remark": current.remark,
    });

    // 建模规范乐观锁：手写 CAS（SeaORM 2.0.4 没有原生 opt-lock）。
    // 只对本节点做版本判定——下面的子树 ancestors 重写是结构推导，不是并发编辑目标。
    let expected = req.version.unwrap_or(current.version);
    let applied = DeptEntity::update_many()
        .col_expr(
            DeptColumn::ParentId,
            sea_orm::sea_query::Expr::value(req.parent_id),
        )
        .col_expr(
            DeptColumn::Ancestors,
            sea_orm::sea_query::Expr::value(new_ancestors.clone()),
        )
        .col_expr(
            DeptColumn::DeptName,
            sea_orm::sea_query::Expr::value(req.dept_name.trim().to_string()),
        )
        .col_expr(
            DeptColumn::OrderNum,
            sea_orm::sea_query::Expr::value(req.order_num),
        )
        .col_expr(
            DeptColumn::Leader,
            sea_orm::sea_query::Expr::value(req.leader.clone()),
        )
        .col_expr(
            DeptColumn::Phone,
            sea_orm::sea_query::Expr::value(req.phone.clone()),
        )
        .col_expr(
            DeptColumn::Email,
            sea_orm::sea_query::Expr::value(req.email.clone()),
        )
        .col_expr(
            DeptColumn::Status,
            sea_orm::sea_query::Expr::value(req.status),
        )
        .col_expr(
            DeptColumn::Remark,
            sea_orm::sea_query::Expr::value(req.remark.clone()),
        )
        .col_expr(
            DeptColumn::UpdateBy,
            sea_orm::sea_query::Expr::value(Some(principal.user_name.clone())),
        )
        .col_expr(
            DeptColumn::UpdateTime,
            sea_orm::sea_query::Expr::value(Some(chrono::Utc::now())),
        )
        .col_expr(
            DeptColumn::Version,
            sea_orm::sea_query::Expr::value(expected + 1),
        )
        .filter(DeptColumn::DeptId.eq(dept_id))
        .filter(DeptColumn::Version.eq(expected))
        .exec(&txn)
        .await
        .map_err(db_error)?;
    if applied.rows_affected == 0 {
        return Err(version_conflict());
    }

    // 父节点变化时级联重写子树 ancestors 前缀。
    if new_ancestors != current.ancestors {
        let prefix = format!("{self_path},");
        for d in all.iter().filter(|d| d.dept_id != dept_id) {
            if d.ancestors == self_path || d.ancestors.starts_with(&prefix) {
                let suffix = &d.ancestors[self_path.len()..];
                DeptActiveModel {
                    dept_id: Set(d.dept_id),
                    ancestors: Set(format!("{new_ancestors}{suffix}")),
                    ..Default::default()
                }
                .update(&txn)
                .await
                .map_err(db_error)?;
            }
        }
    }
    txn.commit().await.map_err(db_error)?;

    state.authz.invalidate_tenant(ctx.tenant_id).await;
    let updated = DeptEntity::find_by_id(dept_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    crate::audit::record_change(
        &state.audit,
        &ctx,
        "sys_dept",
        dept_id.to_string(),
        before,
        serde_json::json!({
            "parent_id": updated.parent_id,
            "ancestors": updated.ancestors,
            "dept_name": updated.dept_name,
            "order_num": updated.order_num,
            "leader": updated.leader,
            "phone": updated.phone,
            "email": updated.email,
            "status": updated.status,
            "remark": updated.remark,
        }),
    )
    .await;
    Ok(Json(leaf(updated)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/system/depts/{dept_id}",
    params(("dept_id" = i64, Path, description = "部门 id")),
    responses((status = 204, description = "已删除"), (status = 409, description = "存在下级部门或已分配用户"))
)]
#[require_perm("system:dept:remove")]
#[audit(module = "system", action = "dept:remove", kind = "delete")]
pub async fn delete_dept(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(dept_id): Path<i64>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let tenant = ctx.tenant_id.get();
    let txn = state.db.begin().await.map_err(db_error)?;
    DeptEntity::find()
        .filter(DeptColumn::DeptId.eq(dept_id))
        .filter(DeptColumn::TenantId.eq(tenant))
        .filter(DeptColumn::DeletedAt.is_null())
        .one(&txn)
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;

    let has_children = DeptEntity::find()
        .filter(DeptColumn::ParentId.eq(dept_id))
        .filter(DeptColumn::TenantId.eq(tenant))
        .filter(DeptColumn::DeletedAt.is_null())
        .one(&txn)
        .await
        .map_err(db_error)?
        .is_some();
    if has_children {
        return Err(referenced("存在下级部门，不能删除"));
    }
    let has_users = UserEntity::find()
        .filter(UserColumn::DeptId.eq(dept_id))
        .filter(UserColumn::TenantId.eq(tenant))
        .filter(UserColumn::DeletedAt.is_null())
        .one(&txn)
        .await
        .map_err(db_error)?
        .is_some();
    if has_users {
        return Err(referenced("部门下存在用户，不能删除"));
    }

    let now = chrono::Utc::now();
    DeptActiveModel {
        dept_id: Set(dept_id),
        deleted_at: Set(Some(now)),
        update_time: Set(Some(now)),
        ..Default::default()
    }
    .update(&txn)
    .await
    .map_err(db_error)?;
    txn.commit().await.map_err(db_error)?;

    state.authz.invalidate_tenant(ctx.tenant_id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// 排序：只改同级顺序。挪动部门（改父节点）走 `PUT /depts/{id}`——那里才有
/// `ancestors` 的级联重写，这里重复实现一份只会长出第二套会写错的树逻辑。
#[derive(Debug, Deserialize, ToSchema)]
pub struct DeptSort {
    pub order_num: i32,
}

#[utoipa::path(
    put,
    path = "/api/v1/system/depts/{dept_id}/sort",
    params(("dept_id" = i64, Path, description = "部门 id")),
    request_body = DeptSort,
    responses(
        (status = 200, description = "已排序", body = DeptVo),
        (status = 400, description = "参数非法", body = Problem),
        (status = 404, description = "不存在")
    )
)]
#[require_perm("system:dept:edit")]
#[audit(module = "system", action = "dept:sort", kind = "update")]
pub async fn sort_dept(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(dept_id): Path<i64>,
    Json(req): Json<DeptSort>,
) -> Result<Json<DeptVo>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    if req.order_num < 0 {
        return Err(field_error("order_num", "不能为负"));
    }
    let tenant = ctx.tenant_id.get();
    let current = DeptEntity::find()
        .filter(DeptColumn::DeptId.eq(dept_id))
        .filter(DeptColumn::TenantId.eq(tenant))
        .filter(DeptColumn::DeletedAt.is_null())
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;

    DeptActiveModel {
        dept_id: Set(dept_id),
        order_num: Set(req.order_num),
        update_by: Set(Some(principal.user_name.clone())),
        update_time: Set(Some(chrono::Utc::now())),
        ..Default::default()
    }
    .update(state.db.conn())
    .await
    .map_err(db_error)?;

    state.authz.invalidate_tenant(ctx.tenant_id).await;
    crate::audit::record_change(
        &state.audit,
        &ctx,
        "sys_dept",
        dept_id.to_string(),
        serde_json::json!({ "order_num": current.order_num }),
        serde_json::json!({ "order_num": req.order_num }),
    )
    .await;
    let updated = DeptEntity::find_by_id(dept_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    Ok(Json(leaf(updated)))
}

pub fn routes(_state: AppState) -> Routes {
    Routes::new()
        .route("/api/v1/system/depts", get(list_depts), list_depts__perms())
        .route(
            "/api/v1/system/depts/tree",
            get(list_depts_tree),
            list_depts_tree__perms(),
        )
        .route(
            "/api/v1/system/depts/{dept_id}",
            get(get_dept),
            get_dept__perms(),
        )
        .route_audited(
            "/api/v1/system/depts",
            post(create_dept),
            create_dept__perms(),
            create_dept__audit(),
        )
        .route_audited(
            "/api/v1/system/depts/{dept_id}",
            put(update_dept),
            update_dept__perms(),
            update_dept__audit(),
        )
        .route_audited(
            "/api/v1/system/depts/{dept_id}",
            delete(delete_dept),
            delete_dept__perms(),
            delete_dept__audit(),
        )
        .route_audited(
            "/api/v1/system/depts/{dept_id}/sort",
            put(sort_dept),
            sort_dept__perms(),
            sort_dept__audit(),
        )
}
