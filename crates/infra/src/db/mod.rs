//! SeaORM 之上的 `Db`：连接池句柄（克隆成本极低）+ 事务。

use std::time::Duration;

use log::LevelFilter;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection,
    DatabaseTransaction, DbErr, Statement, TransactionTrait,
};

use crate::config::DbConfig;
use cicada_kernel::Scope;

pub mod scoped;

pub use scoped::ScopedDb;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("连接数据库失败：{0}")]
    Connect(#[from] DbErr),
    #[error("数据库操作失败：{0}")]
    Operation(#[source] DbErr),
}

/// 深模块：连接 / 事务 / 连接池全部藏在一个句柄后。
#[derive(Clone)]
pub struct Db {
    conn: DatabaseConnection,
}

/// 连接池的实时状态（`db_pool_connections`）。
///
/// `total` 是池内已建立的连接数，`idle` 是其中空闲的；上限来自 `db.max_connections`，
/// sqlx 不把它回读到池对象上，因此由调用方从配置给出。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PoolLive {
    pub total: u32,
    pub idle: u32,
}

impl Db {
    pub async fn connect(cfg: &DbConfig) -> Result<Self, DbError> {
        let mut opt = ConnectOptions::new(cfg.url.expose_secret().to_owned());
        opt.max_connections(cfg.resolved_max_connections())
            .min_connections(cfg.min_connections)
            .acquire_timeout(Duration::from_secs(cfg.acquire_timeout_secs))
            // 数据层约定「sqlx_logging(false)：由 tracing 统一接管」。这里改成「开但级别为 Off」
            // 是为了数据层约定的另一句——慢查询阈值可配：sqlx 只在 `sqlx_logging` 为真时才
            // 应用慢语句设置，而级别 Off 保证**常规语句一个字都不打**。
            // 语句在 span 里（可观测性），慢语句额外有一条 warn 日志可查。
            .sqlx_logging(true)
            .sqlx_logging_level(LevelFilter::Off)
            .record_stmt_in_spans(true)
            .set_schema_search_path(cfg.schema.clone());
        if cfg.slow_query_ms > 0 {
            opt.sqlx_slow_statements_logging_settings(
                LevelFilter::Warn,
                Duration::from_millis(cfg.slow_query_ms),
            );
        }
        let conn = Database::connect(opt).await?;
        Ok(Self { conn })
    }

    pub fn conn(&self) -> &DatabaseConnection {
        &self.conn
    }

    /// 读取 sqlx PostgreSQL 池的即时状态。非 Postgres 后端（测试替身等）返回 `None`。
    pub fn pool_live(&self) -> Option<PoolLive> {
        let pool = self.conn.get_postgres_connection_pool();
        Some(PoolLive {
            total: pool.size(),
            idle: u32::try_from(pool.num_idle()).unwrap_or(u32::MAX),
        })
    }

    /// 关库：等待池内连接归还（关库前 `close().await`）。
    ///
    /// 优雅停机的最后一步，不是可选项——不给池关闭的机会，退出时还挂着的连接
    /// 由 PG 侧单方面断掉，日志里会出现一段和本进程无关的断连告警。
    pub async fn close(&self) -> Result<(), DbError> {
        self.conn.close_by_ref().await.map_err(DbError::Operation)
    }

    pub async fn ping(&self) -> Result<(), DbError> {
        self.conn.ping().await.map_err(DbError::Operation)
    }

    /// 事务边界在 service 层开启；`DatabaseTransaction` 自身实现 `ConnectionTrait`，
    /// 因此 service 方法写成 `impl ConnectionTrait` 就能「既在事务里、也在事务外」跑。
    pub async fn begin(&self) -> Result<DatabaseTransaction, DbError> {
        self.conn.begin().await.map_err(DbError::Operation)
    }

    /// 作用域绑定的执行句柄：RLS 会话变量的唯一注入点（datascope-typestate.md ）。
    ///
    /// 在事务起点用 `SET LOCAL`（随事务回滚自动消失，不污染连接池）写入
    /// `app.tenant_id` / `app.current_user_id` / `app.dept_ids` / `app.data_scope`。
    pub async fn begin_scoped(&self, scope: &Scope) -> Result<ScopedDb, DbError> {
        let txn = self.conn.begin().await.map_err(DbError::Operation)?;
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT set_config('app.tenant_id', $1, true), \
                    set_config('app.current_user_id', $2, true), \
                    set_config('app.dept_ids', $3, true), \
                    set_config('app.data_scope', $4, true)",
            [
                scope.tenant_id.get().to_string().into(),
                scope
                    .user_id
                    .map(|u| u.get())
                    .unwrap_or(0)
                    .to_string()
                    .into(),
                scope
                    .data_scope
                    .dept_ids
                    .iter()
                    .map(|d| d.get().to_string())
                    .collect::<Vec<_>>()
                    .join(",")
                    .into(),
                scope.data_scope.kind.as_str().into(),
            ],
        );
        txn.execute_raw(stmt).await.map_err(DbError::Operation)?;
        Ok(ScopedDb {
            txn,
            scope: scope.clone(),
        })
    }
}
