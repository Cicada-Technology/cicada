//! 菜单管理（权限点 `system:menu:*`）。
//!
//! 菜单是全局字典（无 `tenant_id`，见 `sys_menu` 实体说明）；其 `perms` 字段是
//! 权限点的来源，任何写操作都按版本号整体失效权限缓存。

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post, put};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, Set,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use cicada_macros::{audit, require_perm};
use cicada_system::entity::{
    MenuActiveModel, MenuColumn, MenuEntity, MenuModel, RoleColumn, RoleEntity, RoleMenuColumn,
    RoleMenuEntity,
};

use super::{ApiErr, bad_request, db_error, field_error, not_found, tree_error, unauthorized};
use crate::Problem;
use crate::extractor::RequestContextExt;
use crate::response::created;
use crate::routes::Routes;
use crate::state::AppState;

pub const PERMS: &[&str] = &[
    "system:menu:list",
    "system:menu:add",
    "system:menu:edit",
    "system:menu:remove",
];

#[derive(Debug, Serialize, ToSchema)]
pub struct MenuVo {
    pub menu_id: i64,
    pub menu_name: String,
    pub parent_id: i64,
    pub order_num: i32,
    pub menu_type: String,
    pub path: Option<String>,
    pub component: Option<String>,
    pub perms: Option<String>,
    pub icon: Option<String>,
    pub visible: i16,
    pub status: i16,
    /// 递归树：打破 OpenAPI schema 递归（`#[schema(no_recursion)]`）。
    #[schema(no_recursion)]
    pub children: Vec<MenuVo>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct MenuUpsert {
    pub menu_name: String,
    #[serde(default)]
    pub parent_id: i64,
    #[serde(default)]
    pub order_num: i32,
    /// `M` 目录 / `C` 菜单 / `F` 按钮。
    pub menu_type: String,
    pub path: Option<String>,
    pub component: Option<String>,
    pub query: Option<String>,
    pub perms: Option<String>,
    pub icon: Option<String>,
    #[serde(default)]
    pub is_frame: i16,
    #[serde(default)]
    pub is_cache: i16,
    #[serde(default)]
    pub visible: i16,
    #[serde(default)]
    pub status: i16,
    pub remark: Option<String>,
}

fn leaf(m: MenuModel) -> MenuVo {
    MenuVo {
        menu_id: m.menu_id,
        menu_name: m.menu_name,
        parent_id: m.parent_id,
        order_num: m.order_num,
        menu_type: m.menu_type,
        path: m.path,
        component: m.component,
        perms: m.perms,
        icon: m.icon,
        visible: m.visible,
        status: m.status,
        children: Vec::new(),
    }
}

fn build_tree(nodes: Vec<MenuVo>) -> Vec<MenuVo> {
    let mut by_id: HashMap<i64, MenuVo> = nodes.into_iter().map(|n| (n.menu_id, n)).collect();
    let roots: Vec<i64> = by_id
        .values()
        .filter(|n| n.parent_id == 0 || !by_id.contains_key(&n.parent_id))
        .map(|n| n.menu_id)
        .collect();
    let mut children: HashMap<i64, Vec<i64>> = HashMap::new();
    for n in by_id.values() {
        if n.parent_id != 0 {
            children.entry(n.parent_id).or_default().push(n.menu_id);
        }
    }

    fn assemble(
        id: i64,
        by_id: &mut HashMap<i64, MenuVo>,
        children: &HashMap<i64, Vec<i64>>,
    ) -> MenuVo {
        let mut node = by_id.remove(&id).expect("root 在 by_id 中");
        if let Some(kids) = children.get(&id) {
            let mut kids = kids.clone();
            kids.sort_by_key(|k| by_id.get(k).map(|n| (n.order_num, n.menu_id)));
            for kid in kids {
                if by_id.contains_key(&kid) {
                    node.children.push(assemble(kid, by_id, children));
                }
            }
        }
        node
    }

    let mut roots: Vec<MenuVo> = roots
        .into_iter()
        .map(|id| assemble(id, &mut by_id, &children))
        .collect();
    roots.sort_by_key(|n| (n.order_num, n.menu_id));
    roots
}

fn valid_menu_type(t: &str) -> bool {
    matches!(t, "M" | "C" | "F")
}

#[utoipa::path(
    get,
    path = "/api/v1/system/menus",
    responses((status = 200, description = "菜单树", body = [MenuVo]))
)]
#[require_perm("system:menu:list")]
pub async fn list_menus(
    State(state): State<AppState>,
    ctx: RequestContextExt,
) -> Result<Json<Vec<MenuVo>>, ApiErr> {
    if ctx.into_inner().principal.is_none() {
        return Err(unauthorized());
    }
    let rows = MenuEntity::find()
        .order_by_asc(MenuColumn::ParentId)
        .order_by_asc(MenuColumn::OrderNum)
        .all(state.db.conn())
        .await
        .map_err(db_error)?;
    Ok(Json(build_tree(rows.into_iter().map(leaf).collect())))
}

/// 接口清单同时承诺 `/system/menus`（列表）与 `/system/menus/tree`（树），这里的响应
/// 本就是树，别名只转调；单独成函数是为了让这条路径进 OpenAPI（理由同 `list_depts_tree`）。
#[utoipa::path(
    get,
    path = "/api/v1/system/menus/tree",
    responses((status = 200, description = "菜单树", body = [MenuVo]))
)]
#[require_perm("system:menu:list")]
pub async fn list_menus_tree(
    State(state): State<AppState>,
    ctx: RequestContextExt,
) -> Result<Json<Vec<MenuVo>>, ApiErr> {
    list_menus(State(state), ctx).await
}

#[utoipa::path(
    post,
    path = "/api/v1/system/menus",
    request_body = MenuUpsert,
    responses((status = 201, description = "已创建", body = MenuVo))
)]
#[require_perm("system:menu:add")]
#[audit(module = "system", action = "menu:add", kind = "insert")]
pub async fn create_menu(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Json(req): Json<MenuUpsert>,
) -> Result<(StatusCode, HeaderMap, Json<MenuVo>), ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    if req.menu_name.trim().is_empty() || !valid_menu_type(&req.menu_type) {
        return Err(bad_request("菜单名不能为空，menu_type 只能是 M/C/F"));
    }
    let menu = MenuActiveModel {
        menu_name: Set(req.menu_name.trim().to_string()),
        parent_id: Set(req.parent_id),
        order_num: Set(req.order_num),
        path: Set(req.path.clone()),
        component: Set(req.component.clone()),
        query: Set(req.query.clone()),
        is_frame: Set(req.is_frame),
        is_cache: Set(req.is_cache),
        menu_type: Set(req.menu_type.clone()),
        visible: Set(req.visible),
        status: Set(req.status),
        perms: Set(req.perms.clone()),
        icon: Set(req.icon.clone()),
        remark: Set(req.remark.clone()),
        create_by: Set(Some(principal.user_name.clone())),
        create_time: Set(chrono::Utc::now()),
        ..Default::default()
    }
    .insert(state.db.conn())
    .await
    .map_err(db_error)?;

    super::invalidate_grant_caches(&state, ctx.tenant_id).await;
    Ok(created(
        format!("/api/v1/system/menus/{}", menu.menu_id),
        leaf(menu),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/system/menus/{menu_id}",
    request_body = MenuUpsert,
    responses((status = 200, description = "已更新", body = MenuVo), (status = 404, description = "不存在"))
)]
#[require_perm("system:menu:edit")]
#[audit(module = "system", action = "menu:edit", kind = "update")]
pub async fn update_menu(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(menu_id): Path<i64>,
    Json(req): Json<MenuUpsert>,
) -> Result<Json<MenuVo>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    if !valid_menu_type(&req.menu_type) {
        return Err(bad_request("menu_type 只能是 M/C/F"));
    }
    if req.parent_id == menu_id {
        return Err(tree_error("上级菜单不能是自己"));
    }
    let existing = MenuEntity::find_by_id(menu_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;

    // 留痕快照（审计设计）：本接口能改的列。
    let before = serde_json::json!({
        "menu_name": existing.menu_name,
        "parent_id": existing.parent_id,
        "order_num": existing.order_num,
        "path": existing.path,
        "component": existing.component,
        "query": existing.query,
        "is_frame": existing.is_frame,
        "is_cache": existing.is_cache,
        "menu_type": existing.menu_type,
        "visible": existing.visible,
        "status": existing.status,
        "perms": existing.perms,
    });

    let menu = MenuActiveModel {
        menu_id: Set(menu_id),
        menu_name: Set(req.menu_name.trim().to_string()),
        parent_id: Set(req.parent_id),
        order_num: Set(req.order_num),
        path: Set(req.path.clone()),
        component: Set(req.component.clone()),
        query: Set(req.query.clone()),
        is_frame: Set(req.is_frame),
        is_cache: Set(req.is_cache),
        menu_type: Set(req.menu_type.clone()),
        visible: Set(req.visible),
        status: Set(req.status),
        perms: Set(req.perms.clone()),
        icon: Set(req.icon.clone()),
        remark: Set(req.remark.clone()),
        create_by: Set(existing.create_by),
        create_time: Set(existing.create_time),
        update_by: Set(Some(principal.user_name.clone())),
        update_time: Set(Some(chrono::Utc::now())),
    }
    .update(state.db.conn())
    .await
    .map_err(db_error)?;

    super::invalidate_grant_caches(&state, ctx.tenant_id).await;
    crate::audit::record_change(
        &state.audit,
        &ctx,
        "sys_menu",
        menu_id.to_string(),
        before,
        serde_json::json!({
            "menu_name": menu.menu_name,
            "parent_id": menu.parent_id,
            "order_num": menu.order_num,
            "path": menu.path,
            "component": menu.component,
            "query": menu.query,
            "is_frame": menu.is_frame,
            "is_cache": menu.is_cache,
            "menu_type": menu.menu_type,
            "visible": menu.visible,
            "status": menu.status,
            "perms": menu.perms,
        }),
    )
    .await;
    Ok(Json(leaf(menu)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/system/menus/{menu_id}",
    params(("menu_id" = i64, Path, description = "菜单 id")),
    responses(
        (status = 204, description = "已删除"),
        (status = 409, description = "存在子菜单"),
        (status = 404, description = "不存在")
    )
)]
#[require_perm("system:menu:remove")]
#[audit(module = "system", action = "menu:remove", kind = "delete")]
pub async fn delete_menu(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(menu_id): Path<i64>,
) -> Result<StatusCode, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    let exists = MenuEntity::find_by_id(menu_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    let children = MenuEntity::find()
        .filter(MenuColumn::ParentId.eq(exists.menu_id))
        .count(state.db.conn())
        .await
        .map_err(db_error)?;
    if children > 0 {
        return Err(super::referenced("存在子菜单，不能删除"));
    }
    MenuEntity::delete_by_id(menu_id)
        .exec(state.db.conn())
        .await
        .map_err(db_error)?;

    super::invalidate_grant_caches(&state, ctx.tenant_id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// 菜单全表的 `(id, parent_id)`，用于环检查——树只有这一份真相，不必再查第二遍。
async fn parent_map(state: &AppState) -> Result<HashMap<i64, i64>, ApiErr> {
    let rows = MenuEntity::find()
        .all(state.db.conn())
        .await
        .map_err(db_error)?;
    Ok(rows.into_iter().map(|m| (m.menu_id, m.parent_id)).collect())
}

/// 新父级不能是自己，也不能是自己的后代，否则树里出现环。
fn assert_no_cycle(
    parents: &HashMap<i64, i64>,
    menu_id: i64,
    parent_id: i64,
) -> Result<(), ApiErr> {
    if parent_id == menu_id {
        return Err(tree_error("上级菜单不能是自己"));
    }
    let mut cursor = parent_id;
    // 层数上界取节点数：超过就一定在绕圈。
    for _ in 0..=parents.len() {
        if cursor == 0 {
            return Ok(());
        }
        if cursor == menu_id {
            return Err(tree_error("上级菜单不能是自己的子菜单"));
        }
        match parents.get(&cursor) {
            Some(p) => cursor = *p,
            None => return Ok(()),
        }
    }
    Err(tree_error("菜单层级有环"))
}

#[utoipa::path(
    get,
    path = "/api/v1/system/menus/{menu_id}",
    params(("menu_id" = i64, Path, description = "菜单 id")),
    responses(
        (status = 200, description = "菜单详情（编辑对话框要的平铺单行）", body = MenuVo),
        (status = 404, description = "不存在")
    )
)]
#[require_perm("system:menu:list")]
pub async fn get_menu(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(menu_id): Path<i64>,
) -> Result<Json<MenuVo>, ApiErr> {
    if ctx.into_inner().principal.is_none() {
        return Err(unauthorized());
    }
    let menu = MenuEntity::find_by_id(menu_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    Ok(Json(leaf(menu)))
}

/// 排序：只改 `order_num`（可选同时挂到新父级）。
#[derive(Debug, Deserialize, ToSchema)]
pub struct MenuSort {
    /// 省略表示不动父级。
    pub parent_id: Option<i64>,
    pub order_num: i32,
}

#[utoipa::path(
    put,
    path = "/api/v1/system/menus/{menu_id}/sort",
    params(("menu_id" = i64, Path, description = "菜单 id")),
    request_body = MenuSort,
    responses(
        (status = 200, description = "已排序", body = MenuVo),
        (status = 400, description = "层级不合法", body = Problem),
        (status = 404, description = "不存在")
    )
)]
#[require_perm("system:menu:edit")]
#[audit(module = "system", action = "menu:sort", kind = "update")]
pub async fn sort_menu(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(menu_id): Path<i64>,
    Json(req): Json<MenuSort>,
) -> Result<Json<MenuVo>, ApiErr> {
    let ctx = ctx.into_inner();
    let Some(principal) = &ctx.principal else {
        return Err(unauthorized());
    };
    if req.order_num < 0 {
        return Err(field_error("order_num", "不能为负"));
    }
    let existing = MenuEntity::find_by_id(menu_id)
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    let parent_id = req.parent_id.unwrap_or(existing.parent_id);
    if parent_id != 0 && !parent_map(&state).await?.contains_key(&parent_id) {
        return Err(field_error("parent_id", "上级菜单不存在"));
    }
    assert_no_cycle(&parent_map(&state).await?, menu_id, parent_id)?;

    let menu = MenuActiveModel {
        menu_id: Set(menu_id),
        parent_id: Set(parent_id),
        order_num: Set(req.order_num),
        update_by: Set(Some(principal.user_name.clone())),
        update_time: Set(Some(chrono::Utc::now())),
        ..Default::default()
    }
    .update(state.db.conn())
    .await
    .map_err(db_error)?;

    super::invalidate_grant_caches(&state, ctx.tenant_id).await;
    crate::audit::record_change(
        &state.audit,
        &ctx,
        "sys_menu",
        menu_id.to_string(),
        serde_json::json!({ "parent_id": existing.parent_id, "order_num": existing.order_num }),
        serde_json::json!({ "parent_id": menu.parent_id, "order_num": menu.order_num }),
    )
    .await;
    Ok(Json(leaf(menu)))
}

/// 角色的菜单勾选树（接口清单的 `role-tree/{roleId}`）：整棵树 + 该角色已勾中的菜单 id。
///
/// 与 `GET /system/roles/{role_id}/menus`（平铺 id，提交时整体替换用）的分工：
/// 那个给「读回既有授权」，这个给「树形勾选界面」。
#[derive(Debug, Serialize, ToSchema)]
pub struct RoleMenuTree {
    pub tree: Vec<MenuVo>,
    pub checked: Vec<i64>,
}

#[utoipa::path(
    get,
    path = "/api/v1/system/menus/role-tree/{role_id}",
    params(("role_id" = i64, Path, description = "角色 id")),
    responses(
        (status = 200, description = "菜单树 + 勾选状态", body = RoleMenuTree),
        (status = 404, description = "角色不存在")
    )
)]
#[require_perm("system:menu:list")]
pub async fn role_menu_tree(
    State(state): State<AppState>,
    ctx: RequestContextExt,
    Path(role_id): Path<i64>,
) -> Result<Json<RoleMenuTree>, ApiErr> {
    let ctx = ctx.into_inner();
    if ctx.principal.is_none() {
        return Err(unauthorized());
    }
    // 角色必须先确认属于当前租户：否则别人拿 role_id 就能读到不属于自己的勾选状态。
    let role_exists = RoleEntity::find()
        .filter(RoleColumn::RoleId.eq(role_id))
        .filter(RoleColumn::TenantId.eq(ctx.tenant_id.get()))
        .filter(RoleColumn::DeletedAt.is_null())
        .one(state.db.conn())
        .await
        .map_err(db_error)?
        .is_some();
    if !role_exists {
        return Err(not_found());
    }
    let rows = MenuEntity::find()
        .order_by_asc(MenuColumn::ParentId)
        .order_by_asc(MenuColumn::OrderNum)
        .all(state.db.conn())
        .await
        .map_err(db_error)?;
    let checked = RoleMenuEntity::find()
        .filter(RoleMenuColumn::RoleId.eq(role_id))
        .all(state.db.conn())
        .await
        .map_err(db_error)?
        .into_iter()
        .map(|row| row.menu_id)
        .collect();
    Ok(Json(RoleMenuTree {
        tree: build_tree(rows.into_iter().map(leaf).collect()),
        checked,
    }))
}

pub fn routes(_state: AppState) -> Routes {
    Routes::new()
        .route("/api/v1/system/menus", get(list_menus), list_menus__perms())
        .route(
            "/api/v1/system/menus/tree",
            get(list_menus_tree),
            list_menus_tree__perms(),
        )
        .route_audited(
            "/api/v1/system/menus",
            post(create_menu),
            create_menu__perms(),
            create_menu__audit(),
        )
        .route_audited(
            "/api/v1/system/menus/{menu_id}",
            put(update_menu),
            update_menu__perms(),
            update_menu__audit(),
        )
        .route(
            "/api/v1/system/menus/{menu_id}",
            get(get_menu),
            get_menu__perms(),
        )
        .route_audited(
            "/api/v1/system/menus/{menu_id}/sort",
            put(sort_menu),
            sort_menu__perms(),
            sort_menu__audit(),
        )
        .route(
            "/api/v1/system/menus/role-tree/{role_id}",
            get(role_menu_tree),
            role_menu_tree__perms(),
        )
        .route_audited(
            "/api/v1/system/menus/{menu_id}",
            delete(delete_menu),
            delete_menu__perms(),
            delete_menu__audit(),
        )
}
