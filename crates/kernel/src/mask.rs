//! 数据脱敏：字段级掩码函数与按角色的明文/掩码策略。
//!
//! 掩码规则与参考实现前端展示习惯对齐（手机 `138****5678`、身份证保留前 6 后 4、
//! 邮箱保留首字符与域名、姓名保留首字）。策略由调用方在边界处派生：
//! 超级管理员见明文，其余角色掩码（与权限模型超管短路同一约定）。

use serde::{Deserialize, Serialize};

/// 掩码类型。字段级宏 `#[mask(Mobile)]` 的合法取值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaskKind {
    /// 手机号：保留前 3 后 4。
    Mobile,
    /// 身份证：保留前 6 后 4。
    IdCard,
    /// 邮箱：用户名保留首字符，域名保留。
    Email,
    /// 姓名：保留首字。
    Name,
}

/// 序列化侧的明文/掩码开关。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskPolicy {
    Plain,
    Masked,
}

impl MaskPolicy {
    pub fn is_masked(self) -> bool {
        matches!(self, Self::Masked)
    }
}

/// 可脱敏对象：由 `#[derive(Maskable)]` 实现，逐字段应用 `#[mask(..)]` 声明。
pub trait Maskable {
    /// 就地按策略掩码；`Plain` 时什么都不做。
    fn apply_mask(&mut self, policy: MaskPolicy);

    fn masked(mut self, policy: MaskPolicy) -> Self
    where
        Self: Sized,
    {
        self.apply_mask(policy);
        self
    }
}

/// 按字符数取掩码后的字符串；对不满足预期长度的输入做防御性整体打星。
pub fn mask(kind: MaskKind, value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let n = chars.len();
    match kind {
        MaskKind::Mobile if n > 7 => {
            let mut out = String::with_capacity(n);
            out.extend(&chars[..3]);
            out.push_str(&"*".repeat(n - 7));
            out.extend(&chars[n - 4..]);
            out
        }
        MaskKind::IdCard if n >= 11 => {
            let mut out = String::with_capacity(n);
            out.extend(&chars[..6]);
            out.push_str(&"*".repeat(n - 10));
            out.extend(&chars[n - 4..]);
            out
        }
        MaskKind::Email => match value.split_once('@') {
            Some((user, domain)) if !user.is_empty() => {
                let head: char = user.chars().next().unwrap();
                let stars = "*".repeat(user.chars().count().saturating_sub(1));
                format!("{head}{stars}@{domain}")
            }
            _ => "*".repeat(n.max(1)),
        },
        MaskKind::Name if n >= 2 => {
            let head = chars[0];
            format!("{head}{}", "*".repeat(n - 1))
        }
        _ => "*".repeat(n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mobile_keeps_head3_tail4() {
        assert_eq!(mask(MaskKind::Mobile, "13812345678"), "138****5678");
        assert_eq!(mask(MaskKind::Mobile, "1381234"), "*******");
        assert_eq!(mask(MaskKind::Mobile, ""), "");
    }

    #[test]
    fn id_card_keeps_head6_tail4() {
        assert_eq!(
            mask(MaskKind::IdCard, "110101199003074578"),
            "110101********4578"
        );
    }

    #[test]
    fn email_keeps_first_char_and_domain() {
        assert_eq!(
            mask(MaskKind::Email, "example@corp.com"),
            "e******@corp.com"
        );
        assert_eq!(mask(MaskKind::Email, "no-at-sign"), "**********");
    }

    #[test]
    fn name_keeps_first_char() {
        assert_eq!(mask(MaskKind::Name, "张三丰"), "张**");
        assert_eq!(mask(MaskKind::Name, "李"), "*");
    }

    #[test]
    fn policy_helper() {
        assert!(!MaskPolicy::Plain.is_masked());
        assert!(MaskPolicy::Masked.is_masked());
    }
}
