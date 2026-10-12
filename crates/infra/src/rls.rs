//! RLS 兜底：为敏感表按需开启 PostgreSQL ROW LEVEL SECURITY。
//!
//! 应用层通过 `Db::begin_scoped` 把 `app.tenant_id` 等注入会话；数据库层再按同一
//! 租户条件过滤一次，应用层算错归属条件时数据仍不外泄。**默认关闭，按表开启**：
//! 表名必须来自 [`TENANT_TABLES`] 白名单（identifier 由代码常量给出，不拼接用户输入）。
//!
//! 策略是 fail-closed 的：`current_setting('app.tenant_id', true)` 未设置时返回 NULL，
//! `tenant_id = NULL` 为 NULL，行被过滤——即「没有租户上下文 = 看不到任何行」。

use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement};

/// 策略名（每张表一个同名策略）。
pub const POLICY: &str = "tenant_isolation";

/// 允许开启 RLS 的表：都带 `tenant_id` 列，且读路径本就应经 `Scope` 过滤。
///
/// 不含 `sys_tenant`（租户登记表本身）、`sys_*_role` / `sys_role_menu` 等关联表
/// （无 `tenant_id` 列，靠外键一致性约束，见多租户设计）与全局字典/参数/公告表。
pub const TENANT_TABLES: &[&str] = &[
    "sys_user",
    "sys_user_identity",
    "sys_dept",
    "sys_role",
    "sys_post",
    "sys_oper_log",
    "sys_logininfor",
    "sys_file",
    "sys_job",
    "sys_job_log",
    "gen_table",
    "gen_table_column",
    "sys_api_key",
    "sys_notify_channel",
    "sys_notify_template",
    "sys_notify_record",
];

/// 只允许 `[a-z_][a-z0-9_]*`，且必须命中白名单——identifier 不来自请求。
fn validate(table: &str) -> Result<&str, RlsError> {
    let ok = !table.is_empty()
        && table.len() <= 63
        && table
            .bytes()
            .enumerate()
            .all(|(i, b)| b == b'_' || b.is_ascii_lowercase() || (i > 0 && b.is_ascii_digit()));
    if !ok {
        return Err(RlsError::IllegalIdentifier(table.to_string()));
    }
    if !TENANT_TABLES.contains(&table) {
        return Err(RlsError::NonTenantTable(table.to_string()));
    }
    Ok(table)
}

#[derive(Debug, thiserror::Error)]
pub enum RlsError {
    #[error("非法表标识符：{0}")]
    IllegalIdentifier(String),
    #[error("表不在可开启 RLS 的白名单内（缺少 tenant_id 或未登记）：{0}")]
    NonTenantTable(String),
    #[error("数据库错误：{0}")]
    Db(#[from] DbErr),
}

/// 为一组表开启 RLS：`ENABLE` + `FORCE` + 重建租户隔离策略（幂等）。
///
/// `FORCE` 让表属主也受策略约束；若需要维护窗口绕过，应使用单独的
/// `BYPASSRLS` 角色而不是关掉 `FORCE`。
pub async fn enable<C: ConnectionTrait>(conn: &C, tables: &[String]) -> Result<(), RlsError> {
    for table in tables {
        let table = validate(table)?;
        for sql in [
            format!("ALTER TABLE \"{table}\" ENABLE ROW LEVEL SECURITY"),
            format!("ALTER TABLE \"{table}\" FORCE ROW LEVEL SECURITY"),
            format!("DROP POLICY IF EXISTS {POLICY} ON \"{table}\""),
            format!(
                "CREATE POLICY {POLICY} ON \"{table}\" \
                 USING (tenant_id = current_setting('app.tenant_id', true)::bigint) \
                 WITH CHECK (tenant_id = current_setting('app.tenant_id', true)::bigint)"
            ),
        ] {
            conn.execute_raw(Statement::from_string(DatabaseBackend::Postgres, sql))
                .await?;
        }
    }
    Ok(())
}

/// 关闭一组表的 RLS（删策略 + `DISABLE`），幂等。
pub async fn disable<C: ConnectionTrait>(conn: &C, tables: &[String]) -> Result<(), RlsError> {
    for table in tables {
        let table = validate(table)?;
        for sql in [
            format!("DROP POLICY IF EXISTS {POLICY} ON \"{table}\""),
            format!("ALTER TABLE \"{table}\" NO FORCE ROW LEVEL SECURITY"),
            format!("ALTER TABLE \"{table}\" DISABLE ROW LEVEL SECURITY"),
        ] {
            conn.execute_raw(Statement::from_string(DatabaseBackend::Postgres, sql))
                .await?;
        }
    }
    Ok(())
}

/// 当前 schema 下已开启 RLS 的表（来自 `pg_class.relrowsecurity`）。
pub async fn enabled_tables<C: ConnectionTrait>(conn: &C) -> Result<Vec<String>, RlsError> {
    let rows = conn
        .query_all_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT c.relname FROM pg_class c \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = current_schema() AND c.relkind = 'r' AND c.relrowsecurity \
             ORDER BY c.relname",
        ))
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| r.try_get_by_index::<String>(0).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_identifiers_outside_the_whitelist() {
        assert!(validate("sys_user").is_ok());
        // 白名单内但写法非法（大写/引号/路径）一律拒绝。
        assert!(validate("sys_user; drop table x").is_err());
        assert!(validate("Sys_User").is_err());
        assert!(validate("1_user").is_err());
        assert!(validate("").is_err());
        // 合法标识符但不在白名单：不许开启（无 tenant_id 的表会 fail-closed 掉全表）。
        assert!(validate("sys_dict_type").is_err());
        assert!(validate("sys_tenant").is_err());
    }
}
