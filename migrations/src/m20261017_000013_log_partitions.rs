use sea_orm_migration::prelude::*;

/// 日志表按月分区：保留期可配，分区清单与自动建分区任务见下。
///
/// `sys_oper_log`、`sys_logininfor`、`sys_job_log` 建表时是普通表，而本节要求日志表按月分区：
/// 否则「保留期 + 归档」只能逐行 DELETE，表越大越慢，还跟业务写抢同一张表的锁。
///
/// 两个实现约束值得记一笔：
/// - SeaORM 的 `auto_increment()` 在 PG 上生成的是 **identity 列**，它的内部序列不允许提前
///   `OWNED BY NONE`（PG 报 `0A000 cannot change ownership of identity sequence`）。而
///   `CREATE TABLE ... (LIKE ... INCLUDING DEFAULTS)` 又不复制 identity，于是改造后的主键列
///   变成「普通 bigint + 显式序列」——正是参考实现的 serial 形态，SeaORM 插入路径不变
///   （它不送主键，走 default）。旧表连它的 identity 序列一起 DROP，随后按同名建序列。
/// - 分区键必须进主键，所以主键变成 `(id, 时间列)`。
///
/// 同时创建常驻函数 `cicada_ensure_monthly_partitions`：迁移用它的初始窗口，
/// 运行期的维护任务每月继续调它扩窗（部署「分区表的自动建分区任务」）。
#[derive(DeriveMigrationName)]
pub struct Migration;

/// 往前补的月份数：覆盖存量数据，也覆盖 `retention_days` 默认 180 天的窗口。
const BACK_MONTHS: i32 = 12;
/// 往后预铺的月份数：即使维护任务停跑一阵，写入也不会因为没有分区而失败。
const FORWARD_MONTHS: i32 = 24;

/// (表名, 主键列, 时间列, 需要重建的索引 `(索引名, 列)`)。
struct LogTable {
    table: &'static str,
    pk: &'static str,
    time_col: &'static str,
    indexes: &'static [(&'static str, &'static str)],
}

const LOG_TABLES: &[LogTable] = &[
    LogTable {
        table: "sys_oper_log",
        pk: "oper_id",
        time_col: "oper_time",
        indexes: &[("idx_sys_oper_log_tenant_time", "tenant_id, oper_time")],
    },
    LogTable {
        table: "sys_logininfor",
        pk: "info_id",
        time_col: "login_time",
        indexes: &[("idx_sys_logininfor_tenant_time", "tenant_id, login_time")],
    },
    LogTable {
        table: "sys_job_log",
        pk: "job_log_id",
        time_col: "start_time",
        indexes: &[
            ("idx_sys_job_log_job_time", "job_id, start_time"),
            ("idx_sys_job_log_tenant_time", "tenant_id, start_time"),
        ],
    },
];

/// 改造后的主键是分区表的索引，旧索引名跟着一起没了，所以显式重建。
fn index_ddl(spec: &LogTable) -> String {
    spec.indexes
        .iter()
        .map(|(name, cols)| {
            format!(
                "    EXECUTE 'CREATE INDEX IF NOT EXISTS {name} ON {} ({cols})';",
                spec.table
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 分区扩展函数：迁移与运行期维护任务共用同一份逻辑。
/// 分区扩展函数。给了 `start_month` 就以它为准（迁移搬存量时传旧表的最早一月）；
/// 否则从「本表最早一行所在月」与「当前月往前 `back_months` 个月」里更早的那个起算。
const ENSURE_FN: &str = "CREATE OR REPLACE FUNCTION cicada_ensure_monthly_partitions(
    parent_table text,
    time_column text,
    back_months int,
    forward_months int,
    start_month date DEFAULT NULL
) RETURNS int AS $fn$
DECLARE
    cur  date;
    stop date;
    nsp  text;
    part text;
    made int := 0;
BEGIN
    IF to_regclass(parent_table) IS NULL THEN
        RAISE EXCEPTION '分区父表不存在：%', parent_table;
    END IF;
    -- 分区必须建在父表自己的 schema 里：否则 search_path 上别处的同名表会让
    -- to_regclass 误判「已存在」，这一 schema 反而永远缺分区。
    SELECT n.nspname INTO nsp
        FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE c.oid = to_regclass(parent_table);
    IF start_month IS NOT NULL THEN
        cur := start_month;
    ELSE
        -- 往前也要铺：参考实现存量搬迁与补录都会写几个月前的行（存量数据映射），
        -- 缺分区会让整条 INSERT 失败。空的老分区由保留期任务原样跳过。
        EXECUTE format('SELECT COALESCE(min(%I), now()) FROM %I', time_column, parent_table)
            INTO cur;
        cur := date_trunc('month', LEAST(cur, now() - make_interval(months => back_months)))::date;
    END IF;
    stop := (date_trunc('month', now()) + make_interval(months => forward_months))::date;
    WHILE cur < stop LOOP
        part := parent_table || '_' || to_char(cur, 'YYYYMM');
        IF to_regclass(format('%I.%I', nsp, part)) IS NULL THEN
            EXECUTE format(
                'CREATE TABLE %I PARTITION OF %I FOR VALUES FROM (%L) TO (%L)',
                part, parent_table, cur, (cur + interval '1 month')::date
            );
            made := made + 1;
        END IF;
        cur := (cur + interval '1 month')::date;
    END LOOP;
    RETURN made;
END
$fn$ LANGUAGE plpgsql";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared(ENSURE_FN).await?;

        for spec in LOG_TABLES {
            let seq = format!("{}_{}_seq", spec.table, spec.pk);
            let create_indexes = index_ddl(spec);
            conn.execute_unprepared(&format!(
                "DO $cicada$
                 DECLARE
                     seq text := '{seq}';
                 BEGIN
                     IF (SELECT relkind FROM pg_class WHERE oid = to_regclass('{table}')) = 'p' THEN
                         RETURN;
                     END IF;
                     EXECUTE 'ALTER TABLE {table} RENAME TO _old_{table}';
                     EXECUTE $sql$CREATE TABLE {table} (
                         LIKE _old_{table} INCLUDING DEFAULTS INCLUDING CONSTRAINTS INCLUDING COMMENTS
                     ) PARTITION BY RANGE ({time_col})$sql$;
                     EXECUTE 'ALTER TABLE {table} ADD PRIMARY KEY ({pk}, {time_col})';
                     -- 先铺分区再搬数据：存量的最早一月可能远早于 back_months。
                     PERFORM cicada_ensure_monthly_partitions(
                         '{table}',
                         '{time_col}',
                         {back},
                         {forward},
                         (SELECT date_trunc('month', min({time_col}))::date FROM _old_{table})
                     );
                     EXECUTE 'INSERT INTO {table} SELECT * FROM _old_{table}';
                     -- 旧表连同它的 identity 序列一起消失，随后按同名建普通序列。
                     EXECUTE 'DROP TABLE _old_{table}';
                     EXECUTE 'CREATE SEQUENCE IF NOT EXISTS ' || quote_ident(seq);
                     EXECUTE 'ALTER SEQUENCE ' || quote_ident(seq) || ' OWNED BY {table}.{pk}';
                     EXECUTE 'ALTER TABLE {table} ALTER COLUMN {pk} SET DEFAULT nextval('
                         || quote_literal(seq) || '::regclass)';
                     -- is_called=false 时 nextval 会**重复**给出这个值，所以传 max+1：
                     -- 空表时得到 1（下一行从 1 开始），有数据时下一行是 max+1，不会撞主键。
                     EXECUTE 'SELECT setval(' || quote_literal(seq)
                         || ', COALESCE((SELECT max({pk}) FROM {table}), 0) + 1, false)';
                 {create_indexes}
                 END
                 $cicada$",
                table = spec.table,
                pk = spec.pk,
                time_col = spec.time_col,
                back = BACK_MONTHS,
                forward = FORWARD_MONTHS,
            ))
            .await?;
        }

        // 变更留痕表建表时已按月分区，这里只把窗口对齐到同一跨度。
        conn.execute_unprepared(&format!(
            "SELECT cicada_ensure_monthly_partitions('sys_audit_log', 'oper_time', {back}, {forward})",
            back = BACK_MONTHS,
            forward = FORWARD_MONTHS,
        ))
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        for spec in LOG_TABLES {
            let seq = format!("{}_{}_seq", spec.table, spec.pk);
            let create_indexes = index_ddl(spec);
            conn.execute_unprepared(&format!(
                "DO $cicada$
                 DECLARE
                     seq text := '{seq}';
                 BEGIN
                     IF (SELECT relkind FROM pg_class WHERE oid = to_regclass('{table}')) IS DISTINCT FROM 'p' THEN
                         RETURN;
                     END IF;
                     EXECUTE $sql$CREATE TABLE _plain_{table} (
                         LIKE {table} INCLUDING DEFAULTS INCLUDING CONSTRAINTS INCLUDING COMMENTS
                     )$sql$;
                     EXECUTE 'ALTER TABLE _plain_{table} ADD PRIMARY KEY ({pk})';
                     -- 序列先改挂到新表：否则 DROP 分区父表会把它一起删掉。
                     EXECUTE 'ALTER SEQUENCE ' || quote_ident(seq) || ' OWNED BY _plain_{table}.{pk}';
                     EXECUTE 'INSERT INTO _plain_{table} SELECT * FROM {table}';
                     EXECUTE 'DROP TABLE {table} CASCADE';
                     EXECUTE 'ALTER TABLE _plain_{table} RENAME TO {table}';
                     -- is_called=false 时 nextval 会**重复**给出这个值，所以传 max+1：
                     -- 空表时得到 1（下一行从 1 开始），有数据时下一行是 max+1，不会撞主键。
                     EXECUTE 'SELECT setval(' || quote_literal(seq)
                         || ', COALESCE((SELECT max({pk}) FROM {table}), 0) + 1, false)';
                 {create_indexes}
                 END
                 $cicada$",
                table = spec.table,
                pk = spec.pk,
            ))
            .await?;
        }
        conn.execute_unprepared(
            "DROP FUNCTION IF EXISTS cicada_ensure_monthly_partitions(text, text, int, int, date)",
        )
        .await?;
        Ok(())
    }
}
