//! 库表发现（sea-schema 读取库表结构）：
//! 直接查 `information_schema` / `pg_catalog`，读表注释、列注释、主键与自增。
//!
//! 只用 PostgreSQL。表名一律以**绑定参数**传入，绝不做字符串拼接；
//! 表名本身还要先过 [`crate::naming::is_valid_identifier`] 与拒绝名单。

use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, Value};

use cicada_infra::Db;

use crate::CodegenError;

/// 库里一张可被生成的表的摘要（列表页用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSummary {
    pub table_name: String,
    pub table_comment: String,
    pub column_count: i64,
}

/// 一列的完整物理信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredColumn {
    pub name: String,
    pub comment: String,
    /// `information_schema.columns.udt_name`，如 `varchar` / `int8` / `timestamptz`。
    pub udt_name: String,
    pub data_type: String,
    pub length: Option<i32>,
    pub nullable: bool,
    pub default: Option<String>,
    pub is_pk: bool,
    /// `GENERATED ... AS IDENTITY`，或 `nextval(...)` 默认值（serial）。
    pub is_increment: bool,
    pub ordinal: i32,
}

/// 一张表的完整结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredTable {
    pub table_name: String,
    pub comment: String,
    pub columns: Vec<DiscoveredColumn>,
}

impl DiscoveredTable {
    /// 主键列。复合主键只取第一列（生成器只支持单列主键，多列时明确报错）。
    pub fn primary_key(&self) -> Option<&DiscoveredColumn> {
        self.columns.iter().find(|c| c.is_pk)
    }

    /// 主键列数量；> 1 表示复合主键。
    pub fn pk_count(&self) -> usize {
        self.columns.iter().filter(|c| c.is_pk).count()
    }

    pub fn column(&self, name: &str) -> Option<&DiscoveredColumn> {
        self.columns.iter().find(|c| c.name == name)
    }
}

const LIST_TABLES_SQL: &str = "\
SELECT c.relname AS table_name,
       COALESCE(obj_description(c.oid, 'pg_class'), '') AS table_comment,
       (SELECT count(*) FROM pg_attribute a
         WHERE a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped) AS column_count
  FROM pg_class c
  JOIN pg_namespace n ON n.oid = c.relnamespace
 WHERE n.nspname = current_schema()
   AND c.relkind IN ('r', 'p')
 ORDER BY c.relname";

const DESCRIBE_TABLE_SQL: &str = "\
SELECT c.column_name,
       COALESCE(col_description(t.oid, c.ordinal_position), '') AS column_comment,
       COALESCE(obj_description(t.oid, 'pg_class'), '') AS table_comment,
       c.udt_name,
       c.data_type,
       c.character_maximum_length,
       (c.is_nullable = 'YES') AS nullable,
       c.column_default,
       c.ordinal_position,
       (c.is_identity = 'YES'
        OR (c.column_default IS NOT NULL AND c.column_default LIKE 'nextval(%')) AS is_increment,
       COALESCE(pk.is_pk, false) AS is_pk
  FROM information_schema.columns c
  JOIN pg_namespace n ON n.nspname = c.table_schema
  JOIN pg_class t ON t.relnamespace = n.oid AND t.relname = c.table_name
  LEFT JOIN LATERAL (
        SELECT true AS is_pk
          FROM pg_index i
          JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY (i.indkey)
         WHERE i.indrelid = t.oid AND i.indisprimary AND a.attname = c.column_name
         LIMIT 1
      ) pk ON true
 WHERE c.table_schema = current_schema() AND c.table_name = $1
 ORDER BY c.ordinal_position";

/// 当前 schema 下的所有基表。
pub async fn list_tables(db: &Db) -> Result<Vec<TableSummary>, CodegenError> {
    let rows = db
        .conn()
        .query_all_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            LIST_TABLES_SQL,
        ))
        .await
        .map_err(CodegenError::from_db)?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(TableSummary {
            table_name: row
                .try_get("", "table_name")
                .map_err(CodegenError::from_db)?,
            table_comment: row
                .try_get("", "table_comment")
                .map_err(CodegenError::from_db)?,
            column_count: row
                .try_get("", "column_count")
                .map_err(CodegenError::from_db)?,
        });
    }
    Ok(out)
}

/// 读一张表的完整结构。表名非法或不存在返回错误。
pub async fn describe_table(db: &Db, table: &str) -> Result<DiscoveredTable, CodegenError> {
    if !crate::naming::is_valid_identifier(table) {
        return Err(CodegenError::Rejected(format!("非法表名：{table}")));
    }
    let rows = db
        .conn()
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            DESCRIBE_TABLE_SQL,
            [Value::String(Some(table.to_string()))],
        ))
        .await
        .map_err(CodegenError::from_db)?;
    if rows.is_empty() {
        return Err(CodegenError::NotFound(format!("表不存在或无列：{table}")));
    }
    let mut columns = Vec::with_capacity(rows.len());
    let mut table_comment = String::new();
    for row in rows {
        table_comment = row
            .try_get("", "table_comment")
            .map_err(CodegenError::from_db)?;
        columns.push(DiscoveredColumn {
            name: row
                .try_get("", "column_name")
                .map_err(CodegenError::from_db)?,
            comment: row
                .try_get("", "column_comment")
                .map_err(CodegenError::from_db)?,
            udt_name: row.try_get("", "udt_name").map_err(CodegenError::from_db)?,
            data_type: row
                .try_get("", "data_type")
                .map_err(CodegenError::from_db)?,
            length: row
                .try_get("", "character_maximum_length")
                .map_err(CodegenError::from_db)?,
            nullable: row.try_get("", "nullable").map_err(CodegenError::from_db)?,
            default: row
                .try_get("", "column_default")
                .map_err(CodegenError::from_db)?,
            is_pk: row.try_get("", "is_pk").map_err(CodegenError::from_db)?,
            is_increment: row
                .try_get("", "is_increment")
                .map_err(CodegenError::from_db)?,
            ordinal: row
                .try_get("", "ordinal_position")
                .map_err(CodegenError::from_db)?,
        });
    }
    Ok(DiscoveredTable {
        table_name: table.to_string(),
        comment: table_comment,
        columns,
    })
}
