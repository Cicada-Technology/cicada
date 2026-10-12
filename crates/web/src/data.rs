//! 数据监控 HTTP 面：PostgreSQL 数据库与表的只读统计。
//!
//! 这里刻意**不是** Druid 式的内嵌 SQL 控制台：
//! - 不执行任意 SQL，只读系统目录与统计视图；
//! - 不返回连接串、凭据或参数化查询文本；
//! - 只给运维看「库多大、哪些表在长、连接池紧不紧」这三类事实。
//!
//! 对应权限点：菜单 `monitor:data:view` 负责可见性，接口用 F 类
//! `monitor:data:list` 守卫，避免「能看见菜单」被当成「能读库统计」。

use axum::Json;
use axum::extract::State;
use axum::routing::get;
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseBackend, QueryResult, Statement, TryGetable};
use serde::Serialize;
use utoipa::ToSchema;

use cicada_macros::require_perm;

use crate::Permission;
use crate::extractor::RequestContextExt;
use crate::module::{Manifest, Module};
use crate::problem::{ApiErr, BizCode, Problem};
use crate::routes::Routes;
use crate::state::AppState;

/// 观测权限点：读数据库统计。
pub const PERM_LIST: &str = "monitor:data:list";

const PERMS: &[&str] = &[PERM_LIST];

#[derive(Debug, Serialize, ToSchema)]
pub struct DataDatabaseVo {
    pub name: String,
    pub version: String,
    pub schema: String,
    pub size_bytes: i64,
    pub table_count: i64,
    pub connections: i64,
    pub max_connections: i64,
    pub started_at: DateTime<Utc>,
    pub uptime_seconds: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DataPoolVo {
    pub total: u32,
    pub idle: u32,
    pub max: u32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TableStatVo {
    pub schema: String,
    pub table: String,
    pub row_count: i64,
    pub total_bytes: i64,
    pub table_bytes: i64,
    pub index_bytes: i64,
    pub seq_scan: i64,
    pub idx_scan: i64,
    pub dead_rows: i64,
    pub last_vacuum: Option<DateTime<Utc>>,
    pub last_analyze: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DataMonitorVo {
    pub database: DataDatabaseVo,
    pub pool: DataPoolVo,
    /// 按总大小倒序的用户表（不含系统表与分区子表），最多 100 张。
    pub tables: Vec<TableStatVo>,
    pub collected_at: DateTime<Utc>,
}

fn db_error(e: impl std::fmt::Display) -> ApiErr {
    tracing::error!(error = %e, "数据监控查询失败");
    Problem::new(BizCode::Internal, "数据监控查询失败")
}

fn row_get<T: TryGetable>(row: &QueryResult, index: usize, what: &str) -> Result<T, ApiErr> {
    row.try_get_by_index::<T>(index)
        .map_err(|e| db_error(format!("{what}: {e}")))
}

const DATABASE_SQL: &str = r#"
SELECT current_database()::text,
       version()::text,
       COALESCE(current_schema(), 'public')::text,
       pg_database_size(current_database())::bigint,
       (SELECT count(*) FROM pg_class c
          JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE c.relkind = 'r'
           AND NOT c.relispartition
           AND n.nspname = ANY(current_schemas(false)))::bigint,
       (SELECT count(*) FROM pg_stat_activity
         WHERE datname = current_database())::bigint,
       (SELECT setting::bigint FROM pg_settings WHERE name = 'max_connections'),
       pg_postmaster_start_time(),
       EXTRACT(EPOCH FROM (now() - pg_postmaster_start_time()))::bigint
"#;

const TABLES_SQL: &str = r#"
SELECT n.nspname::text,
       c.relname::text,
       COALESCE(s.n_live_tup, 0)::bigint,
       pg_total_relation_size(c.oid)::bigint,
       pg_relation_size(c.oid)::bigint,
       pg_indexes_size(c.oid)::bigint,
       COALESCE(s.seq_scan, 0)::bigint,
       COALESCE(s.idx_scan, 0)::bigint,
       COALESCE(s.n_dead_tup, 0)::bigint,
       s.last_vacuum,
       s.last_analyze
  FROM pg_class c
  JOIN pg_namespace n ON n.oid = c.relnamespace
  LEFT JOIN pg_stat_user_tables s ON s.relid = c.oid
 WHERE c.relkind = 'r'
   AND NOT c.relispartition
   AND n.nspname = ANY(current_schemas(false))
 ORDER BY pg_total_relation_size(c.oid) DESC, n.nspname, c.relname
 LIMIT 100
"#;

#[utoipa::path(
    get,
    path = "/api/v1/monitor/data",
    responses((status = 200, description = "数据库与表统计", body = DataMonitorVo))
)]
#[require_perm("monitor:data:list")]
pub async fn data_monitor(
    State(state): State<AppState>,
    _ctx: RequestContextExt,
) -> Result<Json<DataMonitorVo>, ApiErr> {
    let db = state.db.conn();

    let row = db
        .query_all_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            DATABASE_SQL,
        ))
        .await
        .map_err(db_error)?
        .into_iter()
        .next()
        .ok_or_else(|| db_error("数据库统计没有返回行"))?;

    let database = DataDatabaseVo {
        name: row_get(&row, 0, "database")?,
        version: row_get(&row, 1, "version")?,
        schema: row_get(&row, 2, "schema")?,
        size_bytes: row_get(&row, 3, "size_bytes")?,
        table_count: row_get(&row, 4, "table_count")?,
        connections: row_get(&row, 5, "connections")?,
        max_connections: row_get(&row, 6, "max_connections")?,
        started_at: row_get(&row, 7, "started_at")?,
        uptime_seconds: row_get(&row, 8, "uptime_seconds")?,
    };

    let tables = db
        .query_all_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            TABLES_SQL,
        ))
        .await
        .map_err(db_error)?
        .iter()
        .map(|row| {
            Ok(TableStatVo {
                schema: row_get(row, 0, "schema")?,
                table: row_get(row, 1, "table")?,
                row_count: row_get(row, 2, "row_count")?,
                total_bytes: row_get(row, 3, "total_bytes")?,
                table_bytes: row_get(row, 4, "table_bytes")?,
                index_bytes: row_get(row, 5, "index_bytes")?,
                seq_scan: row_get(row, 6, "seq_scan")?,
                idx_scan: row_get(row, 7, "idx_scan")?,
                dead_rows: row_get(row, 8, "dead_rows")?,
                last_vacuum: row_get(row, 9, "last_vacuum")?,
                last_analyze: row_get(row, 10, "last_analyze")?,
            })
        })
        .collect::<Result<Vec<_>, ApiErr>>()?;

    let max = state.config.db.resolved_max_connections();
    let pool = state
        .db
        .pool_live()
        .map(|p| DataPoolVo {
            total: p.total,
            idle: p.idle,
            max,
        })
        .unwrap_or(DataPoolVo {
            total: 0,
            idle: 0,
            max,
        });

    Ok(Json(DataMonitorVo {
        database,
        pool,
        tables,
        collected_at: Utc::now(),
    }))
}

/// 数据监控模块：权限点与路由声明，装配期双向对账。
pub struct DataModule;

impl Module for DataModule {
    fn manifest(self: Box<Self>) -> Manifest {
        let perms = PERMS.iter().map(|p| Permission::new(*p));
        Manifest::new("cicada:monitor-data")
            .perms(perms)
            .routes(routes)
    }
}

pub fn routes(_state: AppState) -> Routes {
    Routes::new().route(
        "/api/v1/monitor/data",
        get(data_monitor),
        data_monitor__perms(),
    )
}
