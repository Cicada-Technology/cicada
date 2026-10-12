//! 审计日志落库处理器：`sys_oper_log` / `sys_logininfor` / `sys_audit_log`
//! 的属主是 `system`。
//!
//! relay 把 `audit.oper_log` / `audit.login_log` / `audit.change_log` 事件投递到这里，
//! 本模块是唯一写这三张表的代码；`audit` 只负责产生事件。

use async_trait::async_trait;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, Value};

use cicada_infra::Db;
use cicada_kernel::{DataScope, Scope, TenantId};
use cicada_ports::{
    ChangeLogEvent, DomainEvent, EVENT_CHANGE_LOG, EVENT_LOGIN_LOG, EVENT_OPER_LOG, EventError,
    EventHandler, LoginLogEvent, OperLogEvent,
};

/// 把审计事件落成日志表。
#[derive(Clone)]
pub struct SysLogStore {
    db: Db,
}

impl SysLogStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// 在带租户上下文的事务里执行。
    ///
    /// `sys_oper_log` / `sys_logininfor` 在数据权限的 RLS 白名单里，而策略是 fail-closed 的：
    /// 没有 `app.tenant_id` 时连 `INSERT` 都会被 `WITH CHECK` 拒掉（42501）。relay 是系统任务，
    /// 租户不在会话里，只能从事件本身取。
    async fn execute(&self, event: &DomainEvent, stmt: Statement) -> Result<(), EventError> {
        let scope = Scope {
            tenant_id: TenantId::new(event.tenant_id),
            user_id: None,
            data_scope: DataScope::all(),
        };
        let scoped = self
            .db
            .begin_scoped(&scope)
            .await
            .map_err(|e| EventError::Delivery(e.to_string()))?;
        let result = scoped
            .execute_raw(stmt)
            .await
            .map(|_| ())
            .map_err(|e| EventError::Delivery(e.to_string()));
        if result.is_ok() {
            scoped
                .commit()
                .await
                .map_err(|e| EventError::Delivery(e.to_string()))?;
        } else {
            let _ = scoped.rollback().await;
        }
        result
    }
}

#[async_trait]
impl EventHandler for SysLogStore {
    fn event_types(&self) -> Vec<&'static str> {
        vec![EVENT_OPER_LOG, EVENT_LOGIN_LOG, EVENT_CHANGE_LOG]
    }

    async fn handle(&self, event: &DomainEvent) -> Result<(), EventError> {
        match event.event_type.as_str() {
            EVENT_OPER_LOG => self.execute(event, oper_log_statement(event)?.stmt).await,
            EVENT_LOGIN_LOG => self.execute(event, login_log_statement(event)?.stmt).await,
            EVENT_CHANGE_LOG => self.execute(event, change_log_statement(event)?.stmt).await,
            other => Err(EventError::Unsupported(other.to_string())),
        }
    }
}

/// 语句 + 绑定值：绑定值单独留一份，断言与排障都不必依赖 `Statement` 的内部结构。
#[derive(Debug)]
pub struct OperLogStatement {
    pub stmt: Statement,
    pub values: Vec<Value>,
}

/// 同上，登录日志形态。
#[derive(Debug)]
pub struct LoginLogStatement {
    pub stmt: Statement,
    pub values: Vec<Value>,
}

/// `audit.oper_log` → `sys_oper_log` 一行。
///
/// 时间取事件的 `created_at`（发件箱行的产生时刻），而不是投递时刻：
/// relay 的排队延迟不该改写操作发生的时间。
fn oper_log_statement(event: &DomainEvent) -> Result<OperLogStatement, EventError> {
    let log: OperLogEvent = event.payload_as()?;
    let values = vec![
        log.tenant_id.into(),
        log.title.into(),
        log.business_type.into(),
        log.method.into(),
        log.request_method.into(),
        log.operator.into(),
        log.oper_url.into(),
        log.oper_ip.into(),
        log.oper_param.into(),
        log.status.into(),
        log.error_msg.into(),
        text_value(&event.created_at),
        log.cost_ms.into(),
        log.user_agent.into(),
        log.biz_code.into(),
        log.req_body.into(),
    ];
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO sys_oper_log \
         (tenant_id, title, business_type, method, request_method, oper_name, oper_url, \
          oper_ip, oper_param, status, error_msg, oper_time, cost_ms,
          user_agent, biz_code, req_body) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12::timestamptz,$13,$14,$15,$16)",
        values.clone(),
    );
    Ok(OperLogStatement { stmt, values })
}

/// `audit.login_log` → `sys_logininfor` 一行。浏览器/OS 在这里从 UA 解析——
/// 那是本表自己的列，不交给生产者算。
fn login_log_statement(event: &DomainEvent) -> Result<LoginLogStatement, EventError> {
    let log: LoginLogEvent = event.payload_as()?;
    let values = vec![
        log.tenant_id.into(),
        log.user_name.into(),
        log.ip.into(),
        browser_of(&log.user_agent).into(),
        os_of(&log.user_agent).into(),
        log.status.into(),
        log.message.into(),
        text_value(&event.created_at),
    ];
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO sys_logininfor \
         (tenant_id, user_name, ipaddr, browser, os, status, msg, login_time) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8::timestamptz)",
        values.clone(),
    );
    Ok(LoginLogStatement { stmt, values })
}

/// `audit.change_log` → `sys_audit_log` 一行（审计日志表，P1）。
///
/// 分区表按 `oper_time` 路由，时间同样取发件箱行的 `created_at` 而不是投递时刻。
fn change_log_statement(event: &DomainEvent) -> Result<ChangeLogStatement, EventError> {
    let log: ChangeLogEvent = event.payload_as()?;
    let values = vec![
        log.tenant_id.into(),
        log.table_name.into(),
        log.record_id.into(),
        log.action.into(),
        text_value(&log.changed.to_string()),
        log.oper_id.into(),
        text_value(&event.created_at),
        log.trace_id.into(),
    ];
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO sys_audit_log \
            (tenant_id, table_name, record_id, action, changed, oper_id, oper_time, trace_id) \
         VALUES ($1,$2,$3,$4,$5::jsonb,$6,$7::timestamptz,$8)",
        values.clone(),
    );
    Ok(ChangeLogStatement { stmt, values })
}

/// 语句 + 绑定值：断言与排障都不必依赖 `Statement` 的内部结构。
#[derive(Debug)]
pub struct ChangeLogStatement {
    pub stmt: Statement,
    pub values: Vec<Value>,
}

fn text_value(s: &str) -> Value {
    Value::String(Some(s.to_owned()))
}

/// 从 User-Agent 粗取浏览器名（占位实现；严格解析留待 UA 解析库接入）。
fn browser_of(ua: &Option<String>) -> Option<String> {
    let ua = ua.as_deref()?;
    let name = if ua.contains("Edg/") {
        "Edge"
    } else if ua.contains("Chrome/") {
        "Chrome"
    } else if ua.contains("Firefox/") {
        "Firefox"
    } else if ua.contains("Safari/") {
        "Safari"
    } else {
        return None;
    };
    Some(name.to_string())
}

fn os_of(ua: &Option<String>) -> Option<String> {
    let ua = ua.as_deref()?;
    let name = if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Mac OS") {
        "macOS"
    } else if ua.contains("Android") {
        "Android"
    } else if ua.contains("Linux") {
        "Linux"
    } else {
        return None;
    };
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use cicada_ports::AGGREGATE_AUDIT;

    fn event(event_type: &str, payload: serde_json::Value) -> DomainEvent {
        DomainEvent {
            id: 1,
            tenant_id: 1,
            aggregate: AGGREGATE_AUDIT.to_string(),
            aggregate_id: None,
            event_type: event_type.to_string(),
            payload,
            created_at: "2026-10-16 09:00:00.000000+00".to_string(),
            attempts: 0,
        }
    }

    #[test]
    fn oper_log_statement_binds_every_column() {
        let payload = json!({
            "tenant_id": 2,
            "title": "system:user:delete",
            "business_type": 3,
            "method": "DELETE",
            "request_method": "/api/v1/users/9",
            "operator": "admin",
            "oper_url": "/api/v1/users/9",
            "oper_ip": "10.0.0.1",
            "oper_param": null,
            "status": 0,
            "error_msg": null,
            "cost_ms": 7
        });
        let built = oper_log_statement(&event(EVENT_OPER_LOG, payload)).unwrap();
        assert_eq!(built.values.len(), 16);
        assert!(
            built.stmt.sql.contains("sys_oper_log"),
            "SQL 应指向属主表：{}",
            built.stmt.sql
        );
    }

    #[test]
    fn login_log_statement_parses_ua_into_columns() {
        let payload = json!({
            "tenant_id": 1,
            "user_name": "admin",
            "ip": "10.0.0.2",
            "user_agent": "Mozilla/5.0 (Macintosh; Mac OS X) Safari/17",
            "status": 1,
            "message": "验证码错误"
        });
        let built = login_log_statement(&event(EVENT_LOGIN_LOG, payload)).unwrap();
        assert_eq!(built.values.len(), 8);
        assert_eq!(built.values[3], Value::String(Some("Safari".into())));
        assert_eq!(built.values[4], Value::String(Some("macOS".into())));
    }

    #[test]
    fn malformed_payload_fails_before_building_sql() {
        let err =
            oper_log_statement(&event(EVENT_OPER_LOG, json!({"tenant_id": "x"}))).unwrap_err();
        assert!(matches!(err, EventError::Malformed(_)));
    }

    #[test]
    fn change_log_statement_binds_appendix_f_columns() {
        let payload = json!({
            "tenant_id": 1,
            "table_name": "sys_user",
            "record_id": "42",
            "action": "update",
            "changed": {"nick_name": ["旧", "新"]},
            "oper_id": 9,
            "trace_id": "trace-abc"
        });
        let built = change_log_statement(&event(EVENT_CHANGE_LOG, payload)).unwrap();
        assert_eq!(built.values.len(), 8);
        assert!(
            built.stmt.sql.contains("sys_audit_log"),
            "SQL 应指向属主表：{}",
            built.stmt.sql
        );
        assert!(
            built.stmt.sql.contains("$5::jsonb"),
            "changed 必须显式转 jsonb：{}",
            built.stmt.sql
        );
    }

    #[test]
    fn empty_change_payload_still_maps_types() {
        // 生产端已滤掉空差异；这里只确认 `{}` 这种负载仍能正确绑定，不会因缺字段报错。
        let payload = json!({
            "tenant_id": 2,
            "table_name": "sys_dept",
            "record_id": "7",
            "action": "delete",
            "changed": {},
            "oper_id": null,
            "trace_id": null
        });
        let built = change_log_statement(&event(EVENT_CHANGE_LOG, payload)).unwrap();
        assert_eq!(built.values[3], Value::String(Some("delete".to_string())));
    }

    #[test]
    fn ua_parsing_picks_known_browser_and_os() {
        let ua = Some("Mozilla/5.0 (Windows NT 10.0) Chrome/120".to_string());
        assert_eq!(browser_of(&ua).as_deref(), Some("Chrome"));
        assert_eq!(os_of(&ua).as_deref(), Some("Windows"));
        assert_eq!(browser_of(&None), None);
    }
}
