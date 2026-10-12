//! 审计写入缝的三个实现：发件箱、内存（测试）、空实现。
//!
//! 落库形态由  决定：`audit` 是生产者，**不直接写 `system` 的日志表**，
//! 所以这里没有「直连落库」的 sink 了——真实进程用 `OutboxAuditSink`（见 `outbox`），
//! 落地由 relay 投递给 `system` 的 `EventHandler`。

use std::sync::Mutex;

use async_trait::async_trait;

use crate::{ChangeLog, LoginLog, OperLog};

/// 审计写入缝。实现必须吞掉自身错误（只 `warn`），审计不得阻断业务。
#[async_trait]
pub trait AuditSink: Send + Sync + 'static {
    async fn record_oper(&self, log: OperLog);
    async fn record_login(&self, log: LoginLog);

    /// 字段级变更留痕（审计设计，P1）。
    ///
    /// 默认不记录：只有真正接了前后快照的 sink 才落地，未接入的场合（内存夹具、
    /// 关掉审计的部署）行为与加这个方法之前完全一致。
    async fn record_change(&self, log: ChangeLog) {
        let _ = log;
    }
}

/// 空实现：审计关闭时的兜底。
#[derive(Default, Clone, Copy)]
pub struct NoopAuditSink;

#[async_trait]
impl AuditSink for NoopAuditSink {
    async fn record_oper(&self, _log: OperLog) {}
    async fn record_login(&self, _log: LoginLog) {}
}

/// 内存实现：测试断言用。
#[derive(Default)]
pub struct InMemoryAuditSink {
    opers: Mutex<Vec<OperLog>>,
    logins: Mutex<Vec<LoginLog>>,
    changes: Mutex<Vec<ChangeLog>>,
}

impl InMemoryAuditSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn opers(&self) -> Vec<OperLog> {
        self.opers.lock().unwrap().clone()
    }

    pub fn logins(&self) -> Vec<LoginLog> {
        self.logins.lock().unwrap().clone()
    }

    pub fn changes(&self) -> Vec<ChangeLog> {
        self.changes.lock().unwrap().clone()
    }
}

#[async_trait]
impl AuditSink for InMemoryAuditSink {
    async fn record_oper(&self, log: OperLog) {
        self.opers.lock().unwrap().push(log);
    }

    async fn record_login(&self, log: LoginLog) {
        self.logins.lock().unwrap().push(log);
    }

    async fn record_change(&self, log: ChangeLog) {
        self.changes.lock().unwrap().push(log);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_collects_both_kinds() {
        let sink = InMemoryAuditSink::new();
        sink.record_login(LoginLog::success(1, "admin", None, None))
            .await;
        sink.record_oper(OperLog {
            user_agent: None,
            biz_code: None,
            req_body: None,
            tenant_id: 1,
            title: "system:user:add".into(),
            business_type: 1,
            method: "POST".into(),
            request_method: "/users".into(),
            operator: Some("admin".into()),
            oper_url: "/users".into(),
            oper_ip: None,
            oper_param: None,
            status: 0,
            error_msg: None,
            cost_ms: 3,
        })
        .await;
        assert_eq!(sink.logins().len(), 1);
        assert_eq!(sink.opers().len(), 1);
    }
}
