use sea_orm_migration::prelude::*;

/// 模糊检索列的 `pg_trgm` GIN 索引：查询端用 PG 原生 `ILIKE`，高频模糊列
/// 需要索引加速。
///
/// 后台列表的搜索框是前缀通配的 `%kw%`：B-tree 用不上，普通表只能顺序扫描。开启
/// `pg_trgm` 后三元组转成 GIN 键，`ILIKE '%kw%'` 才走索引——租户表越大差别越明显。
///
/// 索引清单不是随手挑的：**只给真正有模糊过滤条件的列建**（`crates/web` 里 25 个
/// `Column::xxx.fuzzy(..)` 落点，即各列表页搜索框打的那几列），因为每个 GIN 索引都是写放大。
/// `migrations/tests/trgm_indexes.rs` 断言：清单里每一项都真的建成了 `gin` + `gin_trgm_ops`
/// 索引，并且规划器对 `ILIKE '%kw%'` 确实用上了它（不是「建了但没人走」）。
///
/// 分区表（`sys_oper_log` / `sys_logininfor`）在父表上建索引即可：PG 会把它传播到所有
/// 分区，之后维护任务新建的分区也自动带上（审计设计）。
#[derive(DeriveMigrationName)]
pub struct Migration;

/// `(表, 列)`：需要 trgm 索引的模糊检索列。
pub const FUZZY_COLUMNS: &[(&str, &str)] = &[
    ("sys_api_key", "name"),
    ("sys_config", "config_key"),
    ("sys_config", "config_name"),
    ("sys_dict_data", "dict_label"),
    ("sys_dict_type", "dict_name"),
    ("sys_dict_type", "dict_type"),
    ("sys_file", "original_name"),
    ("sys_job", "job_name"),
    ("sys_logininfor", "ipaddr"),
    ("sys_logininfor", "user_name"),
    ("sys_notice", "notice_title"),
    ("sys_notify_channel", "code"),
    ("sys_notify_record", "recipient"),
    ("sys_notify_template", "code"),
    ("sys_oper_log", "oper_name"),
    ("sys_oper_log", "title"),
    ("sys_post", "post_code"),
    ("sys_post", "post_name"),
    ("sys_role", "role_key"),
    ("sys_role", "role_name"),
    ("sys_tenant", "tenant_name"),
    ("sys_tenant_package", "code"),
    ("sys_tenant_package", "name"),
    ("sys_user", "user_name"),
    ("sys_user_online", "user_name"),
];

/// 操作符类全限定名（见 `up` 里对 `search_path` 的说明）。
pub const TRGM_OPCLASS: &str = "public.gin_trgm_ops";

/// 索引名规则固定下来，测试与排障都按同一个名字找。
pub fn trgm_index_name(table: &str, column: &str) -> String {
    format!("{table}_{column}_trgm")
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        // pg_trgm 是 trusted extension，库属主级别的权限就能建，不需要 superuser。
        // 扩展是**库级**对象，一个库里只有一份，所以固定装进 `public`。
        conn.execute_unprepared("CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA public")
            .await?;

        for (table, column) in FUZZY_COLUMNS {
            // opclass 写全限定名：`SeaORM` 的连接把 `search_path` 设成配置里那个 schema
            // **本身**（不含 `public`），裸写 `gin_trgm_ops` 在 schema-per-test 的库里解析不到。
            conn.execute_unprepared(&format!(
                "CREATE INDEX IF NOT EXISTS {name} ON {table} USING gin ({column} {opclass})",
                name = trgm_index_name(table, column),
                opclass = TRGM_OPCLASS,
            ))
            .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        for (table, column) in FUZZY_COLUMNS {
            conn.execute_unprepared(&format!(
                "DROP INDEX IF EXISTS {}",
                trgm_index_name(table, column)
            ))
            .await?;
        }
        // 扩展本身不删：它是库级对象，同库里还有别的 schema（测试台一个用例一个 schema）
        // 可能正在用这些索引；删掉会让别人的迁移半途失败。
        Ok(())
    }
}
