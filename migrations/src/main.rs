//! 开发期迁移执行器（M0 占位）：`cargo run -p cicada-migrations`。
//! 正式入口后续并入 `apps/cli`（migrate / seed / init-admin）。

use sea_orm_migration::MigratorTraitSelf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL 未设置");
    let db = sea_orm_migration::sea_orm::Database::connect(url).await?;
    let cmd = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "migrate".to_string());
    match cmd.as_str() {
        "migrate" => cicada_migrations::Migrator.up(&db, None).await?,
        "seed" => cicada_migrations::seed::seed(&db).await?,
        "init-admin" => cicada_migrations::init_admin::init_admin(&db).await?,
        "all" => {
            cicada_migrations::Migrator.up(&db, None).await?;
            cicada_migrations::seed::seed(&db).await?;
            cicada_migrations::init_admin::init_admin(&db).await?;
        }
        other => {
            eprintln!("未知命令：{other}（可用：migrate | seed | init-admin | all）");
            std::process::exit(2);
        }
    }
    println!("完成");
    Ok(())
}
