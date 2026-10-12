//! 真实 PostgreSQL 集成冒烟（默认 ignore，需 DATABASE_URL）。

use std::io::Write;
use std::sync::{Arc, Mutex};

use cicada_infra::config::{DbConfig, Secret};
use cicada_infra::db::Db;
use sea_orm::{ConnectionTrait, Statement, Value};
use tracing_subscriber::prelude::*;

fn url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://cicada:2025%40hanmole@localhost:5432/cicada".to_string())
}

fn cfg(slow_query_ms: u64) -> DbConfig {
    DbConfig {
        url: Secret::new(url()),
        max_connections: 5,
        min_connections: 0,
        acquire_timeout_secs: 5,
        schema: "public".to_string(),
        slow_query_ms,
    }
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn db_connects_and_pings() {
    let db = Db::connect(&cfg(200)).await.unwrap();
    db.ping().await.unwrap();
}

/// 数据层约定「关库前 `close().await` 等待连接归还」：关掉之后这个句柄就再也拿不到连接了。
#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn close_releases_the_pool() {
    let db = Db::connect(&cfg(200)).await.unwrap();
    db.ping().await.unwrap();
    db.close().await.unwrap();
    assert!(
        db.ping().await.is_err(),
        "池已关闭，ping 必须失败而不是悄悄重建连接"
    );
}

/// 一条 warn 日志的抓取端：写进共享缓冲区，测试里直接读文本。
#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Buffer {
    type Writer = Buffer;
    fn make_writer(&'a self) -> Buffer {
        self.clone()
    }
}

/// 数据层约定慢查询那一档的完整语义：**超阈值才有 warn、带 SQL 摘要与耗时、不带参数值**。
#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn slow_queries_are_logged_without_their_parameters() {
    let buf = Buffer::default();
    let subscriber = tracing_subscriber::Registry::default().with(
        tracing_subscriber::fmt::layer()
            .with_writer(buf.clone())
            .with_target(true)
            .with_level(true),
    );
    let _guard = tracing::subscriber::set_default(subscriber);

    let db = Db::connect(&cfg(1)).await.unwrap();
    // 阈值压到 1ms，这条必然「慢」。参数里放一个只可能出现在参数位置的字面量，
    // 它出现在日志里就等于数据层约定的「不记录参数值」被违背。
    let secret = "SENTINEL-PARAM-VALUE";
    let stmt = Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT pg_sleep(0.05), $1::text",
        [Value::from(secret)],
    );
    db.conn().query_one_raw(stmt).await.unwrap();

    let text = String::from_utf8_lossy(&buf.0.lock().unwrap()).to_string();
    assert!(
        text.contains("slow") && text.to_lowercase().contains("warn"),
        "超阈值应当落一条 warn，实际日志：{text:?}"
    );
    assert!(
        text.contains("pg_sleep"),
        "慢查询日志要带 SQL 摘要，否则无法定位：{text:?}"
    );
    assert!(
        !text.contains(secret),
        "参数值不许进日志（数据层约定）：{text:?}"
    );
    db.close().await.unwrap();
}

/// 阈值之内（且常规语句级别为 Off）不该有任何输出：数据层约定「由 tracing 统一接管」的另一半。
#[tokio::test]
#[ignore = "需要真实 PostgreSQL"]
async fn ordinary_queries_stay_silent() {
    let buf = Buffer::default();
    let subscriber = tracing_subscriber::Registry::default()
        .with(tracing_subscriber::fmt::layer().with_writer(buf.clone()));
    let _guard = tracing::subscriber::set_default(subscriber);

    let db = Db::connect(&cfg(60_000)).await.unwrap();
    db.ping().await.unwrap();
    let stmt = Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT $1::text",
        [Value::from("plain")],
    );
    db.conn().query_one_raw(stmt).await.unwrap();

    let text = String::from_utf8_lossy(&buf.0.lock().unwrap()).to_string();
    assert!(text.is_empty(), "常规语句一个字都不该打，实际：{text:?}");
    db.close().await.unwrap();
}
