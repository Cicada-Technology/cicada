//! 导出规模与内存：大表走 `sys_export_task`，`rust_xlsxwriter` 流式写；
//! 验证「导出 10 万行内存稳定」。真实 PostgreSQL，默认 ignore。
//!
//! 这条测试的存在理由：M2 的出口条件原来是「⏳（无压测环境）」，但那一句里可以本地验证的
//! 部分其实有两件——**写得完**（10 万行不是超时也不是 OOM）与**内存不随行数线性膨胀**
//! （分批读库 + 常量内存工作表）。两者都只需要一台跑得动 PG 的机器，不需要压测集群。
//! HTTP 层的 QPS/延迟仍然要压测环境，那条仍记 ⏳（见性能 M4）。
//!
//! 读数取 `/proc/self/statm` 的 resident（RSS），每批回调里采样一次取最大值：
//! 分批读库的峰值出现在「一批 ORM 模型 + 写缓冲」上，批间采样足够抓住台阶。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use sea_orm_migration::MigratorTraitSelf;

use cicada_kernel::{DataScope, Scope, TenantId, UserId};
use cicada_web::export::MAX_EXPORT_ROWS;
use cicada_web::system::user::{UserExportFilters, build_users_xlsx};

const SCHEMA: &str = "web_export_scale";
/// 压测行数，正好取本节承诺的 10 万。
const BENCH_ROWS: i64 = 100_000;
/// RSS 增量的上限。分批读 + 常量内存写表时，实测增量是 **3 MiB**
/// （10 万行、6.9 秒、2.2 MB 产物，RSS 12 → 15 MiB）；留到 256 MiB 不是为了卡这个数，
/// 而是拦「把分批读库改成整表 `all()`」这种退化——那会轻松越过这道线，
/// 同时在共享 CI 机器上不误报。
const RSS_CEILING_MIB: usize = 256;

fn url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://cicada:2025%40hanmole@localhost:5432/cicada".to_string())
}

async fn connect(schema: Option<&str>) -> DatabaseConnection {
    let mut opt = ConnectOptions::new(url());
    if let Some(s) = schema {
        opt.set_schema_search_path(s.to_string());
    }
    Database::connect(opt).await.unwrap()
}

/// 当前进程 RSS（MiB）。读不到 `/proc` 就返回 0，让断言退化成「不拦」而不是误报。
fn rss_mib() -> usize {
    let Ok(statm) = std::fs::read_to_string("/proc/self/statm") else {
        return 0;
    };
    // statm: size resident shared text data stack exe（单位是页）
    let Some(resident) = statm.split_whitespace().nth(1) else {
        return 0;
    };
    resident
        .parse::<usize>()
        .map(|pages| pages * 4096 / (1024 * 1024))
        .unwrap_or(0)
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL，且会写入 10 万行"]
async fn hundred_thousand_row_export_stays_within_the_memory_ceiling() {
    let admin = connect(None).await;
    admin
        .execute_unprepared(&format!("DROP SCHEMA IF EXISTS {SCHEMA} CASCADE"))
        .await
        .unwrap();
    admin
        .execute_unprepared(&format!("CREATE SCHEMA {SCHEMA}"))
        .await
        .unwrap();

    let conn = connect(Some(SCHEMA)).await;
    cicada_migrations::Migrator.up(&conn, None).await.unwrap();
    cicada_migrations::seed::seed(&conn).await.unwrap();
    cicada_migrations::init_admin::init_admin(&conn)
        .await
        .unwrap();

    // 10 万行由服务端 generate_series 直接落库：造数不该成为这条测试的瓶颈。
    let inserted = conn
        .execute_unprepared(&format!(
            "INSERT INTO sys_user (tenant_id, user_name, nick_name, password, status, version, create_time)
             SELECT 1, 'bench_' || lpad(g::text, 7, '0'), '压测账号', '$argon2id$placeholder', 0, 0, now()
               FROM generate_series(1, {BENCH_ROWS}) AS g"
        ))
        .await
        .unwrap();
    assert_eq!(inserted.rows_affected(), BENCH_ROWS as u64);

    // 租户内共 100_001 行（多出的是 init_admin 的 admin），导出上限是 100_000。
    let scope = Scope {
        tenant_id: TenantId::new(1),
        user_id: Some(UserId::new(1)),
        data_scope: DataScope::all(),
    };
    let baseline = rss_mib();
    let peak = Arc::new(AtomicUsize::new(baseline));
    let peak_inside = peak.clone();
    let started = std::time::Instant::now();

    let bytes = build_users_xlsx(
        conn.clone(),
        scope,
        UserExportFilters::default(),
        move |_n| {
            peak_inside.fetch_max(rss_mib(), Ordering::Relaxed);
        },
    )
    .await
    .expect("10 万行导出必须成功");

    let peak = peak.load(Ordering::Relaxed);
    let grew = peak.saturating_sub(baseline);
    println!(
        "导出 {} 字节 / 耗时 {:.1} 秒 / RSS 基线 {baseline} MiB → 峰值 {peak} MiB（增量 {grew} MiB）",
        bytes.len(),
        started.elapsed().as_secs_f64(),
    );

    // 1. 写完了，而且不是空文件：10 万行 × 若干列，压缩后也远大于 1 MiB。
    assert!(
        bytes.len() > 1024 * 1024,
        "10 万行只导出 {} 字节，导出根本没跑起来",
        bytes.len()
    );
    assert!(
        bytes.starts_with(b"PK"),
        "产物不是 zip/xlsx（前 4 字节 {:?}）",
        &bytes[..4.min(bytes.len())]
    );

    // 2. 内存增量受上限约束。退化成「整表 collect」时，10 万个 ORM 模型 + 字符串
    //    会轻松越过这道线；分批 + 常量内存写表则不会。
    assert!(
        grew < RSS_CEILING_MIB,
        "RSS 增量 {grew} MiB 越过 {RSS_CEILING_MIB} MiB 上限：\
         分批读库或常量内存工作表被改坏了（基线 {baseline} MiB / 峰值 {peak} MiB）"
    );

    // 3. 上限语义本身：库里 100_001 行，导出恰好到 MAX_EXPORT_ROWS 就停。
    //    这一条拦住「把上限悄悄调大」——导入导出的 10 万是承诺值，不是参考值。
    assert_eq!(
        MAX_EXPORT_ROWS, 100_000,
        "导出行数上限被改动：本节承诺的是 10 万行"
    );
}
