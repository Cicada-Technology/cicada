//! 随请求流转的不可变快照 `RequestContext`。
//!
//! 持久层不直接消费它：service 在边界把它收敛为窄接口 `Scope`
//! （见 [`RequestContext::scope`]），DB 测试因此无需构造 HTTP 上下文。

use std::net::IpAddr;

use crate::id::TenantId;
use crate::permission::Permission;
use crate::principal::Principal;
use crate::scope::{DataScope, Scope};

/// 请求唯一标识（透传或生成 `X-Request-Id`）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RequestId(pub String);

/// 链路追踪标识。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TraceId(pub String);

impl RequestId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TraceId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 已由 `auth` / `rbac` 中间件计算完毕的请求快照。
///
/// 与  的唯一差异：`user_agent` 用 `Option<String>` 而非 `HeaderValue`，
/// 使 kernel 保持不依赖 HTTP 类型；HTTP 头到 `String` 的提取由接入层完成。
#[derive(Clone, Debug)]
pub struct RequestContext {
    pub request_id: RequestId,
    /// 匿名请求为 `None`。
    pub principal: Option<Principal>,
    pub tenant_id: TenantId,
    /// 已按角色合并计算完毕。
    pub data_scope: DataScope,
    /// 已解析的权限标识集合（匿名请求为空）。
    pub permissions: Vec<Permission>,
    /// 已解析的角色键集合（匿名请求为空）。
    pub roles: Vec<String>,
    /// 超级管理员短路标记（权限检查放行）。
    pub is_super_admin: bool,
    pub ip: IpAddr,
    pub user_agent: Option<String>,
    pub trace_id: TraceId,
    /// 协商后的响应语言（服务端国际化；接入层按 `Accept-Language` 填充）。
    pub locale: String,
}

impl RequestContext {
    /// 收敛为持久层只认的窄接口：谁、在哪个租户、可见范围多大。
    pub fn scope(&self) -> Scope {
        Scope {
            tenant_id: self.tenant_id,
            user_id: self.principal.as_ref().map(|p| p.user_id),
            data_scope: self.data_scope.clone(),
        }
    }

    /// 权限判定：超级管理员短路，否则查权限集合。
    pub fn has_permission(&self, perm: &Permission) -> bool {
        self.is_super_admin || self.permissions.iter().any(|p| p == perm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::UserId;
    use crate::principal::SessionId;
    use crate::scope::DataScope;
    use std::net::Ipv4Addr;

    fn principal(user_id: i64) -> Principal {
        Principal {
            user_id: UserId::new(user_id),
            tenant_id: TenantId::new(1),
            session_id: SessionId("s".to_string()),
            user_name: "admin".to_string(),
        }
    }

    #[test]
    fn scope_derives_narrow_scope() {
        let ctx = RequestContext {
            request_id: RequestId::new("req-1"),
            principal: Some(principal(7)),
            tenant_id: TenantId::new(1),
            data_scope: DataScope::all(),
            permissions: Vec::new(),
            roles: Vec::new(),
            is_super_admin: false,
            ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            user_agent: None,
            trace_id: TraceId::new("t-1"),
            locale: "zh-CN".to_string(),
        };
        let scope = ctx.scope();
        assert_eq!(scope.tenant_id, TenantId::new(1));
        assert_eq!(scope.user_id, Some(UserId::new(7)));
        assert_eq!(scope.data_scope.kind, DataScope::all().kind);
    }

    #[test]
    fn has_permission_short_circuits_for_super_admin() {
        let mut ctx = RequestContext {
            request_id: RequestId::new("req-1"),
            principal: None,
            tenant_id: TenantId::new(1),
            data_scope: DataScope::all(),
            permissions: Vec::new(),
            roles: Vec::new(),
            is_super_admin: true,
            ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            user_agent: None,
            trace_id: TraceId::new("t-1"),
            locale: "zh-CN".to_string(),
        };
        assert!(ctx.has_permission(&Permission::new("system:user:add")));

        ctx.is_super_admin = false;
        assert!(!ctx.has_permission(&Permission::new("system:user:add")));
        ctx.permissions.push(Permission::new("system:user:add"));
        assert!(ctx.has_permission(&Permission::new("system:user:add")));
    }
}
