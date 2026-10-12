//! 权限标识：`模块:资源:操作`。

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Permission(String);

impl Permission {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Permission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_holds_identifier() {
        let p = Permission::new("system:user:add");
        assert_eq!(p.as_str(), "system:user:add");
        assert_eq!(p.to_string(), "system:user:add");
    }
}
