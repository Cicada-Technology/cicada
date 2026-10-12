use sea_orm_migration::prelude::*;

/// 事务性发件箱 `outbox_event`。
///
/// 生产者是 `audit`：审计只写这张表，不碰 `system` 的 `sys_oper_log` / `sys_logininfor`
/// （数据所有权规则）。`relay` 按 `(published_at, id)` 顺序扫未投递行，
/// 投递成功后写 `published_at`，失败只累加 `attempts`。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum OutboxEvent {
    Table,
    Id,
    TenantId,
    Aggregate,
    AggregateId,
    EventType,
    Payload,
    CreatedAt,
    PublishedAt,
    Attempts,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(OutboxEvent::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OutboxEvent::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(OutboxEvent::TenantId)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OutboxEvent::Aggregate)
                            .string_len(64)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OutboxEvent::AggregateId)
                            .string_len(128)
                            .null(),
                    )
                    .col(
                        ColumnDef::new(OutboxEvent::EventType)
                            .string_len(64)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OutboxEvent::Payload)
                            .json_binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OutboxEvent::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(OutboxEvent::PublishedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(OutboxEvent::Attempts)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .to_owned(),
            )
            .await?;
        // relay 的扫描形态：published_at IS NULL ORDER BY id。
        manager
            .create_index(
                Index::create()
                    .name("idx_outbox_event_pending")
                    .table(OutboxEvent::Table)
                    .col(OutboxEvent::PublishedAt)
                    .col(OutboxEvent::Id)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(OutboxEvent::Table).to_owned())
            .await
    }
}
