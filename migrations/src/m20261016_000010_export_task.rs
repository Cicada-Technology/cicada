use sea_orm_migration::prelude::*;

/// `sys_export_task`：异步导出任务。
///
/// `query` 是条件快照（含数据范围），保证后台执行与发起请求时的可见范围一致；
/// 完成后 `file_id` 指向 `sys_file`，下载走文件模块的鉴权链路。属主 `system`。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum SysExportTask {
    Table,
    Id,
    TenantId,
    UserId,
    Module,
    Query,
    Status,
    FileId,
    Total,
    Processed,
    Error,
    FinishTime,
    CreateTime,
    UpdateTime,
    DeletedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(SysExportTask::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(SysExportTask::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(SysExportTask::TenantId)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(SysExportTask::UserId)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(SysExportTask::Module)
                            .string_len(64)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(SysExportTask::Query)
                            .json_binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(SysExportTask::Status)
                            .small_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(SysExportTask::FileId).big_integer())
                    .col(
                        ColumnDef::new(SysExportTask::Total)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(SysExportTask::Processed)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(SysExportTask::Error).text())
                    .col(ColumnDef::new(SysExportTask::FinishTime).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(SysExportTask::CreateTime)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(ColumnDef::new(SysExportTask::UpdateTime).timestamp_with_time_zone())
                    .col(ColumnDef::new(SysExportTask::DeletedAt).timestamp_with_time_zone())
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_sys_export_task_tenant_user")
                    .table(SysExportTask::Table)
                    .col(SysExportTask::TenantId)
                    .col(SysExportTask::UserId)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(SysExportTask::Table).to_owned())
            .await
    }
}
