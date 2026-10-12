//! 事务性发件箱。
//!
//! `audit` 是审计事件的**生产者**：只往自己拥有的 `outbox_event` 写行，
//! 绝不碰 `system` 的 `sys_oper_log` / `sys_logininfor`。投递由 relay 完成——
//! 本模块提供按序取未投递事件、投递成功后盖章、失败记账的动作，
//! 循环调度由组装根（`cicada::bootstrap`）起一个后台任务来跑。

use async_trait::async_trait;
use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, QueryResult, Statement, Value};

use cicada_infra::Db;
use cicada_infra::metrics::OUTBOX_DELIVERIES;
use cicada_ports::{
    AGGREGATE_AUDIT, ChangeLogEvent, DomainEvent, EVENT_CHANGE_LOG, EVENT_LOGIN_LOG,
    EVENT_OPER_LOG, EventRouter, LoginLogEvent, OperLogEvent,
};

use crate::{AuditSink, ChangeLog, LoginLog, OperLog};

impl From<OperLog> for OperLogEvent {
    fn from(l: OperLog) -> Self {
        Self {
            tenant_id: l.tenant_id,
            title: l.title,
            business_type: l.business_type,
            method: l.method,
            request_method: l.request_method,
            operator: l.operator,
            oper_url: l.oper_url,
            oper_ip: l.oper_ip,
            oper_param: l.oper_param,
            status: l.status,
            error_msg: l.error_msg,
            cost_ms: l.cost_ms,
            user_agent: l.user_agent,
            biz_code: l.biz_code,
            req_body: l.req_body,
        }
    }
}

impl From<LoginLog> for LoginLogEvent {
    fn from(l: LoginLog) -> Self {
        Self {
            tenant_id: l.tenant_id,
            user_name: l.user_name,
            ip: l.ip,
            user_agent: l.user_agent,
            status: l.status,
            message: l.message,
        }
    }
}

/// 发件箱表的读写门面（`outbox_event` 的属主是 `audit`）。
#[derive(Clone)]
pub struct Outbox {
    db: Db,
}

impl Outbox {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// 追加一条待投递事件。
    pub async fn enqueue(
        &self,
        tenant_id: i64,
        aggregate: &str,
        aggregate_id: Option<String>,
        event_type: &str,
        payload: serde_json::Value,
    ) -> Result<(), DbErr> {
        let payload = Value::String(Some(payload.to_string()));
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO outbox_event (tenant_id, aggregate, aggregate_id, event_type, payload) \
             VALUES ($1,$2,$3,$4,$5::jsonb)",
            [
                tenant_id.into(),
                aggregate.to_string().into(),
                aggregate_id.into(),
                event_type.to_string().into(),
                payload,
            ],
        );
        self.db.conn().execute_raw(stmt).await.map(|_| ())
    }

    /// 按 `id` 升序取未投递事件（命中 `(published_at, id)` 索引）。
    ///
    /// 这里**不按租户过滤**，`outbox_event` 也因此不在数据权限的 RLS 白名单里：
    /// relay 是跨租户的系统任务，而 RLS 策略 fail-closed，给它开等于让审计停摆。
    pub async fn fetch_pending(&self, limit: u32) -> Result<Vec<DomainEvent>, DbErr> {
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT id, tenant_id, aggregate, aggregate_id, event_type, payload::text, \
                    created_at::text, attempts \
             FROM outbox_event WHERE published_at IS NULL ORDER BY id LIMIT $1",
            [i64::from(limit).into()],
        );
        let rows = self.db.conn().query_all_raw(stmt).await?;
        rows.iter().map(row_to_event).collect()
    }

    /// 投递成功：盖章 `published_at`。
    pub async fn mark_published(&self, id: i64) -> Result<(), DbErr> {
        self.execute(
            "UPDATE outbox_event SET published_at = now() WHERE id = $1",
            id,
        )
        .await
    }

    /// 投递失败：只累加尝试次数，事件留在发件箱等下一轮。
    pub async fn record_attempt(&self, id: i64) -> Result<(), DbErr> {
        self.execute(
            "UPDATE outbox_event SET attempts = attempts + 1 WHERE id = $1",
            id,
        )
        .await
    }

    /// 放弃投递：毒事件会长期占住扫描窗口（`ORDER BY id LIMIT n`）饿死后面的事件，
    /// 所以到达重试上限后盖章离场，但错误一定打进日志。
    pub async fn abandon(&self, id: i64) -> Result<(), DbErr> {
        self.execute(
            "UPDATE outbox_event SET published_at = now(), attempts = attempts + 1 WHERE id = $1",
            id,
        )
        .await
    }

    /// 待发事件数（监控与测试断言用）。
    pub async fn pending_count(&self) -> Result<i64, DbErr> {
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT count(*) FROM outbox_event WHERE published_at IS NULL",
            [],
        );
        match self.db.conn().query_one_raw(stmt).await? {
            Some(row) => row.try_get_by_index::<i64>(0),
            None => Ok(0),
        }
    }

    /// 排空一轮：取一批待投递事件 → 逐个交给路由 → 成功盖章 / 失败记账。
    ///
    /// 投递语义是**至少一次**：盖章前进程崩溃会导致重投，审计表因此可能出现重复行——
    /// 对日志类数据可接受，反过来丢事件不可接受。
    pub async fn drain_once(
        &self,
        router: &EventRouter,
        batch: u32,
        max_attempts: i32,
    ) -> Result<DrainReport, DbErr> {
        let events = self.fetch_pending(batch).await?;
        let mut report = DrainReport::default();
        for event in events {
            match router.dispatch(&event).await {
                Ok(()) => {
                    self.mark_published(event.id).await?;
                    report.delivered += 1;
                    metrics::counter!(OUTBOX_DELIVERIES, "kind" => "delivered").increment(1);
                }
                Err(e) => {
                    report.failed += 1;
                    self.record_attempt(event.id).await?;
                    if event.attempts >= max_attempts {
                        tracing::error!(
                            event_id = event.id,
                            event_type = %event.event_type,
                            attempts = event.attempts,
                            error = %e,
                            "发件事件超过重试上限，放弃投递"
                        );
                        self.abandon(event.id).await?;
                        report.abandoned += 1;
                        metrics::counter!(OUTBOX_DELIVERIES, "kind" => "abandoned").increment(1);
                    } else {
                        metrics::counter!(OUTBOX_DELIVERIES, "kind" => "failed").increment(1);
                        tracing::warn!(
                            event_id = event.id,
                            event_type = %event.event_type,
                            error = %e,
                            "发件事件投递失败，稍后重试"
                        );
                    }
                }
            }
        }
        Ok(report)
    }

    async fn execute(&self, sql: &'static str, id: i64) -> Result<(), DbErr> {
        let stmt = Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, [id.into()]);
        self.db.conn().execute_raw(stmt).await.map(|_| ())
    }
}

/// 一轮排空的统计。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrainReport {
    pub delivered: usize,
    pub failed: usize,
    pub abandoned: usize,
}

impl From<ChangeLog> for ChangeLogEvent {
    fn from(c: ChangeLog) -> Self {
        Self {
            tenant_id: c.tenant_id,
            table_name: c.table_name,
            record_id: c.record_id,
            action: c.action.code().to_string(),
            changed: c.changed,
            oper_id: c.oper_id,
            trace_id: c.trace_id,
        }
    }
}

/// 写发件箱的审计 sink（`audit` 只写自己的 `outbox_event`）。
///
/// 写失败仍然只 `warn`：审计不得阻断业务请求。
#[derive(Clone)]
pub struct OutboxAuditSink {
    outbox: Outbox,
}

impl OutboxAuditSink {
    pub fn new(db: Db) -> Self {
        Self {
            outbox: Outbox::new(db),
        }
    }

    pub fn from_outbox(outbox: Outbox) -> Self {
        Self { outbox }
    }
}

#[async_trait]
impl AuditSink for OutboxAuditSink {
    async fn record_oper(&self, log: OperLog) {
        let tenant_id = log.tenant_id;
        let aggregate_id = log.operator.clone();
        let payload = match serde_json::to_value(OperLogEvent::from(log)) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "序列化操作日志事件失败");
                return;
            }
        };
        if let Err(e) = self
            .outbox
            .enqueue(
                tenant_id,
                AGGREGATE_AUDIT,
                aggregate_id,
                EVENT_OPER_LOG,
                payload,
            )
            .await
        {
            tracing::warn!(error = %e, "写审计发件箱失败");
        }
    }

    async fn record_login(&self, log: LoginLog) {
        let tenant_id = log.tenant_id;
        let aggregate_id = Some(log.user_name.clone());
        let payload = match serde_json::to_value(LoginLogEvent::from(log)) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "序列化登录日志事件失败");
                return;
            }
        };
        if let Err(e) = self
            .outbox
            .enqueue(
                tenant_id,
                AGGREGATE_AUDIT,
                aggregate_id,
                EVENT_LOGIN_LOG,
                payload,
            )
            .await
        {
            tracing::warn!(error = %e, "写审计发件箱失败");
        }
    }

    async fn record_change(&self, log: ChangeLog) {
        if log.is_empty() {
            return;
        }
        let tenant_id = log.tenant_id;
        let aggregate_id = Some(log.record_id.clone());
        let payload = match serde_json::to_value(ChangeLogEvent::from(log)) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "序列化变更留痕事件失败");
                return;
            }
        };
        if let Err(e) = self
            .outbox
            .enqueue(
                tenant_id,
                AGGREGATE_AUDIT,
                aggregate_id,
                EVENT_CHANGE_LOG,
                payload,
            )
            .await
        {
            tracing::warn!(error = %e, "写审计发件箱失败");
        }
    }
}

fn row_to_event(row: &QueryResult) -> Result<DomainEvent, DbErr> {
    let payload_text: String = row.try_get_by_index(5)?;
    let payload = serde_json::from_str(&payload_text).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "发件箱 payload 不是合法 JSON，按 null 处理");
        serde_json::Value::Null
    });
    Ok(DomainEvent {
        id: row.try_get_by_index(0)?,
        tenant_id: row.try_get_by_index(1)?,
        aggregate: row.try_get_by_index(2)?,
        aggregate_id: row.try_get_by_index(3)?,
        event_type: row.try_get_by_index(4)?,
        payload,
        created_at: row.try_get_by_index(6)?,
        attempts: row.try_get_by_index(7)?,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn oper_log_maps_to_event_contract() {
        let log = OperLog {
            tenant_id: 3,
            title: "system:user:add".into(),
            business_type: 1,
            method: "POST".into(),
            request_method: "/api/v1/users".into(),
            operator: Some("admin".into()),
            oper_url: "/api/v1/users".into(),
            oper_ip: None,
            oper_param: None,
            status: 0,
            error_msg: None,
            cost_ms: 5,

            user_agent: None,
            biz_code: None,
            req_body: None,
        };
        let ev = OperLogEvent::from(log);
        assert_eq!(ev.tenant_id, 3);
        assert_eq!(ev.title, "system:user:add");
        assert_eq!(ev.cost_ms, 5);
        assert_eq!(
            serde_json::to_value(&ev).unwrap()["request_method"],
            json!("/api/v1/users")
        );
    }

    #[test]
    fn login_log_maps_to_event_contract() {
        let ev = LoginLogEvent::from(LoginLog::success(
            1,
            "admin",
            Some("10.0.0.1".into()),
            Some("Chrome/120".into()),
        ));
        assert_eq!(ev.user_name, "admin");
        assert_eq!(ev.status, LoginLog::SUCCESS);
        assert_eq!(ev.message, None);
    }
}
