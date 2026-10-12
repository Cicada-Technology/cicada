//! `#[require_perm]` 元数据 + 装配期双向对账的纯逻辑测试（无需数据库）。

use axum::routing::{get, post};

use cicada_kernel::Permission;
use cicada_web::ModuleName;
use cicada_web::assemble::{AssembleError, reconcile_route_perms};
use cicada_web::routes::{RouteMeta, Routes};

#[cicada_macros::require_perm("system:user:add")]
async fn create_user() -> &'static str {
    "ok"
}

#[cicada_macros::require_perm("system:user:list", "system:user:query")]
async fn list_users() -> &'static str {
    "ok"
}

#[cicada_macros::audit(module = "system", action = "user:add", kind = "insert")]
async fn create_user_audited() -> &'static str {
    "ok"
}

#[test]
fn audit_macro_emits_audit_meta() {
    let meta = create_user_audited__audit();
    assert_eq!(meta.module, "system");
    assert_eq!(meta.action, "user:add");
    assert_eq!(meta.business_type, 1);
    assert_eq!(meta.title(), "system:user:add");
}

#[test]
fn audit_macro_defaults_to_other() {
    #[cicada_macros::audit(module = "system", action = "ping")]
    #[allow(dead_code)]
    async fn ping() -> &'static str {
        "ok"
    }
    assert_eq!(ping__audit().business_type, 0);
}

#[test]
fn route_audited_is_registrable() {
    let routes = Routes::new().route_audited(
        "/users",
        post(create_user_audited),
        &[],
        create_user_audited__audit(),
    );
    assert!(routes.declared_perms().is_empty());
}

#[test]
fn macro_emits_perms_accessor() {
    assert_eq!(create_user__perms(), &["system:user:add"]);
    assert_eq!(
        list_users__perms(),
        &["system:user:list", "system:user:query"]
    );
}

#[test]
fn routes_collect_route_metadata() {
    let routes = Routes::new()
        .route("/users", get(list_users), list_users__perms())
        .route("/users", post(create_user), create_user__perms());
    let perms = routes.declared_perms();
    assert!(perms.contains(&Permission::new("system:user:add")));
    assert!(perms.contains(&Permission::new("system:user:list")));
    assert!(perms.contains(&Permission::new("system:user:query")));
}

fn meta(path: &str, perms: &[&str]) -> RouteMeta {
    RouteMeta {
        path: path.to_string(),
        perms: perms.iter().map(|p| Permission::new(*p)).collect(),
    }
}

#[test]
fn reconciliation_passes_when_balanced() {
    let declared = vec![(ModuleName("m"), Permission::new("system:user:add"))];
    let routes = vec![(ModuleName("m"), meta("/users", &["system:user:add"]))];
    assert!(reconcile_route_perms(&declared, &routes).is_empty());
}

#[test]
fn reconciliation_flags_unseeded_permission() {
    let declared = vec![(ModuleName("m"), Permission::new("system:user:add"))];
    let routes = vec![(ModuleName("m"), meta("/users", &["system:user:export"]))];
    let errors = reconcile_route_perms(&declared, &routes);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AssembleError::UnseededPermission { .. })),
        "路由声明了未注册的权限点应报错：{errors:?}"
    );
}

#[test]
fn reconciliation_flags_orphan_permission() {
    let declared = vec![
        (ModuleName("m"), Permission::new("system:user:add")),
        (ModuleName("m"), Permission::new("system:user:remove")),
    ];
    let routes = vec![(ModuleName("m"), meta("/users", &["system:user:add"]))];
    let errors = reconcile_route_perms(&declared, &routes);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AssembleError::OrphanPermission { .. })),
        "注册了却无人使用的权限点应报错：{errors:?}"
    );
}
