//! 菜单注册端口的落库验证。真实 PostgreSQL，默认 ignore。

use cicada_infra::Db;
use cicada_infra::config::{DbConfig, Secret};
use cicada_ports::{MenuKind, MenuRegistry, MenuRegistryError, MenuSpec};
use cicada_system::DbMenuRegistry;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
};
use sea_orm_migration::MigratorTraitSelf;

const SCHEMA: &str = "sys_menu_registry_test";

fn url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://cicada:2025%40hanmole@localhost:5432/cicada".to_string())
}

async fn connect(schema: Option<&str>) -> DatabaseConnection {
    let mut opt = ConnectOptions::new(url());
    if let Some(s) = schema {
        opt.set_schema_search_path(s.to_string());
    }
    Database::connect(opt).await.unwrap()
}

async fn scalar(conn: &DatabaseConnection, sql: &str) -> Option<i64> {
    let rows = conn
        .query_all_raw(Statement::from_string(DatabaseBackend::Postgres, sql))
        .await
        .unwrap();
    rows.first().and_then(|r| r.try_get_by_index::<i64>(0).ok())
}

fn spec(
    name: &str,
    kind: MenuKind,
    perm: &str,
    path: Option<&str>,
    parent: Option<&str>,
) -> MenuSpec {
    MenuSpec {
        module: "business".to_string(),
        name: name.to_string(),
        kind,
        permission: (!perm.is_empty()).then(|| perm.to_string()),
        path: path.map(str::to_string),
        parent: parent.map(str::to_string),
        order: 1,
    }
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn declared_menus_land_once_and_reuse_parents() {
    let admin = connect(None).await;
    admin
        .execute_unprepared(&format!("DROP SCHEMA IF EXISTS {SCHEMA} CASCADE"))
        .await
        .unwrap();
    admin
        .execute_unprepared(&format!("CREATE SCHEMA {SCHEMA}"))
        .await
        .unwrap();
    let conn = connect(Some(SCHEMA)).await;
    cicada_migrations::Migrator.up(&conn, None).await.unwrap();
    cicada_migrations::seed::seed(&conn).await.unwrap();

    let db = Db::connect(&DbConfig {
        url: Secret::new(url()),
        max_connections: 3,
        min_connections: 0,
        acquire_timeout_secs: 5,
        slow_query_ms: 200,
        schema: SCHEMA.to_string(),
    })
    .await
    .unwrap();
    let registry = DbMenuRegistry::new(db);

    let specs = vec![
        spec(
            "示例管理",
            MenuKind::Menu,
            "business:demo:view",
            Some("/business/demo"),
            Some("系统管理"),
        ),
        spec(
            "示例查询",
            MenuKind::Button,
            "business:demo:list",
            None,
            None,
        ),
        spec(
            "示例新增",
            MenuKind::Button,
            "business:demo:add",
            None,
            None,
        ),
    ];
    assert_eq!(registry.upsert(&specs).await.unwrap(), 3);
    assert_eq!(
        registry.upsert(&specs).await.unwrap(),
        0,
        "同一批声明重复落库不应产生第二份"
    );

    let directory = scalar(
        &conn,
        "SELECT menu_id FROM sys_menu WHERE menu_name = '系统管理'",
    )
    .await
    .expect("种子里的系统管理目录");
    let menu = scalar(
        &conn,
        "SELECT menu_id FROM sys_menu WHERE perms = 'business:demo:view'",
    )
    .await
    .expect("声明的 C 菜单");
    assert_eq!(
        scalar(
            &conn,
            "SELECT parent_id FROM sys_menu WHERE perms = 'business:demo:view'"
        )
        .await,
        Some(directory),
        "C 菜单按父目录名定位"
    );
    for perm in ["business:demo:list", "business:demo:add"] {
        assert_eq!(
            scalar(
                &conn,
                &format!("SELECT parent_id FROM sys_menu WHERE perms = '{perm}'")
            )
            .await,
            Some(menu),
            "按钮未给父级时按 `{perm}` → `business:demo:view` 的约定挂到同模块菜单下"
        );
    }

    let known = registry.known_permissions().await.unwrap();
    assert!(known.contains(&"business:demo:list".to_string()));

    // 父级推断失败要明确报错，而不是悄悄挂到根节点。
    let orphan = vec![spec(
        "孤儿按钮",
        MenuKind::Button,
        "business:none:list",
        None,
        None,
    )];
    assert!(matches!(
        registry.upsert(&orphan).await,
        Err(MenuRegistryError::MissingParent(_))
    ));
    assert_eq!(
        scalar(
            &conn,
            "SELECT count(*) FROM sys_menu WHERE perms = 'business:none:list'"
        )
        .await,
        Some(0)
    );

    admin
        .execute_unprepared(&format!("DROP SCHEMA {SCHEMA} CASCADE"))
        .await
        .unwrap();
}
