//! `#[derive(Maskable)]` 展开测试：按策略掩码 String 与 Option<String> 字段。

use cicada_kernel::mask::{MaskPolicy, Maskable};
use cicada_macros::Maskable;
use serde::Serialize;

#[derive(Debug, PartialEq, Serialize, Maskable)]
struct Contact {
    #[mask(Mobile)]
    phone: Option<String>,
    #[mask(Email)]
    email: String,
    #[mask(Name)]
    nick_name: String,
    plain_field: String,
}

#[test]
fn masked_policy_masks_declared_fields_only() {
    let c = Contact {
        phone: Some("13812345678".into()),
        email: "zhangsan@corp.com".into(),
        nick_name: "张三丰".into(),
        plain_field: "keep".into(),
    }
    .masked(MaskPolicy::Masked);

    assert_eq!(c.phone.as_deref(), Some("138****5678"));
    assert_eq!(c.email, "z*******@corp.com");
    assert_eq!(c.nick_name, "张**");
    assert_eq!(c.plain_field, "keep");
}

#[test]
fn plain_policy_is_noop() {
    let original = Contact {
        phone: Some("13812345678".into()),
        email: "zhangsan@corp.com".into(),
        nick_name: "张三丰".into(),
        plain_field: "keep".into(),
    };
    let same = original.masked(MaskPolicy::Plain);
    assert_eq!(same.phone.as_deref(), Some("13812345678"));
    assert_eq!(same.email, "zhangsan@corp.com");
}

#[test]
fn none_phone_stays_none() {
    let c = Contact {
        phone: None,
        email: "a@b.co".into(),
        nick_name: "王".into(),
        plain_field: String::new(),
    }
    .masked(MaskPolicy::Masked);
    assert_eq!(c.phone, None);
    assert_eq!(c.nick_name, "*");
}
