//! {{project-name}} 的迁移入口。
//!
//! 迁移列表在 `migrations/mod.rs`：框架迁移 + 你自己的表，合成**一个** `Migrator`。
//! 本入口只负责跑它，外加可选的种子与初始管理员。
//!
//! ```text
//! cargo run --bin migrate -- up           # 框架迁移 + 业务迁移
//! cargo run --bin migrate -- up --seed    # 再加字典/参数/菜单种子
//! cargo run --bin migrate -- up --admin   # 再加默认租户的 admin 账号
//! cargo run --bin migrate -- down 1       # 回滚 1 步
//! cargo run --bin migrate -- status       # 待执行迁移
//! ```

use cicada::sea_orm_migration::MigratorTrait;
use cicada::{Config, Db, migrations};

#[path = "../../migrations/mod.rs"]
mod app_migrations;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("up");
    let config = Config::load()?;
    let db = Db::connect(&config.db).await?;

    match cmd {
        "up" => {
            app_migrations::Migrator::up(db.conn(), None).await?;
            if args.iter().any(|a| a == "--seed" || a == "--admin") {
                migrations::seed::seed(db.conn()).await?;
            }
            if args.iter().any(|a| a == "--admin") {
                migrations::init_admin::init_admin(db.conn()).await?;
            }
            println!("迁移完成");
        }
        "down" => {
            let steps = args.get(1).and_then(|s| s.parse::<u32>().ok()).unwrap_or(1);
            app_migrations::Migrator::down(db.conn(), Some(steps)).await?;
            println!("回滚 {steps} 步");
        }
        "status" => {
            app_migrations::Migrator::status(db.conn()).await?;
        }
        other => {
            eprintln!("未知命令：{other}（支持 up / down / status）");
            std::process::exit(2);
        }
    }
    Ok(())
}
