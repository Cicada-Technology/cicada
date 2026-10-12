//! 标识符命名：库表名 / 列名 → Rust 与前端标识。
//!
//! 生成器只做「确定性」的机械转换，不做猜测：同一个表名永远得到同一个
//! `class_name` / `business_name`，因此生成结果可快照、可 diff。

/// 拆词：在 `_`、`-`、空格与 camelCase 边界处切分。
fn split_words(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for ch in s.chars() {
        if ch == '_' || ch == '-' || ch.is_whitespace() {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if ch.is_uppercase() && prev_lower && !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
        prev_lower = ch.is_lowercase() || ch.is_ascii_digit();
        cur.push(ch);
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// `sys_user_role` / `SysUserRole` → `SysUserRole`。
pub fn pascal_case(s: &str) -> String {
    split_words(s)
        .into_iter()
        .map(|w| {
            let mut cs = w.chars();
            match cs.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &cs.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect()
}

/// `SysUserRole` / `sys-user-role` → `sys_user_role`。
pub fn snake_case(s: &str) -> String {
    split_words(s)
        .into_iter()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join("_")
}

/// Rust 关键字：作为字段/模块标识时必须加后缀，否则生成代码无法编译。
const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "become", "box", "break", "const", "continue", "crate", "do", "dyn",
    "else", "enum", "extern", "false", "final", "fn", "for", "if", "impl", "in", "let", "loop",
    "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref", "return", "self",
    "static", "struct", "super", "trait", "true", "try", "type", "typeof", "unsafe", "unsized",
    "use", "virtual", "where", "while", "yield", "union",
];

/// 确保是合法的 Rust 标识符：关键字加 `_` 后缀，数字开头加 `_` 前缀。
pub fn safe_ident(name: &str) -> String {
    let mut out = name.to_string();
    if out.is_empty() {
        return "_unnamed".to_string();
    }
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    if RUST_KEYWORDS.contains(&out.as_str()) {
        out.push('_');
    }
    out
}

/// 列名 → Rust 字段名。
///
/// 关键字用 `_col` 后缀（不是单个 `_`）：`DeriveEntityModel` 由**字段名**推导
/// `Column` 变体名，`type_col` → `TypeCol` 是双方都无歧义的形态；
/// 而 `type_` 经 PascalCase 后是 `Type` 还是 `Type_` 取决于 derive 的实现细节。
pub fn field_ident(column: &str) -> String {
    let mut s = snake_case(column);
    if s.is_empty() {
        return "unnamed_col".to_string();
    }
    if s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    if RUST_KEYWORDS.contains(&s.as_str()) {
        s.push_str("_col");
    }
    s
}

/// 去掉表名前缀（按 `prefixes` 中**最长匹配**优先），返回剩余部分。
pub fn strip_prefix<'a>(table: &'a str, prefixes: &[String]) -> &'a str {
    let mut best: Option<&str> = None;
    for p in prefixes {
        if p.is_empty() {
            continue;
        }
        if let Some(rest) = table.strip_prefix(p.as_str())
            && !rest.is_empty()
            && best.is_none_or(|b| rest.len() < b.len())
        {
            best = Some(rest);
        }
    }
    best.unwrap_or(table)
}

/// 表名 → Rust 类型名（PascalCase），去前缀。
pub fn class_name(table: &str, prefixes: &[String]) -> String {
    pascal_case(strip_prefix(table, prefixes))
}

/// 表名 → 业务名（snake_case），去前缀。用作路由段与 `gen_table.business_name`。
pub fn business_name(table: &str, prefixes: &[String]) -> String {
    safe_ident(&snake_case(strip_prefix(table, prefixes)))
}

/// 表名 → 模块名（snake_case）。去前缀后仍有 ≥3 段时取首段
/// （`goods_sku_attr` → `goods`），否则回落配置里的默认模块名。
pub fn module_name(table: &str, prefixes: &[String], default_module: &str) -> String {
    let rest = snake_case(strip_prefix(table, prefixes));
    let mut parts = rest.split('_');
    let first = parts.next().unwrap_or_default();
    if rest.split('_').count() >= 3 && !first.is_empty() {
        safe_ident(first)
    } else {
        safe_ident(&snake_case(default_module))
    }
}

/// 标识符是否可安全内插进标识符位置（表名 / 列名）。
pub fn is_valid_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    s.len() <= 63 && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pascal_and_snake_round_trip() {
        assert_eq!(pascal_case("sys_user_role"), "SysUserRole");
        assert_eq!(pascal_case("biz-goods-sku"), "BizGoodsSku");
        assert_eq!(pascal_case("SysUserRole"), "SysUserRole");
        assert_eq!(snake_case("SysUserRole"), "sys_user_role");
        assert_eq!(snake_case("biz-goods"), "biz_goods");
    }

    #[test]
    fn prefix_stripping_prefers_longest_match() {
        let prefixes = vec!["sys_".to_string(), "sys_user_".to_string()];
        assert_eq!(strip_prefix("sys_user_role", &prefixes), "role");
        assert_eq!(strip_prefix("sys_role", &prefixes), "role");
        assert_eq!(strip_prefix("biz_goods", &prefixes), "biz_goods");
    }

    #[test]
    fn keywords_are_escaped() {
        assert_eq!(safe_ident("type"), "type_");
        assert_eq!(safe_ident("match"), "match_");
        assert_eq!(safe_ident("3d_model"), "_3d_model");
        assert_eq!(safe_ident("goods"), "goods");
    }

    #[test]
    fn module_name_uses_first_segment_for_deep_tables() {
        let prefixes = vec!["sys_".to_string()];
        // 去前缀后两段：用默认模块名（可预测优先于聪明）。
        assert_eq!(
            module_name("sys_user_role", &prefixes, "business"),
            "business"
        );
        assert_eq!(module_name("biz_goods", &prefixes, "business"), "business");
        // 去前缀后三段：取首段。
        assert_eq!(module_name("goods_sku_attr", &[], "business"), "goods");
    }

    #[test]
    fn field_ident_escapes_keywords_with_a_stable_suffix() {
        assert_eq!(field_ident("type"), "type_col");
        assert_eq!(field_ident("Type"), "type_col");
        assert_eq!(field_ident("goods_name"), "goods_name");
        assert_eq!(field_ident("3d_model"), "_3d_model");
        // 若按 `_` 后缀生成，Column 变体名将取决于 derive 的裁剪实现。
        assert_eq!(pascal_case(&field_ident("type")), "TypeCol");
    }

    #[test]
    fn identifiers_are_validated() {
        assert!(is_valid_identifier("biz_goods"));
        assert!(!is_valid_identifier("biz goods"));
        assert!(!is_valid_identifier("../etc/passwd"));
        assert!(!is_valid_identifier("1abc"));
        assert!(!is_valid_identifier(""));
    }
}
