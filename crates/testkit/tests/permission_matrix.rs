//! 测试分层「P0 权限矩阵全分支」：表驱动，**5 种数据范围 × 3 类实体**的结果集断言
//! （另含内部 `None` 兜底共 6 种）。
//!
//! 三类实体对应四种归属形态里的三种（`NoOwnership` 见 kernel 单测）：
//! - `DeptOnly`：`sys_user`（按 `dept_id` 过滤）；
//! - `DeptAndOwner`：`fixtures::Entity`（`sample_doc`）；
//! - `OwnerOnly`：本文件内定义的 `sample_task::Entity`。
//!
//! 断言的是**返回的 id 集合**（真实 PostgreSQL 执行），不是 SQL 字符串。
//! 期望值取自 `docs/superpowers/design-it-twice/datascope-typestate.md` 的判定矩阵。

use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, DbBackend,
    Statement, Value,
};
use sea_orm_migration::MigratorTraitSelf;

use cicada_infra::Db;
use cicada_infra::config::{DbConfig, Secret};
use cicada_kernel::{DataScope, DataScopeKind, DeptId, Query, Scope, TenantId, UserId};
use cicada_rbac::compute_data_scope;
use cicada_system::entity::{UserColumn, UserEntity};
use cicada_system::{LocalDeptDirectory, load_grant_snapshot};
use cicada_testkit::fixtures;

/// `OwnerOnly` 形态夹具：只有归属人列，没有部门列（就地定义，避免改动 `testkit`
/// 既有的编译失败快照）。
mod sample_task {
    use cicada_macros::DataScoped;
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior, DataScoped)]
    #[sea_orm(table_name = "sample_task")]
    #[data_scope(tenant = Column::TenantId, owner = Column::OwnerId, deleted = Column::DeletedAt)]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i64,
        pub tenant_id: i64,
        pub owner_id: i64,
        pub deleted_at: Option<i64>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
}

const SCHEMA: &str = "perm_matrix_test";

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

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap();
}

/// 直接构造 `Scope`（不经过 `RequestContext`），覆盖判定矩阵的每一格。
fn scope(kind: DataScopeKind, user: i64, depts: &[i64]) -> Scope {
    Scope {
        tenant_id: TenantId::new(1),
        user_id: Some(UserId::new(user)),
        data_scope: DataScope {
            kind,
            dept_ids: depts.iter().copied().map(DeptId::new).collect(),
            subtree_paths: Vec::new(),
            user_id: UserId::new(user),
        },
    }
}

async fn user_ids(db: &DatabaseConnection, sc: &Scope) -> Vec<i64> {
    Query::<UserEntity>::open()
        .scoped(sc)
        .order_by(UserColumn::UserId)
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|u| u.user_id)
        .collect()
}

async fn doc_ids(db: &DatabaseConnection, sc: &Scope) -> Vec<i64> {
    Query::<fixtures::Entity>::open()
        .scoped(sc)
        .order_by(fixtures::Column::Id)
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|d| d.id)
        .collect()
}

async fn task_ids(db: &DatabaseConnection, sc: &Scope) -> Vec<i64> {
    Query::<sample_task::Entity>::open()
        .scoped(sc)
        .order_by(sample_task::Column::Id)
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect()
}

async fn seed_fixture_tables(db: &DatabaseConnection) {
    // 夹具表（仅测试用，走原始 DDL；与 `fixtures` 的实体定义一一对应）。
    for ddl in [
        "CREATE TABLE sample_doc (id bigint PRIMARY KEY, tenant_id bigint NOT NULL, dept_id bigint NOT NULL, owner_id bigint NOT NULL, deleted_at bigint NULL)",
        "CREATE TABLE sample_task (id bigint PRIMARY KEY, tenant_id bigint NOT NULL, owner_id bigint NOT NULL, deleted_at bigint NULL)",
    ] {
        exec(db, ddl, Vec::new()).await;
    }

    let now: Value = chrono::Utc::now().into();

    // 部门树：100(根) -> 110,120；110 -> 111。
    for sql in [
        "INSERT INTO sys_dept (dept_id, tenant_id, parent_id, ancestors, dept_name, order_num, status, version, create_time) VALUES (100,1,0,'0','总公司',1,0,0,$1)",
        "INSERT INTO sys_dept (dept_id, tenant_id, parent_id, ancestors, dept_name, order_num, status, version, create_time) VALUES (110,1,100,'0,100','研发部',1,0,0,$1)",
        "INSERT INTO sys_dept (dept_id, tenant_id, parent_id, ancestors, dept_name, order_num, status, version, create_time) VALUES (111,1,110,'0,100,110','前端组',1,0,0,$1)",
        "INSERT INTO sys_dept (dept_id, tenant_id, parent_id, ancestors, dept_name, order_num, status, version, create_time) VALUES (120,1,100,'0,100','销售部',1,0,0,$1)",
    ] {
        exec(db, sql, vec![now.clone()]).await;
    }

    // 用户（DeptOnly）：1=110, 2=111, 3=120, 4=无部门, 5=租户 2, 6=已软删。
    for sql in [
        "INSERT INTO sys_user (user_id, tenant_id, user_name, nick_name, dept_id, password, status, version, create_time) VALUES (1,1,'alice','Alice',110,'x',0,0,$1)",
        "INSERT INTO sys_user (user_id, tenant_id, user_name, nick_name, dept_id, password, status, version, create_time) VALUES (2,1,'bob','Bob',111,'x',0,0,$1)",
        "INSERT INTO sys_user (user_id, tenant_id, user_name, nick_name, dept_id, password, status, version, create_time) VALUES (3,1,'carol','Carol',120,'x',0,0,$1)",
        "INSERT INTO sys_user (user_id, tenant_id, user_name, nick_name, dept_id, password, status, version, create_time) VALUES (4,1,'dave','Dave',NULL,'x',0,0,$1)",
        "INSERT INTO sys_user (user_id, tenant_id, user_name, nick_name, dept_id, password, status, version, create_time) VALUES (5,2,'erin','Erin',110,'x',0,0,$1)",
        "INSERT INTO sys_user (user_id, tenant_id, user_name, nick_name, dept_id, password, status, version, create_time, deleted_at) VALUES (6,1,'frank','Frank',110,'x',0,0,$1,$1)",
    ] {
        exec(db, sql, vec![now.clone()]).await;
    }

    // 样例文档（DeptAndOwner）：5 为租户 2、6 为已软删。
    for sql in [
        "INSERT INTO sample_doc (id, tenant_id, dept_id, owner_id, deleted_at) VALUES (1,1,110,1,NULL)",
        "INSERT INTO sample_doc (id, tenant_id, dept_id, owner_id, deleted_at) VALUES (2,1,111,2,NULL)",
        "INSERT INTO sample_doc (id, tenant_id, dept_id, owner_id, deleted_at) VALUES (3,1,120,3,NULL)",
        "INSERT INTO sample_doc (id, tenant_id, dept_id, owner_id, deleted_at) VALUES (4,2,110,1,NULL)",
        "INSERT INTO sample_doc (id, tenant_id, dept_id, owner_id, deleted_at) VALUES (5,1,110,1,123)",
    ] {
        exec(db, sql, Vec::new()).await;
    }

    // 样例任务（OwnerOnly）。
    for sql in [
        "INSERT INTO sample_task (id, tenant_id, owner_id, deleted_at) VALUES (1,1,1,NULL)",
        "INSERT INTO sample_task (id, tenant_id, owner_id, deleted_at) VALUES (2,1,2,NULL)",
        "INSERT INTO sample_task (id, tenant_id, owner_id, deleted_at) VALUES (3,1,3,NULL)",
        "INSERT INTO sample_task (id, tenant_id, owner_id, deleted_at) VALUES (4,2,1,NULL)",
        "INSERT INTO sample_task (id, tenant_id, owner_id, deleted_at) VALUES (5,1,1,123)",
    ] {
        exec(db, sql, Vec::new()).await;
    }
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn permission_matrix_covers_all_branches() {
    let admin = connect(None).await;
    admin
        .execute_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            format!("DROP SCHEMA IF EXISTS {SCHEMA} CASCADE"),
        ))
        .await
        .unwrap();
    admin
        .execute_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            format!("CREATE SCHEMA {SCHEMA}"),
        ))
        .await
        .unwrap();

    let db = connect(Some(SCHEMA)).await;
    cicada_migrations::Migrator.up(&db, None).await.unwrap();
    seed_fixture_tables(&db).await;

    // (范围标签, Scope, 期望 sys_user ids, 期望 sample_doc ids, 期望 sample_task ids)
    type Case = (&'static str, Scope, Vec<i64>, Vec<i64>, Vec<i64>);
    let cases: Vec<Case> = vec![
        (
            "All",
            scope(DataScopeKind::All, 1, &[]),
            vec![1, 2, 3, 4],
            vec![1, 2, 3],
            vec![1, 2, 3],
        ),
        (
            "Custom[120]",
            scope(DataScopeKind::Custom, 1, &[120]),
            vec![3],
            vec![3],
            vec![],
        ),
        (
            "Dept[110]",
            scope(DataScopeKind::Dept, 1, &[110]),
            vec![1],
            vec![1],
            vec![],
        ),
        (
            "DeptAndChild[110,111]",
            scope(DataScopeKind::DeptAndChild, 1, &[110, 111]),
            vec![1, 2],
            vec![1, 2],
            vec![],
        ),
        (
            "SelfOnly(me=1)",
            scope(DataScopeKind::SelfOnly, 1, &[]),
            vec![],
            vec![1],
            vec![1],
        ),
        (
            "None",
            scope(DataScopeKind::None, 1, &[]),
            vec![],
            vec![],
            vec![],
        ),
    ];

    for (label, sc, want_users, want_docs, want_tasks) in cases {
        assert_eq!(
            user_ids(&db, &sc).await,
            want_users,
            "DeptOnly(sys_user) 在数据范围 {label}"
        );
        assert_eq!(
            doc_ids(&db, &sc).await,
            want_docs,
            "DeptAndOwner(sample_doc) 在数据范围 {label}"
        );
        assert_eq!(
            task_ids(&db, &sc).await,
            want_tasks,
            "OwnerOnly(sample_task) 在数据范围 {label}"
        );
    }

    // ============================================================================
    // 第二段：`scope × 实体 × 角色` —— 范围不再手写，而是由**角色配置**推出
    // （"seeded scope-driven" -> `load_grant_snapshot` + `compute_data_scope`）。
    // ============================================================================
    seed_matrix_roles(&db).await;

    let cfg = DbConfig {
        url: Secret::new(url()),
        max_connections: 5,
        min_connections: 0,
        acquire_timeout_secs: 5,
        slow_query_ms: 200,
        schema: SCHEMA.to_string(),
    };
    let dept_dir = LocalDeptDirectory::new(Db::connect(&cfg).await.unwrap());

    // (角色用户 id, 说明, 期望 sys_user ids, 期望 sample_doc ids, 期望 sample_task ids)
    type RoleCase = (&'static str, i64, Vec<i64>, Vec<i64>, Vec<i64>);
    let role_cases: Vec<RoleCase> = vec![
        (
            "role=All(user101)",
            101,
            vec![1, 2, 3, 4, 101, 102, 103, 104],
            vec![1, 2, 3],
            vec![1, 2, 3],
        ),
        ("role=Custom[120](user102)", 102, vec![3], vec![3], vec![]),
        (
            "role=Dept[110](user103)",
            103,
            vec![1, 103, 104],
            vec![1],
            vec![],
        ),
        (
            "role=DeptAndChild[110,111](user104)",
            104,
            vec![1, 2, 103, 104],
            vec![1, 2],
            vec![],
        ),
        ("role=SelfOnly(user1)", 1, vec![], vec![1], vec![1]),
    ];

    for (label, user, want_users, want_docs, want_tasks) in role_cases {
        let snapshot = load_grant_snapshot(&db, TenantId::new(1), UserId::new(user)).await;
        let data_scope = compute_data_scope(
            TenantId::new(1),
            UserId::new(user),
            snapshot.user_dept,
            &snapshot.roles,
            &dept_dir,
        )
        .await;
        let sc = Scope {
            tenant_id: TenantId::new(1),
            user_id: Some(UserId::new(user)),
            data_scope,
        };
        assert_eq!(
            user_ids(&db, &sc).await,
            want_users,
            "DeptOnly(sys_user) 在 {label}"
        );
        assert_eq!(
            doc_ids(&db, &sc).await,
            want_docs,
            "DeptAndOwner(sample_doc) 在 {label}"
        );
        assert_eq!(
            task_ids(&db, &sc).await,
            want_tasks,
            "OwnerOnly(sample_task) 在 {label}"
        );
    }

    admin
        .execute_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            format!("DROP SCHEMA {SCHEMA} CASCADE"),
        ))
        .await
        .unwrap();
}

/// 为 5 种数据范围各建一个角色并绑定到专门的用户上（覆盖「范围 × 角色」维度）。
async fn seed_matrix_roles(db: &DatabaseConnection) {
    let now: Value = chrono::Utc::now().into();

    // 角色：20=All, 21=Custom, 22=Dept, 23=DeptAndChild, 24=SelfOnly。
    for (id, key, scope) in [
        (20i64, "m_all", 1i16),
        (21, "m_custom", 2),
        (22, "m_dept", 3),
        (23, "m_child", 4),
        (24, "m_self", 5),
    ] {
        exec(
            db,
            "INSERT INTO sys_role (role_id, tenant_id, role_name, role_key, role_sort, data_scope, status, version, create_time) VALUES ($1,1,$2,$3,$1,$4,0,0,$5)",
            vec![id.into(), key.into(), format!("role_{key}").into(), scope.into(), now.clone()],
        )
        .await;
    }

    // 角色用户：101(All, 无部门), 102(Custom, 无部门), 103(Dept, 110), 104(DeptAndChild, 110)。
    for (id, dept) in [
        (101i64, None::<i64>),
        (102, None),
        (103, Some(110)),
        (104, Some(110)),
    ] {
        exec(
            db,
            "INSERT INTO sys_user (user_id, tenant_id, user_name, nick_name, dept_id, password, status, version, create_time) VALUES ($1,1,$2,$2,$3,'x',0,0,$4)",
            vec![
                id.into(),
                format!("role{id}").into(),
                dept.into(),
                now.clone(),
            ],
        )
        .await;
    }

    for (user, role) in [(101i64, 20i64), (102, 21), (103, 22), (104, 23), (1, 24)] {
        exec(
            db,
            "INSERT INTO sys_user_role (user_id, role_id) VALUES ($1,$2)",
            vec![user.into(), role.into()],
        )
        .await;
    }

    // Custom 角色的自定义部门集合。
    exec(
        db,
        "INSERT INTO sys_role_dept (role_id, dept_id) VALUES (21,120)",
        Vec::new(),
    )
    .await;
}
