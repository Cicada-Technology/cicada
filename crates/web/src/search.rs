//! 模糊检索的统一出口：PG 原生 `ILIKE`，高频模糊列建 `pg_trgm` GIN 索引。
//!
//! 为什么不用 SeaORM 的 `contains`：它生成的是 `LIKE`，两件事不对——
//! 1. 大小写敏感：搜 `admin` 命中不了 `Admin`，而后台搜索框的直觉是「不区分大小写」；
//! 2. 不转义通配符：用户在搜索框里输入 `50%`，实际发出的模式是 `%50%%`，
//!    意思变成「以 50 结尾或后面还有东西」，结果集与输入字面量无关，还顺手放大了扫描量。
//!
//! 于是这里给列加一个 `fuzzy()`：`ILIKE` + 把 `%` `_` `\\` 当字面量。
//! 配套的 trgm GIN 索引在 `migrations/src/m20261017_000015_trgm_indexes.rs`，
//! `pg_trgm` 对 `LIKE` 与 `ILIKE` 都建键，所以大小写不敏感不影响走索引。

use sea_orm::ColumnTrait;
use sea_orm::sea_query::Expr;

/// LIKE 模式里的三个特殊字符：`\` 是转义符本身，`%` 与 `_` 是通配符。
/// 顺序不能反——先转义 `\` 再转义通配符，否则会把刚加的反斜杠再转义一遍。
pub fn escape_like(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// 列上的模糊检索。`fuzzy("50%")` 生成 `ILIKE '%50\%%'`：只匹配字面量里含 `50%` 的行。
pub trait Fuzzy: ColumnTrait {
    fn fuzzy<S: Into<String>>(&self, kw: S) -> Expr {
        self.ilike(format!("%{}%", escape_like(&kw.into())))
    }
}

impl<C: ColumnTrait> Fuzzy for C {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards_and_escape_char_become_literals() {
        assert_eq!(escape_like("50%"), r"50\%");
        assert_eq!(escape_like(r"a\b"), r"a\\b");
        assert_eq!(escape_like("a_b"), r"a\_b");
        // 反斜杠不能被二次转义：输入 `\%` 应得到 `\\\%`（两个字面量字符）。
        assert_eq!(escape_like(r"\%"), r"\\\%");
        assert_eq!(escape_like("普通关键词"), "普通关键词");
    }
}
