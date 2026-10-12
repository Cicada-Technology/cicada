use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

/// `sys_oper_log` 补上审计设计点名却没落地的三项：**UA**、**业务码**、**脱敏后的请求体**。
///
/// 此前这张表只记 URL + query：`method` 是「POST /path」这种字符串，请求体从来没进过库，
/// 于是「谁把哪个字段改成了什么」在审计里看不见——而审计设计要审计能回答的恰恰是这个问题。
/// 请求体由 `audit_middleware` 缓冲一层：先读、脱敏、再原样交给 handler，
/// 所以列里存的是**脱敏后**的 JSON（`password`/`token`/`secret` 一类只留键名不留值）。
///
/// 三列都可空：升级前入队的事件、以及不采集体的请求（multipart 上传、超过 8KB 的体）
/// 本来就没有这些值，用 NOT NULL 等于逼写入端造假数据。
#[derive(DeriveMigrationName)]
pub struct Migration;

/// 列长度上限（写入端 `clip` 用的是同一组数字，两边各写一个数迟早漂移）。
pub const USER_AGENT_LEN: usize = 512;
pub const REQ_BODY_LEN: usize = 4096;
pub const BIZ_CODE_LEN: usize = 40;

/// 加列与建索引都是 `IF NOT EXISTS`：迁移本身按序只跑一次，但 `up → down → up`
/// 的往返测试要求同一份 DDL 可以重放。
const STATEMENTS: &[&str] = &[
    "ALTER TABLE sys_oper_log ADD COLUMN IF NOT EXISTS user_agent varchar(512)",
    "ALTER TABLE sys_oper_log ADD COLUMN IF NOT EXISTS biz_code varchar(40)",
    "ALTER TABLE sys_oper_log ADD COLUMN IF NOT EXISTS req_body varchar(4096)",
    // 「这段时间哪些接口在报什么错」是审计最常见的过滤。sys_oper_log 按月分区，
    // 索引建在父表上由 PG 自动下推到各分区。
    "CREATE INDEX IF NOT EXISTS idx_oper_log_biz_code \
        ON sys_oper_log (biz_code) WHERE biz_code IS NOT NULL",
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        for sql in STATEMENTS {
            conn.execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("DROP INDEX IF EXISTS idx_oper_log_biz_code")
            .await?;
        for column in ["req_body", "biz_code", "user_agent"] {
            conn.execute_unprepared(&format!(
                "ALTER TABLE sys_oper_log DROP COLUMN IF EXISTS {column}"
            ))
            .await?;
        }
        Ok(())
    }
}
