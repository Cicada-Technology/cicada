//! 作用域绑定的执行句柄 `ScopedDb`（datascope-typestate.md ）。
//!
//! 受控查询只接受 `ScopedDb`（或任一 `ConnectionTrait`），而 `ScopedDb` 的构造入口
//! 只有 `Db::begin_scoped`——那里就是 RLS 会话变量的唯一注入点。应用层即使算错归属
//! 条件，数据库层的 RLS 策略仍会兜底。

use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseTransaction, DbErr, ExecResult, QueryResult,
    Statement,
};

use cicada_kernel::Scope;

/// 一个事务 + 它所属的 `Scope`。
pub struct ScopedDb {
    pub(crate) txn: DatabaseTransaction,
    pub(crate) scope: Scope,
}

impl ScopedDb {
    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    pub async fn commit(self) -> Result<(), DbErr> {
        self.txn.commit().await
    }

    pub async fn rollback(self) -> Result<(), DbErr> {
        self.txn.rollback().await
    }
}

#[async_trait::async_trait]
impl ConnectionTrait for ScopedDb {
    fn get_database_backend(&self) -> DatabaseBackend {
        self.txn.get_database_backend()
    }

    async fn execute_raw(&self, stmt: Statement) -> Result<ExecResult, DbErr> {
        self.txn.execute_raw(stmt).await
    }

    async fn execute_unprepared(&self, sql: &str) -> Result<ExecResult, DbErr> {
        self.txn.execute_unprepared(sql).await
    }

    async fn query_one_raw(&self, stmt: Statement) -> Result<Option<QueryResult>, DbErr> {
        self.txn.query_one_raw(stmt).await
    }

    async fn query_all_raw(&self, stmt: Statement) -> Result<Vec<QueryResult>, DbErr> {
        self.txn.query_all_raw(stmt).await
    }
}
