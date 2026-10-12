//! 数据库类型 → Rust 类型 / 前端控件 / 查询方式（推断字段类型与注释）。
//!
//! 只认 PostgreSQL（一期不做多方言）。映射以 `udt_name` 为准，
//! 因为它把 `varchar(64)` / `varchar` / `bpchar` 区分开而 `data_type` 会归一化。

use serde::{Deserialize, Serialize};

/// 一次类型推断的结果。`Option` 由 `is_nullable` 决定，不在这里拼字符串。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustType {
    /// Rust 类型字面量，如 `i64` / `DateTimeUtc` / `Vec<u8>`。
    pub base: &'static str,
    /// 需要额外 `use` 的路径（模板里生成 `use ...;`），`None` 表示 prelude 已覆盖。
    pub import: Option<&'static str>,
    /// 是否可空（由 `is_nullable` 覆盖）。
    pub nullable: bool,
}

impl RustType {
    pub fn render(&self) -> String {
        if self.nullable {
            format!("Option<{}>", self.base)
        } else {
            self.base.to_string()
        }
    }
}

/// 按 pg 内部类型名（`udt_name`）推断。未知类型一律回落 `String`，绝不 panic。
pub fn rust_type(udt_name: &str, nullable: bool) -> RustType {
    let (base, import) = match udt_name {
        "int2" | "smallserial" | "serial2" => ("i16", None),
        "int4" | "serial" | "serial4" => ("i32", None),
        "int8" | "bigserial" | "serial8" => ("i64", None),
        "bool" => ("bool", None),
        "float4" => ("f32", None),
        "float8" => ("f64", None),
        "numeric" | "decimal" => ("Decimal", Some("rust_decimal::Decimal")),
        "varchar" | "bpchar" | "char" | "text" | "name" | "citext" | "inet" | "cidr"
        | "macaddr" | "xml" | "tsvector" => ("String", None),
        "timestamp"
        | "timestamptz"
        | "timestamp with time zone"
        | "timestamp without time zone" => ("DateTimeUtc", None),
        // `Date` / `Time` 是 `sea_orm::entity::prelude` 里 `NaiveDate` / `NaiveTime`
        // 的别名，用别名可以让生成的实体文件零额外 `use`。
        "date" => ("Date", None),
        "time" | "timetz" => ("Time", None),
        "json" | "jsonb" => ("Json", None),
        "uuid" => ("Uuid", None),
        "bytea" => ("Vec<u8>", None),
        "interval" => ("String", None),
        _ => ("String", None),
    };
    RustType {
        base,
        import,
        nullable,
    }
}

/// 前端控件类型（`gen_table_column.html_type`）。可在导入后逐个改写。
pub fn html_type(column: &str, udt_name: &str, dict_type: Option<&str>) -> String {
    if dict_type.is_some_and(|d| !d.is_empty()) {
        return "select".to_string();
    }
    let c = column.to_ascii_lowercase();
    if c.ends_with("_image") || c.ends_with("_avatar") || c.ends_with("_icon") {
        return "image".to_string();
    }
    if c.ends_with("_file") || c.ends_with("_attachment") {
        return "file".to_string();
    }
    if c == "content" || c.ends_with("_content") || c.ends_with("_detail") {
        return "editor".to_string();
    }
    match udt_name {
        "text" => "textarea".to_string(),
        "bool" => "switch".to_string(),
        "date" => "date".to_string(),
        "timestamp" | "timestamptz" => "datetime".to_string(),
        "time" | "timetz" => "time".to_string(),
        _ => "input".to_string(),
    }
}

/// 查询方式（`gen_table_column.query_type`），参考实现的 `EQ` / `LIKE` / `BETWEEN` 三档。
pub fn query_type(column: &str, udt_name: &str) -> &'static str {
    let c = column.to_ascii_lowercase();
    if matches!(udt_name, "date" | "timestamp" | "timestamptz") || c.ends_with("_at") {
        return "BETWEEN";
    }
    if c.ends_with("_name") || c.ends_with("_title") || c.ends_with("_code") || c.contains("descr")
    {
        return "LIKE";
    }
    "EQ"
}

/// 已知字典列 → 字典类型。参考实现的 `GenUtils.getDictType` 白名单的 Rust 版。
pub fn dict_type(column: &str, udt_name: &str) -> Option<&'static str> {
    let c = column.to_ascii_lowercase();
    if !matches!(
        udt_name,
        "varchar" | "bpchar" | "char" | "text" | "int2" | "int4" | "int8"
    ) {
        return None;
    }
    Some(match c.as_str() {
        "sex" => "sys_user_sex",
        "del_flag" => "sys_yes_no",
        "status" => "sys_normal_disable",
        "notice_type" => "sys_notice_type",
        "oper_type" => "sys_oper_type",
        "job_group" => "sys_job_group",
        "job_status" => "sys_job_status",
        _ => return None,
    })
}

/// 列是否属于「框架自带列」：生成时强制排除，避免生成器与框架职责重叠。
pub fn is_framework_column(column: &str) -> bool {
    matches!(
        column,
        "tenant_id" | "deleted_at" | "create_by" | "create_time" | "update_by" | "update_time"
    )
}

/// 由列名猜出的软删列。
pub fn is_soft_delete_column(column: &str) -> bool {
    column == "deleted_at"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_hot_types() {
        assert_eq!(rust_type("int8", false).render(), "i64");
        assert_eq!(rust_type("varchar", true).render(), "Option<String>");
        assert_eq!(
            rust_type("timestamptz", true).render(),
            "Option<DateTimeUtc>"
        );
        assert_eq!(rust_type("numeric", true).render(), "Option<Decimal>");
        assert_eq!(rust_type("date", true).render(), "Option<Date>");
        assert_eq!(rust_type("time", false).render(), "Time");
        assert_eq!(rust_type("bytea", false).render(), "Vec<u8>");
        assert_eq!(
            rust_type("numeric", true).import,
            Some("rust_decimal::Decimal")
        );
    }

    #[test]
    fn unknown_types_fall_back_to_string() {
        assert_eq!(rust_type("tsquery", false).render(), "String");
    }

    #[test]
    fn infers_html_and_query_types() {
        assert_eq!(html_type("goods_name", "varchar", None), "input");
        assert_eq!(html_type("remark", "text", None), "textarea");
        assert_eq!(
            html_type("status", "int2", Some("sys_normal_disable")),
            "select"
        );
        assert_eq!(html_type("cover_image", "varchar", None), "image");
        assert_eq!(html_type("enabled", "bool", None), "switch");

        assert_eq!(query_type("goods_name", "varchar"), "LIKE");
        assert_eq!(query_type("status", "int2"), "EQ");
        assert_eq!(query_type("create_time", "timestamptz"), "BETWEEN");
    }

    #[test]
    fn dict_whitelist_only_covers_text_like_columns() {
        assert_eq!(dict_type("status", "int2"), Some("sys_normal_disable"));
        assert_eq!(dict_type("quantity", "int8"), None);
        assert_eq!(dict_type("create_time", "timestamptz"), None);
    }
}
