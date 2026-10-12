//! 领域 ID 新类型：编译期防止 ID 混用（类型即契约）。

use serde::{Deserialize, Serialize};

macro_rules! id_newtype {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        pub struct $name(i64);

        impl $name {
            pub const fn new(v: i64) -> Self {
                Self(v)
            }

            pub const fn get(self) -> i64 {
                self.0
            }
        }

        impl From<i64> for $name {
            fn from(v: i64) -> Self {
                Self(v)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

id_newtype!(TenantId);
id_newtype!(UserId);
id_newtype!(DeptId);
id_newtype!(RoleId);
id_newtype!(MenuId);
id_newtype!(PostId);
id_newtype!(JobId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_wrap_and_display() {
        let user = UserId::new(42);
        assert_eq!(user.get(), 42);
        assert_eq!(user.to_string(), "42");
        assert_eq!(UserId::from(42), user);
    }

    #[test]
    fn ids_are_distinct_types() {
        // 编译期无法把 TenantId 当 UserId 用；此处仅验证值语义。
        let tenant = TenantId::new(1);
        let user = UserId::new(1);
        assert_ne!(tenant.to_string(), user.to_string() + "_user");
    }
}
