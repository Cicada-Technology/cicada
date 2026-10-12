//! 日志保留期与归档：表按月分区，保留期可配（默认 180 天），到期归档到对象
//! 存储而不是直接删除；部署侧由自动建分区任务延伸窗口。
//!
//! 三件事：
//! 1. `ensure_partitions`：把分区窗口往前后延伸（幂等，可每月重复跑）。
//! 2. `expired_partitions`：列出**整月**都早于保留期的分区——半个月还在保留期内的月不删，
//!    免得把「默认 180 天」变成「最多 150 天」。
//! 3. `run`：先把分区导成 NDJSON 放进归档存储，成功后才 `DROP PARTITION`；
//!    归档写失败就保留分区，宁可多留一天也不能把证据弄丢。
//!
//! 表名/分区名只能来自白名单与严格解析：DDL 里的标识符不能用绑定参数，所以必须先校验再拼接。

use chrono::{Datelike, NaiveDate, Utc};
use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement};

use cicada_infra::Db;
use cicada_ports::{ArchiveError, ArchiveStore};

/// 保留期处理的失败面。
#[derive(Debug, thiserror::Error)]
pub enum RetentionError {
    #[error("数据库错误：{0}")]
    Db(#[from] DbErr),
    #[error("归档存储错误：{0}")]
    Archive(#[from] ArchiveError),
    #[error("配置或标识符不合法：{0}")]
    Config(String),
}

/// 参与分区生命周期管理的日志表：`(表名, 时间列)`。
pub const PARTITIONED_TABLES: &[(&str, &str)] = &[
    ("sys_oper_log", "oper_time"),
    ("sys_logininfor", "login_time"),
    ("sys_job_log", "start_time"),
    ("sys_audit_log", "oper_time"),
];

/// 单批导出行数：一次 `string_agg` 拉全月可能到几百 MB，分批拼。
const DUMP_BATCH_ROWS: i64 = 2_000;

/// 一个到期分区的判定结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpiredPartition {
    pub table: String,
    pub name: String,
    /// 该月首日（UTC）。
    pub month: NaiveDate,
    pub rows: i64,
}

/// 一轮保留期处理的产出。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RetentionReport {
    pub archived: Vec<String>,
    pub dropped: Vec<String>,
    pub bytes: u64,
    /// 归档关闭（`audit.archive = false`）时只报告、不删除。
    pub skipped: usize,
}

/// 日志保留期管理器。
#[derive(Clone)]
pub struct LogRetention {
    db: Db,
}

impl LogRetention {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// 延伸分区窗口。返回新建的分区数；重复调用是 0。
    pub async fn ensure_partitions(&self, back: i32, forward: i32) -> Result<u32, DbErr> {
        let mut made = 0u32;
        for (table, time_col) in PARTITIONED_TABLES {
            let stmt = Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT cicada_ensure_monthly_partitions($1, $2, $3, $4)",
                [
                    (*table).into(),
                    (*time_col).into(),
                    back.into(),
                    forward.into(),
                ],
            );
            let rows = self.db.conn().query_all_raw(stmt).await?;
            if let Some(row) = rows.first() {
                made += row.try_get_by_index::<i32>(0).unwrap_or(0) as u32;
            }
        }
        Ok(made)
    }

    /// 列出整月都早于 `now - retention_days` 的分区（按月份升序）。
    pub async fn expired_partitions(
        &self,
        retention_days: i64,
    ) -> Result<Vec<ExpiredPartition>, RetentionError> {
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT parent.relname, child.relname \
             FROM pg_inherits i \
             JOIN pg_class child ON child.oid = i.inhrelid \
             JOIN pg_class parent ON parent.oid = i.inhparent \
             JOIN pg_namespace n ON n.oid = child.relnamespace \
             WHERE n.nspname = current_schema() \
             ORDER BY parent.relname, child.relname",
            [],
        );
        let rows = self.db.conn().query_all_raw(stmt).await?;
        let now = Utc::now().naive_utc();
        let mut out = Vec::new();
        for row in rows {
            let parent: String = row.try_get_by_index(0)?;
            let child: String = row.try_get_by_index(1)?;
            if !is_managed_table(&parent) {
                continue;
            }
            let Some(month) = parse_partition_month(&parent, &child) else {
                // 手工建的、命名不合约定的分区一律不动：宁可留着也不误删。
                tracing::debug!(table = %parent, partition = %child, "分区名不符合 <表>_YYYYMM 约定，跳过");
                continue;
            };
            if !month_is_expired(month, now, retention_days) {
                continue;
            }
            let rows_in_part = count_rows(&self.db, &child).await?;
            if rows_in_part == 0 {
                // 空分区没什么可归档的：留着不碍事，删了反而让 `ensure` 白铺过。
                continue;
            }
            out.push(ExpiredPartition {
                table: parent,
                name: child,
                month,
                rows: rows_in_part,
            });
        }
        Ok(out)
    }

    /// 把一个分区导出成 NDJSON（一行一条 JSON 记录，按时间列升序）。
    pub async fn dump_partition(
        &self,
        table: &str,
        partition: &str,
    ) -> Result<Vec<u8>, RetentionError> {
        let time_col = time_column(table)
            .ok_or_else(|| RetentionError::Config(format!("{table} 不在受管日志表白名单内")))?;
        assert_ident(table)?;
        assert_ident(partition)?;
        let mut body = Vec::new();
        let mut offset = 0i64;
        loop {
            let sql = format!(
                "SELECT COALESCE(string_agg(line, E'\\n' ORDER BY rn), '') FROM (\
                     SELECT row_to_json(t)::text AS line, \
                            row_number() OVER (ORDER BY {time_col}) AS rn \
                     FROM {partition} t \
                     ORDER BY rn LIMIT {DUMP_BATCH_ROWS} OFFSET {offset}\
                 ) b"
            );
            let stmt = Statement::from_string(DatabaseBackend::Postgres, sql);
            let rows = self.db.conn().query_all_raw(stmt).await?;
            let chunk: String = rows
                .first()
                .map(|r| r.try_get_by_index::<String>(0).unwrap_or_default())
                .unwrap_or_default();
            if chunk.is_empty() {
                break;
            }
            if !body.is_empty() {
                body.push(b'\n');
            }
            body.extend_from_slice(chunk.as_bytes());
            let got = chunk.lines().count() as i64;
            offset += got;
            if got < DUMP_BATCH_ROWS {
                break;
            }
        }
        Ok(body)
    }

    /// 归档一个分区（不删）。键名形如 `logs/sys_oper_log/sys_oper_log_202504.ndjson`。
    pub async fn archive_partition(
        &self,
        part: &ExpiredPartition,
        archive: &dyn ArchiveStore,
    ) -> Result<u64, RetentionError> {
        let body = self.dump_partition(&part.table, &part.name).await?;
        let key = format!("logs/{}/{part}.ndjson", part.table, part = part.name);
        archive.put(&key, &body).await?;
        tracing::info!(
            table = %part.table,
            partition = %part.name,
            rows = part.rows,
            bytes = body.len(),
            key = %key,
            "日志分区已归档"
        );
        Ok(body.len() as u64)
    }

    /// 删除一个分区（前提是归档已成功）。
    pub async fn drop_partition(&self, table: &str, partition: &str) -> Result<(), RetentionError> {
        assert_ident(table)?;
        assert_ident(partition)?;
        // PG 的 `DROP PARTITION` 没有 IF EXISTS；DETACH 后再 DROP TABLE 才既能幂等
        // 又保留数据文件的可控删除。
        for sql in [
            format!("ALTER TABLE {table} DETACH PARTITION {partition}"),
            format!("DROP TABLE IF EXISTS {partition}"),
        ] {
            let stmt = Statement::from_string(DatabaseBackend::Postgres, sql);
            self.db.conn().execute_raw(stmt).await?;
        }
        Ok(())
    }

    /// 一轮保留期处理：扩窗 → 找出到期分区 → 归档 → 删除。
    ///
    /// `archive_enabled = false` 时只报告不删除（反对直接删除，关掉归档就等于不动）。
    pub async fn run(
        &self,
        retention_days: i64,
        horizon_months: i32,
        archive_enabled: bool,
        archive: &dyn ArchiveStore,
    ) -> Result<RetentionReport, RetentionError> {
        self.ensure_partitions(horizon_months.max(1) / 2, horizon_months.max(1))
            .await?;
        let expired = self.expired_partitions(retention_days).await?;
        let mut report = RetentionReport::default();
        if !archive_enabled {
            report.skipped = expired.len();
            return Ok(report);
        }
        for part in expired {
            report.bytes += self.archive_partition(&part, archive).await?;
            report.archived.push(part.name.clone());
            self.drop_partition(&part.table, &part.name).await?;
            report.dropped.push(part.name);
        }
        Ok(report)
    }
}

/// 分区月首日：`<表>_YYYYMM` → `NaiveDate`。不匹配就返回 `None`。
pub fn parse_partition_month(table: &str, name: &str) -> Option<NaiveDate> {
    let suffix = name.strip_prefix(&format!("{table}_"))?;
    if suffix.len() != 6 || !suffix.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year: i32 = suffix[..4].parse().ok()?;
    let month: u32 = suffix[4..].parse().ok()?;
    if !(1..=12).contains(&month) {
        return None;
    }
    NaiveDate::from_ymd_opt(year, month, 1)
}

/// 整月过期判定：该月**最后一天**的最后一刻也要早于 `now - retention_days`。
pub fn month_is_expired(month: NaiveDate, now: chrono::NaiveDateTime, retention_days: i64) -> bool {
    let (y, m) = (month.year(), month.month());
    let (next_year, next_month) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    let Some(next_start) = NaiveDate::from_ymd_opt(next_year, next_month, 1) else {
        return false;
    };
    // 下一月首日 00:00 即视为整月结束；再减去 1 纳秒避免边界抖动，直接用 <= 比较更稳。
    next_start.and_hms_opt(0, 0, 0).unwrap_or_default()
        <= now - chrono::Duration::days(retention_days.max(0))
}

fn time_column(table: &str) -> Option<&'static str> {
    PARTITIONED_TABLES
        .iter()
        .find(|(t, _)| *t == table)
        .map(|(_, c)| *c)
}

fn is_managed_table(table: &str) -> bool {
    PARTITIONED_TABLES.iter().any(|(t, _)| *t == table)
}

async fn count_rows(db: &Db, partition: &str) -> Result<i64, RetentionError> {
    assert_ident(partition)?;
    let stmt = Statement::from_string(
        DatabaseBackend::Postgres,
        format!("SELECT count(*) FROM {partition}"),
    );
    let row = db.conn().query_one_raw(stmt).await?;
    Ok(match row {
        Some(r) => r.try_get_by_index::<i64>(0)?,
        None => 0,
    })
}

/// DDL / 表名不能走绑定参数，因此这里做**严格标识符校验**：只允许小写字母、数字与下划线，
/// 且必须以字母或下划线开头（正是 `parse_partition_month` 能产出的形态）。
fn assert_ident(name: &str) -> Result<(), RetentionError> {
    let ok = !name.is_empty()
        && name.len() <= 63
        && name
            .bytes()
            .enumerate()
            .all(|(i, b)| b == b'_' || b.is_ascii_lowercase() || (b.is_ascii_digit() && i > 0));
    if ok {
        Ok(())
    } else {
        Err(RetentionError::Config(format!("非法标识符：{name}")))
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn dt(y: i32, mo: u32, d: u32) -> NaiveDate {
        Utc.with_ymd_and_hms(y, mo, d, 0, 0, 0)
            .unwrap()
            .date_naive()
    }

    #[test]
    fn partition_names_parse_to_month_starts() {
        assert_eq!(
            parse_partition_month("sys_oper_log", "sys_oper_log_202504"),
            Some(dt(2025, 4, 1))
        );
        assert_eq!(
            parse_partition_month("sys_audit_log", "sys_audit_log_202612"),
            Some(dt(2026, 12, 1))
        );
        // 索引、非月份后缀、别的表前缀都不认。
        assert_eq!(
            parse_partition_month("sys_oper_log", "sys_oper_log_202504_tenant_id_idx"),
            None
        );
        assert_eq!(
            parse_partition_month("sys_oper_log", "sys_job_log_202504"),
            None
        );
        assert_eq!(
            parse_partition_month("sys_oper_log", "sys_oper_log_202513"),
            None
        );
    }

    #[test]
    fn only_whole_expired_months_are_selected() {
        // 今天 = 2026-10-17，保留 180 天 → 界限 2026-04-20。
        let now = Utc
            .with_ymd_and_hms(2026, 10, 17, 0, 0, 0)
            .unwrap()
            .naive_utc();
        assert!(month_is_expired(dt(2025, 4, 1), now, 180));
        assert!(!month_is_expired(dt(2026, 4, 1), now, 180)); // 4/30 之后才算整月过期
        assert!(month_is_expired(dt(2026, 3, 1), now, 180));
        // retention 0：只有上个月及更早才算整月过期。
        assert!(month_is_expired(dt(2026, 9, 1), now, 0));
        assert!(!month_is_expired(dt(2026, 10, 1), now, 0));
    }

    #[test]
    fn time_columns_come_from_the_whitelist() {
        assert_eq!(time_column("sys_oper_log"), Some("oper_time"));
        assert_eq!(time_column("sys_job_log"), Some("start_time"));
        assert_eq!(time_column("sys_user"), None);
        assert!(is_managed_table("sys_audit_log"));
        assert!(!is_managed_table("pg_class"));
    }

    #[test]
    fn identifier_validation_blocks_injection() {
        assert!(assert_ident("sys_oper_log_202504").is_ok());
        assert!(assert_ident("a_b1").is_ok());
        for bad in [
            "",
            "1abc",
            "A",
            "a b",
            "a;b",
            "a\"b",
            "a-b",
            "a'b'",
            &"x".repeat(64),
        ] {
            assert!(assert_ident(bad).is_err(), "应拒绝：{bad:?}");
        }
    }
}
