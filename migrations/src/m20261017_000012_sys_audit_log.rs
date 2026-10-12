use sea_orm_migration::prelude::*;

/// `sys_audit_log`：字段级变更留痕。
///
/// 只存变化字段 `{field: [old, new]}`，快照式留痕而不是全量历史。按月范围分区
/// （审计日志表「按月分区，保留期见 7.2」），分区在迁移里按「当前月往前 12 个月、往后 24 个月」
/// 动态生成——写死月份会让迁移随时间失效；后续分区由 `cicada:partitions` 维护任务续上，
/// 过期分区按审计设计归档到对象存储而不是直接删除。
///
/// 属主是 `system`：`audit` 只把 `audit.change_log` 事件写进自己的发件箱，
/// 落地由 relay 投递给 `cicada_system::SysLogStore`（审计设计 / ADR 0008）。
#[derive(DeriveMigrationName)]
pub struct Migration;

/// 分区跨度：往前补 12 个月（参考实现存量留痕可能落在过去），往后铺 24 个月。
/// 再往后的月份由 `cicada:partitions` 维护任务续接（部署「分区表的自动建分区任务」）。
const PARTITIONS_BACK: i32 = 12;
const PARTITIONS_FORWARD: i32 = 24;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();

        conn.execute_unprepared("CREATE SEQUENCE IF NOT EXISTS sys_audit_log_audit_id_seq")
            .await?;

        // 分区表的唯一约束必须包含分区键，因此主键是 (audit_id, oper_time)。
        conn.execute_unprepared(
            "CREATE TABLE sys_audit_log (
                 audit_id   bigint      NOT NULL DEFAULT nextval('sys_audit_log_audit_id_seq'),
                 tenant_id  bigint      NOT NULL,
                 table_name text        NOT NULL,
                 record_id  text        NOT NULL,
                 action     text        NOT NULL CHECK (action IN ('insert', 'update', 'delete')),
                 changed    jsonb       NOT NULL,
                 oper_id    bigint,
                 oper_time  timestamptz NOT NULL DEFAULT now(),
                 trace_id   text,
                 PRIMARY KEY (audit_id, oper_time)
             ) PARTITION BY RANGE (oper_time)",
        )
        .await?;

        conn.execute_unprepared(
            "ALTER SEQUENCE sys_audit_log_audit_id_seq OWNED BY sys_audit_log.audit_id",
        )
        .await?;

        // 父表上的索引会自动传播到每个分区。
        conn.execute_unprepared(
            "CREATE INDEX idx_sys_audit_log_tenant_time
                 ON sys_audit_log (tenant_id, oper_time)",
        )
        .await?;
        conn.execute_unprepared(
            "CREATE INDEX idx_sys_audit_log_entity
                 ON sys_audit_log (table_name, record_id)",
        )
        .await?;

        // 动态生成分区：迁移里不能写死月份。
        let total = PARTITIONS_BACK + PARTITIONS_FORWARD;
        conn.execute_unprepared(&format!(
            "DO $cicada$
             DECLARE
                 base date := (date_trunc('month', now()) - interval '{PARTITIONS_BACK} months')::date;
                 from_month date;
                 to_month date;
             BEGIN
                 FOR i IN 0..({total} - 1) LOOP
                     from_month := (base + (i * interval '1 month'))::date;
                     to_month := (base + ((i + 1) * interval '1 month'))::date;
                     EXECUTE format(
                         'CREATE TABLE IF NOT EXISTS %I PARTITION OF sys_audit_log
                              FOR VALUES FROM (%L) TO (%L)',
                         'sys_audit_log_' || to_char(from_month, 'YYYYMM'),
                         from_month,
                         to_month
                     );
                 END LOOP;
             END
             $cicada$"
        ))
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        // DROP 父表会级联删掉所有分区；序列是独立对象，要单独回收。
        conn.execute_unprepared("DROP TABLE IF EXISTS sys_audit_log CASCADE")
            .await?;
        conn.execute_unprepared("DROP SEQUENCE IF EXISTS sys_audit_log_audit_id_seq")
            .await?;
        Ok(())
    }
}
