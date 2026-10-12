//! cicada-audit（L3 能力）
//!
//! 审计：操作日志 / 登录日志的采集与投递。
//!
//! handler 用 `#[audit(module = "..", action = "..", kind = "..")]` 在编译期声明
//! 审计元数据（过程宏生成 `<name>__audit()`），运行时由 `cicada-web` 的
//! `audit_middleware` 读取元数据并写日志；登录日志由认证接口在登录尝试处写入。
//!
//! 字段级变更留痕（审计设计，P1）由 `change` 模块计算差异（只留变化字段，凭据类字段不落库），
//! 同样经发件箱交给属主落 `sys_audit_log`。
//!
//! 日志表属主是 `system`，因此本 crate 只把事件写进自己的 `outbox_event`，
//! 由 relay 投递给 `system` 的落库处理器（审计设计 / 模块契约数据所有权）。
//! 写审计失败绝不阻断业务请求：`AuditSink` 的实现只记录 `warn`。

pub mod change;
pub mod outbox;
pub mod sink;

use std::fmt;

pub use change::{ChangeAction, ChangeLog, diff, is_sensitive};
pub use outbox::{DrainReport, Outbox, OutboxAuditSink};
pub use sink::{AuditSink, InMemoryAuditSink, NoopAuditSink};

/// 编译期声明的审计元数据（`#[audit]` 宏产出）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuditMeta {
    /// 模块名，如 `system` / `auth`。
    pub module: &'static str,
    /// 业务动作，如 `user:add`。日志 `title` 用 `{module}:{action}`。
    pub action: &'static str,
    /// 业务类型码，见 [`OperType`]。
    pub business_type: i16,
}

impl AuditMeta {
    /// 日志标题（参考实现 `sys_oper_log.title` 语义）。
    pub fn title(&self) -> String {
        format!("{}:{}", self.module, self.action)
    }
}

/// 操作类型（参考实现 `BusinessType`，数值保持兼容）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperType {
    Other = 0,
    Insert = 1,
    Update = 2,
    Delete = 3,
    Grant = 4,
    Export = 5,
    Import = 6,
    Force = 7,
    GenCode = 8,
    Clean = 9,
}

impl OperType {
    /// 从 `#[audit(kind = "..")]` 的稳定字符串解析。
    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "other" => Self::Other,
            "insert" => Self::Insert,
            "update" => Self::Update,
            "delete" => Self::Delete,
            "grant" => Self::Grant,
            "export" => Self::Export,
            "import" => Self::Import,
            "force" => Self::Force,
            "gencode" | "gen_code" => Self::GenCode,
            "clean" => Self::Clean,
            _ => return None,
        })
    }

    pub const fn code(self) -> i16 {
        self as i16
    }
}

/// 一次操作日志（`sys_oper_log` 一行的输入）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperLog {
    pub tenant_id: i64,
    pub title: String,
    pub business_type: i16,
    pub method: String,
    pub request_method: String,
    pub operator: Option<String>,
    pub oper_url: String,
    pub oper_ip: Option<String>,
    pub oper_param: Option<String>,
    /// 0 成功 / 1 失败（与业务码目录的 `sys_common_status` 一致）。
    pub status: i16,
    pub error_msg: Option<String>,
    pub cost_ms: i64,
    /// 浏览器 UA（审计设计要求记录；`sys_oper_log` 早期版本没有这一列）。
    pub user_agent: Option<String>,
    /// 业务码目录的业务码（失败时才有）。`error_msg` 是给人看的一句话，
    /// 业务码才是能 GROUP BY 的那个维度。
    pub biz_code: Option<String>,
    /// **脱敏后**的请求体（审计设计）。查询串在 `oper_param`，两者不重复。
    pub req_body: Option<String>,
}

/// 一次登录日志（`sys_logininfor` 一行的输入）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginLog {
    pub tenant_id: i64,
    pub user_name: String,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    /// 0 成功 / 1 失败。
    pub status: i16,
    pub message: Option<String>,
}

impl LoginLog {
    pub const SUCCESS: i16 = 0;
    pub const FAIL: i16 = 1;

    pub fn success(
        tenant_id: i64,
        user_name: impl Into<String>,
        ip: Option<String>,
        ua: Option<String>,
    ) -> Self {
        Self {
            tenant_id,
            user_name: user_name.into(),
            ip,
            user_agent: ua,
            status: Self::SUCCESS,
            message: None,
        }
    }

    pub fn failure(
        tenant_id: i64,
        user_name: impl Into<String>,
        ip: Option<String>,
        ua: Option<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            tenant_id,
            user_name: user_name.into(),
            ip,
            user_agent: ua,
            status: Self::FAIL,
            message: Some(message.into()),
        }
    }
}

impl fmt::Display for OperType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oper_type_keys_map_to_codes() {
        assert_eq!(OperType::from_key("insert").unwrap().code(), 1);
        assert_eq!(OperType::from_key("force").unwrap().code(), 7);
        assert_eq!(OperType::from_key("gencode").unwrap().code(), 8);
        assert!(OperType::from_key("bogus").is_none());
    }

    #[test]
    fn meta_title_is_module_colon_action() {
        let meta = AuditMeta {
            module: "system",
            action: "user:add",
            business_type: OperType::Insert.code(),
        };
        assert_eq!(meta.title(), "system:user:add");
    }
}
