//! 口令强度规则。
//!
//! 为什么不在 `cicada-auth`：`migrations`（`init_admin` / `tenant init` / `admin reset`）
//! 也要用同一份规则，而数据层不许反向依赖认证域。阈值本来就配在 `config::PasswordConfig`，
//! 规则与阈值放一起，谁也漏不掉另一半。

/// 口令规则：长度 ≥ `min_length`，且大写、小写、数字各至少一个（允许特殊字符）。
///
/// 放在 `infra`（配置的家）而不是各接口里，是因为「改密处统一用 validator 拦截」要求的正是
/// 同一份规则：建号、管理员重置、导入用初始密码、CLI 初始化超管——各写一套迟早会不一致。
/// 返回人类可读的违规项（调用点原样贴进统一响应的 `errors[]`），不返回错误码：
/// 这不是失败，而是输入校验，状态码由调用点决定（422）。
pub fn password_problems(pw: &str, min_length: usize) -> Vec<&'static str> {
    let mut out = Vec::new();
    // 按字符计，不按字节：中文口令「张三的密码」不该因为 UTF-8 长度被误判为够长。
    if pw.chars().count() < min_length {
        out.push("长度不足");
    }
    if !pw.chars().any(|c| c.is_ascii_uppercase()) {
        out.push("缺大写字母");
    }
    if !pw.chars().any(|c| c.is_ascii_lowercase()) {
        out.push("缺小写字母");
    }
    if !pw.chars().any(|c| c.is_ascii_digit()) {
        out.push("缺数字");
    }
    out
}

/// 组装成一句提示，给只吃 `detail` 的调用点用。
pub fn password_problem_text(problems: &[&'static str], min_length: usize) -> String {
    if problems.is_empty() {
        return String::new();
    }
    format!(
        "口令不符合规则（{}）：至少 {min_length} 位，且需包含大写字母、小写字母与数字各一个",
        problems.join("、")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_example_password_passes() {
        assert!(password_problems("Cicada@2026", 8).is_empty());
    }

    #[test]
    fn every_rule_is_reported_together() {
        assert_eq!(
            password_problems("abc", 8),
            vec!["长度不足", "缺大写字母", "缺数字"]
        );
    }

    #[test]
    fn length_counts_characters_not_bytes() {
        // 7 个汉字是 21 字节，但按字符算仍不够 8 位。
        assert!(
            password_problems("一个很长的密码", 8).contains(&"长度不足"),
            "中文按字节会误判为够长"
        );
        let problems = password_problems("一个很长的密码啊", 8);
        assert!(!problems.contains(&"长度不足"), "{problems:?}");
    }

    #[test]
    fn empty_password_fails_every_rule() {
        assert_eq!(
            password_problems("", 8),
            vec!["长度不足", "缺大写字母", "缺小写字母", "缺数字"]
        );
    }

    #[test]
    fn min_length_comes_from_config() {
        assert!(password_problems("Abc123", 6).is_empty());
        assert!(password_problems("Abc12", 6).contains(&"长度不足"));
    }

    #[test]
    fn problem_text_names_the_minimum_length() {
        let text = password_problem_text(&["缺数字"], 8);
        assert!(text.contains("缺数字") && text.contains("8"), "{text}");
        assert_eq!(password_problem_text(&[], 8), "");
    }
}
