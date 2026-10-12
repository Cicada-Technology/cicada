use sea_orm_migration::prelude::*;

/// 在线会话快照 `sys_user_online`：权威在 Redis；此表为异步
/// write-behind 快照，可重建、不参与踢出/上限判定。
///
/// 此前这张表只存在于文档里：会话生命周期（登录 / 登出 / 强退 / 改密失效）在 Redis 发生后
/// 就没有任何持久痕迹，「谁在什么时候从哪个 IP 登录过、会话到什么时候结束」只能靠
/// `sys_logininfor` 拼出一半。快照把另一半补上，并明确它是**派生数据**：
/// 随时可以 `TRUNCATE` 后从 Redis 重建，所以它不承载任何判定逻辑。
///
/// 主键直接用 `session_id`（= access token 的 `jti`）：会话与行一一对应，
/// write-behind 于是天然幂等（同一 jti 重复投递只会 upsert）。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum SysUserOnline {
    Table,
    SessionId,
    TenantId,
    UserId,
    UserName,
    Ip,
    UserAgent,
    LoginTime,
    LastAccessTime,
    Status,
    EndTime,
    CreateTime,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(SysUserOnline::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(SysUserOnline::SessionId)
                            .string_len(64)
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(SysUserOnline::TenantId)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(SysUserOnline::UserId)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(SysUserOnline::UserName)
                            .string_len(64)
                            .not_null(),
                    )
                    .col(ColumnDef::new(SysUserOnline::Ip).string_len(64).not_null())
                    .col(ColumnDef::new(SysUserOnline::UserAgent).text().not_null())
                    .col(
                        ColumnDef::new(SysUserOnline::LoginTime)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(SysUserOnline::LastAccessTime)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    // 0 在线 / 1 已退出（对齐参考实现 `sys_user_online.status`）。
                    .col(
                        ColumnDef::new(SysUserOnline::Status)
                            .small_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(SysUserOnline::EndTime).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(SysUserOnline::CreateTime)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;
        // 历史查询按「本租户 + 登录时间倒序」翻页；「某人的会话」是第二个高频入口。
        manager
            .create_index(
                Index::create()
                    .name("idx_sys_user_online_tenant_login")
                    .table(SysUserOnline::Table)
                    .col(SysUserOnline::TenantId)
                    .col(SysUserOnline::LoginTime)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_sys_user_online_user")
                    .table(SysUserOnline::Table)
                    .col(SysUserOnline::UserId)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(SysUserOnline::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}
