//! 模板渲染：`minijinja`，上下文就是投递 `payload`。
//!
//! 未定义变量按**严格模式**报错（而不是渲染成空串）：模板写错字段名应当在
//! 发送时立刻暴露，而不是安静地发出「尊敬的 ，您好」。

use minijinja::{Environment, UndefinedBehavior};

use crate::channel::NotifyError;

/// 渲染标题与正文。
pub fn render(
    title_template: &str,
    body_template: &str,
    payload: &serde_json::Value,
) -> Result<(String, String), NotifyError> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.add_template("title", title_template)
        .map_err(|e| NotifyError::Render(e.to_string()))?;
    env.add_template("body", body_template)
        .map_err(|e| NotifyError::Render(e.to_string()))?;

    let ctx = minijinja::Value::from_serialize(payload);
    let title = env
        .get_template("title")
        .and_then(|t| t.render(&ctx))
        .map_err(|e| NotifyError::Render(e.to_string()))?;
    let body = env
        .get_template("body")
        .and_then(|t| t.render(&ctx))
        .map_err(|e| NotifyError::Render(e.to_string()))?;
    Ok((title, body))
}

/// 只解析、不渲染：用于保存模板时立刻暴露语法错误。
pub fn validate(title_template: &str, body_template: &str) -> Result<(), NotifyError> {
    let mut env = Environment::new();
    env.add_template("title", title_template)
        .map_err(|e| NotifyError::Render(e.to_string()))?;
    env.add_template("body", body_template)
        .map_err(|e| NotifyError::Render(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_payload_fields() {
        let payload = serde_json::json!({ "user": "admin", "code": "123456" });
        let (title, body) = render("验证码", "{{ user }} 的验证码是 {{ code }}", &payload).unwrap();
        assert_eq!(title, "验证码");
        assert_eq!(body, "admin 的验证码是 123456");
    }

    #[test]
    fn validate_rejects_broken_syntax() {
        assert!(validate("题目", "{{ name }} 你好").is_ok());
        assert!(validate("题目", "{% if %}").is_err());
    }

    #[test]
    fn strict_mode_rejects_unknown_fields() {
        let payload = serde_json::json!({ "a": 1 });
        assert!(render("t", "{{ missing }}", &payload).is_err());
    }
}
