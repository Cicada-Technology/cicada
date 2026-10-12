//! 字段级变更留痕。
//!
//! 语义：更新前后各取一次序列化快照，只记录**变化**的字段，形态 `{field: [old, new]}`。
//! 本模块只做纯计算，不碰数据库——落地由 `audit` 写进自己的发件箱，
//! relay 投递给属主 `system` 写 `sys_audit_log`（审计设计 / ADR 0008）。

use serde_json::{Map, Value};

/// 变更动作（审计日志表的 `action` 列）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeAction {
    Insert,
    Update,
    Delete,
}

impl ChangeAction {
    /// 落库值（与审计日志表的 CHECK 约束一致）。
    pub fn code(self) -> &'static str {
        match self {
            Self::Insert => "insert",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

/// 一次留痕。`changed` 由 [`diff`] 产出，只含变化字段。
#[derive(Clone, Debug, PartialEq)]
pub struct ChangeLog {
    pub tenant_id: i64,
    /// 被改动的实体（表名）。
    pub table_name: String,
    /// 主键值，字符串化以兼容复合键。
    pub record_id: String,
    pub action: ChangeAction,
    pub changed: Value,
    /// 操作人。
    pub oper_id: Option<i64>,
    /// 关联链路。
    pub trace_id: Option<String>,
}

impl ChangeLog {
    pub fn inserted(
        tenant_id: i64,
        table_name: impl Into<String>,
        record_id: impl Into<String>,
        after: &Value,
        oper_id: Option<i64>,
        trace_id: Option<String>,
    ) -> Self {
        Self {
            tenant_id,
            table_name: table_name.into(),
            record_id: record_id.into(),
            action: ChangeAction::Insert,
            changed: diff(&Value::Null, after),
            oper_id,
            trace_id,
        }
    }

    pub fn updated(
        tenant_id: i64,
        table_name: impl Into<String>,
        record_id: impl Into<String>,
        before: &Value,
        after: &Value,
        oper_id: Option<i64>,
        trace_id: Option<String>,
    ) -> Self {
        Self {
            tenant_id,
            table_name: table_name.into(),
            record_id: record_id.into(),
            action: ChangeAction::Update,
            changed: diff(before, after),
            oper_id,
            trace_id,
        }
    }

    pub fn deleted(
        tenant_id: i64,
        table_name: impl Into<String>,
        record_id: impl Into<String>,
        before: &Value,
        oper_id: Option<i64>,
        trace_id: Option<String>,
    ) -> Self {
        Self {
            tenant_id,
            table_name: table_name.into(),
            record_id: record_id.into(),
            action: ChangeAction::Delete,
            changed: diff(before, &Value::Null),
            oper_id,
            trace_id,
        }
    }

    /// 没有任何字段变化（或全被脱敏滤掉）时不必留痕。
    pub fn is_empty(&self) -> bool {
        self.changed.as_object().is_some_and(|map| map.is_empty())
    }
}

/// 前后快照的差异，只保留变化字段：`{field: [old, new]}`。
///
/// 一侧不是对象（例如整列缺失）时按空对象处理；凭据类字段一律不落库。
pub fn diff(before: &Value, after: &Value) -> Value {
    let mut out = Map::new();
    let empty = Map::new();
    let b = before.as_object().unwrap_or(&empty);
    let a = after.as_object().unwrap_or(&empty);
    let mut keys: Vec<&String> = b.keys().chain(a.keys()).collect();
    keys.sort_unstable();
    keys.dedup();
    for key in keys {
        if is_sensitive(key) {
            continue;
        }
        let old = b.get(key).cloned().unwrap_or(Value::Null);
        let new = a.get(key).cloned().unwrap_or(Value::Null);
        if old != new {
            out.insert(key.clone(), Value::Array(vec![old, new]));
        }
    }
    Value::Object(out)
}

/// 凭据与密钥类字段不进留痕：留痕表是给人读的，快照一旦落盘就长期存在。
///
/// 按小写子串匹配，覆盖 `password` / `secret_hash` / `refresh_token` / `totp_secret` 这类命名。
pub fn is_sensitive(key: &str) -> bool {
    const MARKERS: &[&str] = &[
        "password",
        "secret",
        "token",
        "totp",
        "credential",
        "hash",
        "jwt",
        "signature",
    ];
    let lowered = key.to_lowercase();
    MARKERS.iter().any(|m| lowered.contains(m))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn diff_keeps_only_changed_fields() {
        let before = json!({"user_name": "admin", "nick_name": "老管理员", "status": 0});
        let after = json!({"user_name": "admin", "nick_name": "新管理员", "status": 1});
        let changed = diff(&before, &after);
        let map = changed.as_object().unwrap();
        assert_eq!(map.len(), 2, "{changed}");
        assert_eq!(map["nick_name"], json!(["老管理员", "新管理员"]));
        assert_eq!(map["status"], json!([0, 1]));
        assert!(!map.contains_key("user_name"), "未变化的字段不该留痕");
    }

    #[test]
    fn diff_records_added_and_removed_fields() {
        let changed = diff(&json!({"a": 1}), &json!({"b": 2}));
        assert_eq!(changed, json!({"a": [1, null], "b": [null, 2]}));
    }

    #[test]
    fn diff_drops_credentials() {
        let before = json!({"password": "old", "secret_hash": "h1", "dept_id": 1});
        let after = json!({"password": "new", "secret_hash": "h2", "dept_id": 2});
        let changed = diff(&before, &after);
        let map = changed.as_object().unwrap();
        assert_eq!(
            map.keys().cloned().collect::<Vec<_>>(),
            vec!["dept_id".to_string()]
        );
    }

    #[test]
    fn non_object_snapshots_are_treated_as_empty() {
        assert_eq!(
            diff(&json!("scalar"), &json!({"a": 1})),
            json!({"a": [null, 1]})
        );
        assert_eq!(diff(&Value::Null, &Value::Null), json!({}));
    }

    #[test]
    fn actions_map_to_appendix_f_codes() {
        assert_eq!(ChangeAction::Insert.code(), "insert");
        assert_eq!(ChangeAction::Update.code(), "update");
        assert_eq!(ChangeAction::Delete.code(), "delete");
    }

    #[test]
    fn constructors_flag_no_change_cases() {
        let same = json!({"nick_name": "x", "password": "ignored"});
        let log = ChangeLog::updated(1, "sys_user", "7", &same, &same, None, None);
        assert!(log.is_empty());

        let ins = ChangeLog::inserted(
            1,
            "sys_user",
            "7",
            &json!({"nick_name": "y"}),
            Some(9),
            None,
        );
        assert_eq!(ins.action, ChangeAction::Insert);
        assert_eq!(ins.changed, json!({"nick_name": [null, "y"]}));
        assert!(!ins.is_empty());
    }

    #[test]
    fn sensitive_matching_is_case_insensitive() {
        assert!(is_sensitive("Password"));
        assert!(is_sensitive("refresh_TOKEN"));
        assert!(is_sensitive("secret_hash"));
        assert!(!is_sensitive("nick_name"));
        assert!(!is_sensitive("status"));
    }
}
