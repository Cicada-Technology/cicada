//! 领域事件与投递端口。
//!
//! 「写完自己的数据还要影响别人」的副作用一律先进 `outbox_event`，再由 relay 投递：
//! 生产者（`audit`）只负责把事件写进自己的发件箱，消费者实现本模块的 [`EventHandler`]
//! 落地到**自己拥有**的表。事件负载类型在这里定义，生产端与消费端共用同一份字段契约
//! （依赖规则规则 2/3：L3 与 L2 不得互相依赖，协作只走 `ports`）。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// `outbox_event.aggregate`：审计事件源。
pub const AGGREGATE_AUDIT: &str = "audit";
/// `outbox_event.event_type`：一条操作日志（审计日志表举例）。
pub const EVENT_OPER_LOG: &str = "audit.oper_log";
/// `outbox_event.event_type`：一条登录日志。
pub const EVENT_LOGIN_LOG: &str = "audit.login_log";
/// `outbox_event.event_type`：一次字段级变更留痕（`sys_audit_log`）。
pub const EVENT_CHANGE_LOG: &str = "audit.change_log";

#[derive(Debug, thiserror::Error)]
pub enum EventError {
    #[error("事件类型 {0} 没有注册的处理器")]
    Unsupported(String),
    #[error("事件负载不是合法 JSON：{0}")]
    Malformed(String),
    #[error("事件投递失败：{0}")]
    Delivery(String),
}

/// 发件箱里的一行：与 `outbox_event` 的列一一对应（审计日志表）。
#[derive(Clone, Debug, PartialEq)]
pub struct DomainEvent {
    /// 发件箱行 id，relay 顺序扫描用。
    pub id: i64,
    pub tenant_id: i64,
    /// 事件源模块，如 `audit`。
    pub aggregate: String,
    /// 事件源记录标识（如操作人），可空。
    pub aggregate_id: Option<String>,
    /// 事件类型，如 `audit.oper_log`。
    pub event_type: String,
    /// 事件负载，列类型为 jsonb。
    pub payload: serde_json::Value,
    /// 事件产生时间（数据库文本形态）。投递时直接作为日志时间，
    /// 避免 relay 的延迟把 `oper_time` 写成投递时刻。
    pub created_at: String,
    /// 已尝试投递次数（relay 的退避与放弃判据）。
    pub attempts: i32,
}

impl DomainEvent {
    /// 负载反序列化；字段名由本模块的事件结构体固定。
    pub fn payload_as<T: serde::de::DeserializeOwned>(&self) -> Result<T, EventError> {
        serde_json::from_value(self.payload.clone())
            .map_err(|e| EventError::Malformed(e.to_string()))
    }
}

/// `audit.oper_log` 的负载（`sys_oper_log` 的输入列，属主 `system`）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OperLogEvent {
    pub tenant_id: i64,
    pub title: String,
    pub business_type: i16,
    pub method: String,
    pub request_method: String,
    pub operator: Option<String>,
    pub oper_url: String,
    pub oper_ip: Option<String>,
    pub oper_param: Option<String>,
    /// 0 成功 / 1 失败。
    pub status: i16,
    pub error_msg: Option<String>,
    pub cost_ms: i64,
    /// 审计设计后三项（UA / 业务码 / 脱敏请求体）。`#[serde(default)]` 是**兼容**用的：
    /// outbox 里可能躺着这次升级之前入队的事件，缺字段必须当 `None` 而不是让 relay 卡住。
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub biz_code: Option<String>,
    #[serde(default)]
    pub req_body: Option<String>,
}

/// `audit.login_log` 的负载（`sys_logininfor` 的输入列）。
///
/// 浏览器/操作系统不在这里算——那是 `sys_logininfor` 的列，属主 `system` 自己解析。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoginLogEvent {
    pub tenant_id: i64,
    pub user_name: String,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    /// 0 成功 / 1 失败。
    pub status: i16,
    pub message: Option<String>,
}

/// `audit.change_log` 的负载（`sys_audit_log` 的输入列，属主 `system`）。
///
/// `changed` 只含真正变了的字段，形态是 `{field: [old, new]}`（审计日志表）。
/// 生产端负责把凭据类字段滤掉——见 `cicada_audit::change::is_sensitive`。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChangeLogEvent {
    pub tenant_id: i64,
    /// 被改动的实体（表名）。
    pub table_name: String,
    /// 主键值，字符串化以兼容复合键。
    pub record_id: String,
    /// `insert` / `update` / `delete`。
    pub action: String,
    pub changed: serde_json::Value,
    /// 操作人（`sys_user.user_id`）。
    pub oper_id: Option<i64>,
    /// 关联链路（`RequestContext.trace_id`）。
    pub trace_id: Option<String>,
}

/// 事件消费端。实现者只写自己拥有的表，失败要返回 `Err` 让 relay 记账（`attempts`）。
#[async_trait]
pub trait EventHandler: Send + Sync + 'static {
    /// 本处理器负责的事件类型（精确匹配 `event_type`）。
    fn event_types(&self) -> Vec<&'static str>;

    async fn handle(&self, event: &DomainEvent) -> Result<(), EventError>;
}

/// 按事件类型分发的路由表。
#[derive(Default)]
pub struct EventRouter {
    handlers: Mutex<Vec<(String, Arc<dyn EventHandler>)>>,
}

impl EventRouter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个处理器；同一类型后注册者覆盖先注册者（便于测试替换）。
    pub fn register(&self, handler: Arc<dyn EventHandler>) {
        let mut guard = self.handlers.lock().unwrap();
        for ty in handler.event_types() {
            guard.retain(|(k, _)| k != ty);
            guard.push((ty.to_string(), handler.clone()));
        }
    }

    /// 已注册的事件类型集合（装配期对账用）。
    pub fn registered(&self) -> Vec<String> {
        self.handlers
            .lock()
            .unwrap()
            .iter()
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// 投递一条事件。没有对应处理器时返回 `Unsupported`，让事件留在发件箱并累加尝试次数，
    /// 而不是静默丢弃（丢审计等于销毁证据）。
    pub async fn dispatch(&self, event: &DomainEvent) -> Result<(), EventError> {
        let handler = self
            .handlers
            .lock()
            .unwrap()
            .iter()
            .find(|(k, _)| *k == event.event_type)
            .map(|(_, h)| h.clone());
        match handler {
            Some(h) => h.handle(event).await,
            None => Err(EventError::Unsupported(event.event_type.clone())),
        }
    }
}

/// 测试夹具：只记录收到的事件，不落任何表。
#[derive(Default)]
pub struct InMemoryEventHandler {
    types: Vec<&'static str>,
    received: Mutex<Vec<DomainEvent>>,
}

impl InMemoryEventHandler {
    pub fn new(types: Vec<&'static str>) -> Self {
        Self {
            types,
            received: Mutex::new(Vec::new()),
        }
    }

    pub fn received(&self) -> Vec<DomainEvent> {
        self.received.lock().unwrap().clone()
    }
}

#[async_trait]
impl EventHandler for InMemoryEventHandler {
    fn event_types(&self) -> Vec<&'static str> {
        self.types.clone()
    }

    async fn handle(&self, event: &DomainEvent) -> Result<(), EventError> {
        self.received.lock().unwrap().push(event.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn event(event_type: &str, payload: serde_json::Value) -> DomainEvent {
        DomainEvent {
            id: 1,
            tenant_id: 7,
            aggregate: AGGREGATE_AUDIT.to_string(),
            aggregate_id: Some("admin".to_string()),
            event_type: event_type.to_string(),
            payload,
            created_at: "2026-10-16 09:00:00.000000+00".to_string(),
            attempts: 0,
        }
    }

    #[tokio::test]
    async fn dispatch_routes_by_event_type() {
        let router = EventRouter::new();
        let sink = Arc::new(InMemoryEventHandler::new(vec![EVENT_OPER_LOG]));
        router.register(sink.clone());
        router
            .dispatch(&event(EVENT_OPER_LOG, json!({"tenant_id": 7})))
            .await
            .unwrap();
        assert_eq!(sink.received().len(), 1);
    }

    #[tokio::test]
    async fn unknown_type_is_an_error_not_a_drop() {
        let router = EventRouter::new();
        let err = router
            .dispatch(&event(EVENT_LOGIN_LOG, json!({})))
            .await
            .unwrap_err();
        assert!(matches!(err, EventError::Unsupported(_)));
    }

    #[tokio::test]
    async fn payload_contract_round_trips() {
        let op = OperLogEvent {
            tenant_id: 1,
            title: "system:user:add".into(),
            business_type: 1,
            method: "POST".into(),
            request_method: "/api/v1/users".into(),
            operator: Some("admin".into()),
            oper_url: "/api/v1/users".into(),
            oper_ip: Some("127.0.0.1".into()),
            oper_param: Some("{\"name\":\"n\"}".into()),
            status: 0,
            error_msg: None,
            cost_ms: 12,
            user_agent: None,
            biz_code: None,
            req_body: None,
        };
        let ev = event(EVENT_OPER_LOG, serde_json::to_value(&op).unwrap());
        assert_eq!(ev.payload_as::<OperLogEvent>().unwrap(), op);
    }

    #[tokio::test]
    async fn malformed_payload_reports_malformed() {
        let ev = event(EVENT_OPER_LOG, json!("not-an-object"));
        assert!(matches!(
            ev.payload_as::<OperLogEvent>().unwrap_err(),
            EventError::Malformed(_)
        ));
    }
}
