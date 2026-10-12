//! 库表发现测试（需要真实 PostgreSQL，默认 ignore）。
//!
//! 运行：`cargo test -p cicada-codegen --test discovery -- --ignored`

use cicada_codegen::{DiscoveredTable, TplCategory, build_plan, describe_table, list_tables};
use cicada_infra::config::{DbConfig, Secret};
use cicada_infra::{Db, GenConfig};
use sea_orm::ConnectionTrait;

/// 与仓库其它 DB 测试一致：优先 `DATABASE_URL`，否则用本机开发库。
fn url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://cicada:2025%40hanmole@localhost:5432/cicada".to_string())
}

async fn connect() -> Db {
    Db::connect(&DbConfig {
        url: Secret::new(url()),
        max_connections: 5,
        min_connections: 0,
        acquire_timeout_secs: 5,
        slow_query_ms: 200,
        schema: "public".to_string(),
    })
    .await
    .expect("连接数据库")
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL（--ignored 运行）"]
async fn discovers_columns_of_a_seeded_table() {
    let db = connect().await;
    let summaries = list_tables(&db).await.expect("列表");
    assert!(
        summaries.iter().any(|t| t.table_name == "sys_post"),
        "应能看到 sys_post"
    );

    let table = describe_table(&db, "sys_post").await.expect("描述");
    assert_eq!(table.pk_count(), 1);
    let pk = table.primary_key().expect("主键");
    assert_eq!(pk.name, "post_id");
    assert_eq!(pk.udt_name, "int8");
    assert!(table.column("post_code").is_some());
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL（--ignored 运行）"]
async fn end_to_end_plan_of_a_temp_table() {
    let db = connect().await;
    let name = format!("biz_gen_demo_{}", std::process::id());
    db.conn()
        .execute_unprepared(&format!(
            "CREATE TABLE {name} (
                 demo_id bigserial PRIMARY KEY,
                 tenant_id bigint NOT NULL,
                 demo_name varchar(64) NOT NULL,
                 amount numeric(12,2),
                 deleted_at timestamptz,
                 create_time timestamptz NOT NULL DEFAULT now()
             )"
        ))
        .await
        .expect("建表");

    let discovered: DiscoveredTable = describe_table(&db, &name).await.expect("描述");
    assert!(discovered.column("amount").is_some());
    assert!(discovered.column("demo_name").is_some());

    let mut cfg = GenConfig::default();
    cfg.strip_prefixes.push("biz_".to_string());
    let plan = build_plan(&discovered, &cfg, TplCategory::Crud).expect("建计划");
    assert!(
        plan.class_name.starts_with("GenDemo"),
        "class_name={}",
        plan.class_name
    );

    // 框架表被拒。
    let sys_user = describe_table(&db, "sys_user")
        .await
        .expect("sys_user 存在");
    assert!(build_plan(&sys_user, &cfg, TplCategory::Crud).is_err());

    db.conn()
        .execute_unprepared(&format!("DROP TABLE {name}"))
        .await
        .expect("清理表");
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL（--ignored 运行）"]
async fn rejects_illegal_table_names() {
    let db = connect().await;
    assert!(describe_table(&db, "../etc/passwd").await.is_err());
    assert!(describe_table(&db, "no_such_table_here").await.is_err());
}
