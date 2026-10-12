//! cicada-cli：迁移 / 种子 / 初始化 / 运维命令。
//!
//! ```text
//! cicada-cli migrate [up|down|status]      结构迁移（默认 up）
//! cicada-cli seed                          字典 / 参数 / 菜单种子（幂等）
//! cicada-cli init-admin                    默认租户 + admin 账号
//! cicada-cli tenant init --name <名> [--admin <账号>] [--password <密码>]
//! cicada-cli admin reset --user <账号> [--tenant <id>] --password <新密码>
//! cicada-cli rls <enable|disable|status> [--table <表名>]...
//! cicada-cli gen --table <表名> [--tree] [--out <文件.zip>]   （默认输出 <表名>.zip）
//! cicada new <目录> [--name <包名>] [--cicada <TOML 片段>]   生成使用者工程
//! ```
//!
//! 连接串取自 `config/default.yaml`（或 `DATABASE_URL`）。参数解析刻意保持朴素：
//! 命令少、无嵌套子命令库依赖，出错时直接打印用法。

use std::process::ExitCode;

mod new;

use cicada_infra::Config;
use cicada_infra::rls;
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use sea_orm_migration::MigratorTraitSelf;

const USAGE: &str = "\
cicada-cli 命令：
  migrate [up|down|status]          结构迁移（默认 up）
  seed                              字典 / 参数 / 菜单种子（幂等）
  init-admin                        默认租户 + admin 账号
  tenant init --name <名> [--admin <账号>] [--password <密码>]
  admin reset --user <账号> [--tenant <id>] --password <新密码>
  rls <enable|disable|status> [--table <表名>]...
  gen --table <表名> [--tree] [--out <文件.zip>]
  new <目录> [--name <包名>] [--cicada <TOML 片段>] [--template <模板目录>]
";

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::Usage(msg)) => {
            eprintln!("{msg}\n\n{USAGE}");
            ExitCode::from(2)
        }
        Err(CliError::Failed(msg)) => {
            eprintln!("失败：{msg}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug)]
enum CliError {
    Usage(String),
    Failed(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::Usage(m) | CliError::Failed(m) => f.write_str(m),
        }
    }
}

fn fail(e: impl std::fmt::Display) -> CliError {
    CliError::Failed(e.to_string())
}

/// 取 `--flag value` 形式的参数（支持 `--flag=value`）。
fn flag(args: &[String], name: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if let Some(v) = a.strip_prefix(&format!("{name}=")) {
            return Some(v.to_string());
        }
        if a == name {
            return it.next().cloned();
        }
    }
    None
}

/// 取所有出现的 `--flag value`（可重复）。
fn flags(args: &[String], name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if let Some(v) = args[i].strip_prefix(&format!("{name}=")) {
            out.push(v.to_string());
        } else if args[i] == name
            && let Some(v) = args.get(i + 1)
        {
            out.push(v.clone());
            i += 1;
        }
        i += 1;
    }
    out
}

async fn connect() -> Result<DatabaseConnection, CliError> {
    let config = Config::load().map_err(fail)?;
    let mut opt = ConnectOptions::new(config.db.url.expose_secret().to_owned());
    opt.set_schema_search_path(config.db.schema.clone())
        .sqlx_logging(false);
    Database::connect(opt).await.map_err(fail)
}

async fn run() -> Result<(), CliError> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some((command, rest)) = argv.split_first() else {
        return Err(CliError::Usage("缺少命令".to_string()));
    };

    match command.as_str() {
        "migrate" => {
            let db = connect().await?;
            match rest.first().map(String::as_str).unwrap_or("up") {
                "up" => {
                    cicada_migrations::Migrator
                        .up(&db, None)
                        .await
                        .map_err(fail)?;
                    println!("结构迁移完成");
                }
                "down" => {
                    cicada_migrations::Migrator
                        .down(&db, None)
                        .await
                        .map_err(fail)?;
                    println!("结构回滚完成");
                }
                "status" => {
                    let pending = cicada_migrations::Migrator
                        .get_pending_migrations(&db)
                        .await
                        .map_err(fail)?;
                    if pending.is_empty() {
                        println!("无待执行迁移");
                    } else {
                        let names: Vec<&str> = pending.iter().map(|m| m.name()).collect();
                        println!("待执行迁移 {} 个：{}", names.len(), names.join(", "));
                    }
                }
                other => return Err(CliError::Usage(format!("未知 migrate 动作：{other}"))),
            }
        }
        "seed" => {
            let db = connect().await?;
            cicada_migrations::seed::seed(&db).await.map_err(fail)?;
            println!("种子写入完成");
        }
        "init-admin" => {
            let db = connect().await?;
            cicada_migrations::init_admin::init_admin(&db)
                .await
                .map_err(fail)?;
            println!(
                "默认租户与管理员就绪（账号 {}）",
                cicada_migrations::init_admin::ADMIN_USERNAME
            );
        }
        "tenant" => {
            if rest.first().map(String::as_str) != Some("init") {
                return Err(CliError::Usage("用法：tenant init --name <名>".to_string()));
            }
            let name = flag(rest, "--name")
                .ok_or_else(|| CliError::Usage("tenant init 缺少 --name".to_string()))?;
            let admin = flag(rest, "--admin").unwrap_or_else(|| "admin".to_string());
            let password = flag(rest, "--password")
                .ok_or_else(|| CliError::Usage("tenant init 缺少 --password".to_string()))?;
            let db = connect().await?;
            let (tenant_id, user_id) =
                cicada_migrations::init_admin::init_tenant_admin(&db, &name, &admin, &password)
                    .await
                    .map_err(fail)?;
            println!("租户 {tenant_id}（{name}）已创建，管理员 {admin}（user_id={user_id}）");
        }
        "admin" => {
            if rest.first().map(String::as_str) != Some("reset") {
                return Err(CliError::Usage(
                    "用法：admin reset --user <账号> --password <新密码>".to_string(),
                ));
            }
            let user = flag(rest, "--user")
                .ok_or_else(|| CliError::Usage("admin reset 缺少 --user".to_string()))?;
            let password = flag(rest, "--password")
                .ok_or_else(|| CliError::Usage("admin reset 缺少 --password".to_string()))?;
            let tenant: i64 = flag(rest, "--tenant")
                .map(|v| {
                    v.parse()
                        .map_err(|_| CliError::Usage("--tenant 需为数字".to_string()))
                })
                .transpose()?
                .unwrap_or(cicada_migrations::init_admin::DEFAULT_TENANT_ID);
            let db = connect().await?;
            let updated =
                cicada_migrations::init_admin::reset_password(&db, tenant, &user, &password)
                    .await
                    .map_err(fail)?;
            if !updated {
                return Err(CliError::Failed(format!(
                    "租户 {tenant} 下没有可用账号 {user}"
                )));
            }
            println!("已重置 {user} 的密码");
        }
        "rls" => {
            let action = rest
                .first()
                .map(String::as_str)
                .ok_or_else(|| CliError::Usage("rls 需要 enable / disable / status".to_string()))?;
            let config = Config::load().map_err(fail)?;
            let mut opt = ConnectOptions::new(config.db.url.expose_secret().to_owned());
            opt.set_schema_search_path(config.db.schema.clone())
                .sqlx_logging(false);
            let db = Database::connect(opt).await.map_err(fail)?;
            // 未显式给 --table 时用配置里的表（enable/disable）。
            let tables: Vec<String> = if flags(rest, "--table").is_empty() {
                config.rls.tables.clone()
            } else {
                flags(rest, "--table")
            };
            match action {
                "status" => {
                    let enabled = rls::enabled_tables(&db).await.map_err(fail)?;
                    if enabled.is_empty() {
                        println!("当前 schema 未开启 RLS");
                    } else {
                        println!("已开启 RLS 的表：{}", enabled.join(", "));
                    }
                }
                "enable" => {
                    if tables.is_empty() {
                        return Err(CliError::Usage(
                            "rls enable 需要 --table，或在配置 rls.tables 中声明".to_string(),
                        ));
                    }
                    rls::enable(&db, &tables).await.map_err(fail)?;
                    println!("已开启 RLS：{}", tables.join(", "));
                }
                "disable" => {
                    if tables.is_empty() {
                        return Err(CliError::Usage(
                            "rls disable 需要 --table，或在配置 rls.tables 中声明".to_string(),
                        ));
                    }
                    rls::disable(&db, &tables).await.map_err(fail)?;
                    println!("已关闭 RLS：{}", tables.join(", "));
                }
                other => return Err(CliError::Usage(format!("未知 rls 动作：{other}"))),
            }
        }
        "gen" => {
            let table = flag(rest, "--table")
                .ok_or_else(|| CliError::Usage("gen 缺少 --table".to_string()))?;
            let tree = rest.iter().any(|a| a == "--tree");
            let out = flag(rest, "--out").unwrap_or_else(|| format!("{table}.zip"));
            let config = Config::load().map_err(fail)?;
            let db = cicada_infra::Db::connect(&config.db).await.map_err(fail)?;
            let category = if tree {
                cicada_codegen::TplCategory::Tree
            } else {
                cicada_codegen::TplCategory::Crud
            };
            let plan = cicada_codegen::plan_from_database(&db, &config.codegen, &table, category)
                .await
                .map_err(fail)?;
            let input = cicada_codegen::RenderInput {
                plan: &plan,
                date: chrono::Utc::now().format("%Y%m%d").to_string(),
                seq: format!("{:06}", plan.columns.len()),
                route_prefix: config.codegen.route_prefix.clone(),
                template_dir: config
                    .codegen
                    .template_dir
                    .clone()
                    .map(std::path::PathBuf::from),
            };
            let artifacts = cicada_codegen::render_all(&input).map_err(fail)?;
            let zip = cicada_codegen::zip_artifacts(&artifacts).map_err(fail)?;
            std::fs::write(&out, zip).map_err(fail)?;
            println!("已生成 {} 个文件 → {out}", artifacts.len());
        }
        "new" => {
            let opts = new::NewOptions::from_args(rest).map_err(CliError::Usage)?;
            let n = new::render(&opts).map_err(fail)?;
            println!(
                "已生成 {n} 个文件 → {}\n\n下一步：\n  cd {}\n  cargo run --bin migrate -- up --seed --admin\n  cargo run",
                opts.dir.display(),
                opts.dir.display()
            );
        }
        other => return Err(CliError::Usage(format!("未知命令：{other}"))),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn flag_supports_separate_and_inline_forms() {
        let a = args(&["init", "--name", "租户A", "--admin=bob"]);
        assert_eq!(flag(&a, "--name").as_deref(), Some("租户A"));
        assert_eq!(flag(&a, "--admin").as_deref(), Some("bob"));
        assert_eq!(flag(&a, "--password"), None);
    }

    #[test]
    fn flags_collects_repeats() {
        let a = args(&["enable", "--table", "sys_user", "--table=sys_role"]);
        assert_eq!(flags(&a, "--table"), vec!["sys_user", "sys_role"]);
    }
}
