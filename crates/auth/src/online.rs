//! 在线会话快照：权威在 Redis；`sys_user_online` 只是异步
//! write-behind 快照，可重建、不参与踢出/上限判定。
//!
//! 为什么要有这张表：会话生命周期（登录 / 登出 / 强退 / 改密失效）发生在 Redis 里，
//! 进程一重启就查无对证。「某人何时从哪个 IP 登录、会话到什么时候结束」只能靠
//! `sys_logininfor` 拼出一半——登录有记录、退出没记录。快照补上后半。
//!
//! 三条纪律：
//! 1. **只由会话生命周期驱动**：`SnapshottedSessionStore` 装饰真正的 `SessionStore`，
//!    Redis 操作成功之后才写快照；判定（存在性、最大会话数、踢出）一律只读 Redis。
//! 2. **写失败不影响登录**：快照写是 fire-and-forget，失败降级为 `warn`
//!    （与审计设计「审计写入不阻塞主流程」同一条理由）。
//! 3. **可重建**：`reconcile` 以 Redis 为准对账——Redis 里已不存在的「在线」行改成已退出，
//!    Redis 里存在的会话 upsert 回来。历史（已退出）行不删，那正是这张表的用途。

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use sea_orm::entity::prelude::*;
use sea_orm::{DatabaseBackend, Statement, Value};

use cicada_infra::Db;
use cicada_macros::DataScoped;

use crate::error::AuthError;
use crate::session::{Session, SessionStore};

/// `sys_user_online` 的实体。主键是 `jti`（access token 的会话标识），
/// 所以 write-behind 天然幂等：同一会话重复投递只会 upsert。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior, DataScoped)]
#[sea_orm(table_name = "sys_user_online")]
#[data_scope(tenant = Column::TenantId, no_ownership)]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub session_id: String,
    pub tenant_id: i64,
    pub user_id: i64,
    pub user_name: String,
    pub ip: String,
    pub user_agent: String,
    pub login_time: DateTimeUtc,
    pub last_access_time: DateTimeUtc,
    /// 0 在线 / 1 已退出。
    pub status: i16,
    pub end_time: Option<DateTimeUtc>,
    pub create_time: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

/// Unix 秒 → `timestamptz`；越界或 0（老会话没有 last_seen）按「现在」算。
fn epoch(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, 0).single().unwrap_or_else(Utc::now)
}

/// upsert 的绑定值：单独成一个纯函数，测试断言值时不必钻进 `Statement` 的内部结构
/// （`stmt.values` 是 `Option<Values>`，取出来比传一份 `Vec` 更绕）。
pub fn upsert_values(jti: &str, session: &Session) -> Vec<Value> {
    vec![
        jti.to_string().into(),
        session.tenant_id.into(),
        session.user_id.into(),
        session.username.clone().into(),
        session.ip.clone().into(),
        session.user_agent.clone().into(),
        epoch(session.login_time).into(),
        epoch(session.last_seen.max(session.login_time)).into(),
    ]
}

/// 一条语句写死在这里：登录与刷新都走它，`end_time` 清空表示「又上线了」。
pub const UPSERT_SQL: &str = "INSERT INTO sys_user_online \
         (session_id, tenant_id, user_id, user_name, ip, user_agent, login_time, \
          last_access_time, status, end_time) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 0, NULL) \
         ON CONFLICT (session_id) DO UPDATE SET \
            tenant_id = EXCLUDED.tenant_id, user_id = EXCLUDED.user_id, \
            user_name = EXCLUDED.user_name, ip = EXCLUDED.ip, \
            user_agent = EXCLUDED.user_agent, login_time = EXCLUDED.login_time, \
            last_access_time = EXCLUDED.last_access_time, status = 0, end_time = NULL";

/// upsert 一条在线会话。主键是 `jti`，所以重复投递天然幂等。
pub fn upsert_statement(jti: &str, session: &Session) -> Statement {
    Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        UPSERT_SQL,
        upsert_values(jti, session),
    )
}

/// 标记单个会话已退出（登出 / 强退）。`end_time IS NULL` 让重复投递只生效一次。
pub fn close_statement(jti: &str) -> Statement {
    Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE sys_user_online SET status = 1, end_time = now() \
         WHERE session_id = $1 AND end_time IS NULL",
        [jti.to_string().into()],
    )
}

/// 标记某用户全部会话已退出（改密即失效、重放保护）。
pub fn close_user_statement(user_id: i64) -> Statement {
    Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE sys_user_online SET status = 1, end_time = now() \
         WHERE user_id = $1 AND end_time IS NULL",
        [user_id.into()],
    )
}

/// 一轮对账的产出。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// 刷回在线的行数（新写入 + 已是在线的）。
    pub refreshed: usize,
    /// Redis 里已不存在、于是被标记退出的行数。
    pub closed: usize,
}

/// 快照写入器：`sys_user_online` 的唯一写入者。
#[derive(Clone)]
pub struct OnlineSnapshot {
    db: Db,
}

impl OnlineSnapshot {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    async fn exec(&self, stmt: Statement) -> Result<u64, DbErr> {
        self.db
            .conn()
            .execute_raw(stmt)
            .await
            .map(|r| r.rows_affected())
    }

    pub async fn record_open(&self, jti: &str, session: &Session) -> Result<(), DbErr> {
        self.exec(upsert_statement(jti, session)).await?;
        Ok(())
    }

    pub async fn record_close(&self, jti: &str) -> Result<(), DbErr> {
        self.exec(close_statement(jti)).await?;
        Ok(())
    }

    pub async fn record_close_user(&self, user_id: i64) -> Result<(), DbErr> {
        self.exec(close_user_statement(user_id)).await?;
        Ok(())
    }

    /// 以 Redis 为准对账（可随时从 Redis 重建）。
    ///
    /// 入参是权威源的在线会话集合，逐条 upsert；库里标着在线但 Redis 已经没有的，
    /// 按退出处理（进程被杀、TTL 自然过期这些路径不会经过 `revoke`，只能靠对账收口）。
    pub async fn reconcile(&self, open: &[(String, Session)]) -> Result<ReconcileReport, DbErr> {
        let mut report = ReconcileReport::default();
        let alive: std::collections::HashSet<&str> =
            open.iter().map(|(jti, _)| jti.as_str()).collect();
        for (jti, session) in open {
            self.exec(upsert_statement(jti, session)).await?;
            report.refreshed += 1;
        }
        let rows = self
            .db
            .conn()
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT session_id FROM sys_user_online WHERE end_time IS NULL",
            ))
            .await?;
        for row in rows {
            let jti: String = row.try_get_by_index(0)?;
            if !alive.contains(jti.as_str()) {
                self.exec(close_statement(&jti)).await?;
                report.closed += 1;
            }
        }
        Ok(report)
    }

    /// 以某个 `SessionStore`（权威源）为准对账快照——组装根启动时调一次。
    ///
    /// 单独收在这里是因为 `reconcile` 的输入只能来自权威源：让调用方传 `list_all()` 的结果
    /// 等于把「谁是权威」这件事说漏了嘴。
    pub async fn reconcile_with(&self, store: &dyn SessionStore) -> Result<ReconcileReport, DbErr> {
        let open = store
            .list_all()
            .await
            .map_err(|e| DbErr::Exec(sea_orm::RuntimeErr::Internal(e.to_string())))?;
        self.reconcile(&open).await
    }

    /// 库里仍标着在线的会话 id（对账与排障用）。
    pub async fn open_session_ids(&self) -> Result<Vec<String>, DbErr> {
        let rows = self
            .db
            .conn()
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT session_id FROM sys_user_online WHERE end_time IS NULL ORDER BY session_id",
            ))
            .await?;
        rows.iter()
            .map(|r| r.try_get_by_index::<String>(0))
            .collect()
    }
}

/// 给 `SessionStore` 加上 write-behind 快照的装饰器（Redis 仍是唯一权威源）。
///
/// 快照写在 `tokio::spawn` 里：登录路径不等它，它失败也只留 `warn`。
pub struct SnapshottedSessionStore {
    inner: Arc<dyn SessionStore>,
    snapshot: OnlineSnapshot,
}

impl SnapshottedSessionStore {
    pub fn new(inner: Arc<dyn SessionStore>, db: Db) -> Self {
        Self {
            inner,
            snapshot: OnlineSnapshot::new(db),
        }
    }

    pub fn snapshot(&self) -> &OnlineSnapshot {
        &self.snapshot
    }

    /// 把写任务丢后台：`Db` 是 `Clone`（内部是连接池句柄），任务里只碰快照。
    fn fire(&self, task: impl std::future::Future<Output = Result<(), DbErr>> + Send + 'static) {
        tokio::spawn(async move {
            if let Err(e) = task.await {
                tracing::warn!(error = %e, "在线会话快照写入失败（不影响会话本身）");
            }
        });
    }
}

#[async_trait]
impl SessionStore for SnapshottedSessionStore {
    async fn create(&self, jti: &str, session: &Session, ttl_secs: u64) -> Result<(), AuthError> {
        self.inner.create(jti, session, ttl_secs).await?;
        let snapshot = self.snapshot.clone();
        let jti = jti.to_string();
        let session = session.clone();
        self.fire(async move { snapshot.record_open(&jti, &session).await });
        Ok(())
    }

    async fn get(&self, jti: &str) -> Result<Option<Session>, AuthError> {
        self.inner.get(jti).await
    }

    async fn touch(&self, jti: &str, ttl_secs: u64) -> Result<(), AuthError> {
        self.inner.touch(jti, ttl_secs).await
    }

    async fn revoke(&self, jti: &str) -> Result<(), AuthError> {
        self.inner.revoke(jti).await?;
        let snapshot = self.snapshot.clone();
        let jti = jti.to_string();
        self.fire(async move { snapshot.record_close(&jti).await });
        Ok(())
    }

    async fn revoke_user(&self, user_id: i64) -> Result<(), AuthError> {
        self.inner.revoke_user(user_id).await?;
        let snapshot = self.snapshot.clone();
        self.fire(async move { snapshot.record_close_user(user_id).await });
        Ok(())
    }

    async fn list_user(&self, user_id: i64) -> Result<Vec<(String, Session)>, AuthError> {
        self.inner.list_user(user_id).await
    }

    async fn list_all(&self) -> Result<Vec<(String, Session)>, AuthError> {
        self.inner.list_all().await
    }

    async fn ping(&self) -> Result<(), AuthError> {
        self.inner.ping().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session {
            user_id: 7,
            tenant_id: 2,
            username: "alice".to_string(),
            refresh_hash: "hash".to_string(),
            ip: "10.0.0.9".to_string(),
            user_agent: "Firefox".to_string(),
            login_time: 1_700_000_000,
            last_seen: 0,
        }
    }

    #[test]
    fn upsert_carries_identity_and_both_timestamps() {
        assert!(
            UPSERT_SQL.contains("ON CONFLICT (session_id) DO UPDATE"),
            "重复投递必须幂等：{UPSERT_SQL}"
        );
        assert!(UPSERT_SQL.contains("status = 0, end_time = NULL"));
        assert!(
            upsert_statement("jti-1", &session())
                .sql
                .as_str()
                .eq(UPSERT_SQL)
        );

        let values = upsert_values("jti-1", &session());
        assert_eq!(values.len(), 8);
        assert_eq!(values[0], Value::String(Some("jti-1".to_string())));
        assert_eq!(values[1], Value::BigInt(Some(2)));
        assert_eq!(values[2], Value::BigInt(Some(7)));
        assert_eq!(values[3], Value::String(Some("alice".to_string())));
        // last_seen=0（老会话没写过活跃时间）不能落成 1970：回落到 login_time。
        let login = Value::ChronoDateTimeUtc(Some(Utc.timestamp_opt(1_700_000_000, 0).unwrap()));
        assert_eq!(values[6], login);
        assert_eq!(values[7], login);
    }

    #[test]
    fn close_only_touches_still_open_rows() {
        for stmt in [close_statement("jti-1"), close_user_statement(7)] {
            let sql = stmt.sql.as_str();
            assert!(sql.contains("end_time IS NULL"), "{sql}");
            assert!(sql.contains("SET status = 1, end_time = now()"), "{sql}");
        }
    }
}
