//! 认证后的请求主体（`Principal`）。

use serde::{Deserialize, Serialize};

use crate::id::{TenantId, UserId};

/// 会话标识（JWT 的 `jti`）。
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 已认证用户的可信快照。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    pub user_id: UserId,
    pub tenant_id: TenantId,
    pub session_id: SessionId,
    pub user_name: String,
}
