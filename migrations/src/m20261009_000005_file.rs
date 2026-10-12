use sea_orm_migration::prelude::*;

/// `sys_file`：文件元数据。
///
/// 物理路径 `path` 不可猜测、对外不暴露；`mime` 由 magic number 判定，
/// `sha256` 用于去重与完整性校验。属主 `system`。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum SysFile {
    Table,
    FileId,
    TenantId,
    Storage,
    Path,
    OriginalName,
    Size,
    Mime,
    Sha256,
    BizType,
    BizId,
    CreateBy,
    CreateTime,
    UpdateBy,
    UpdateTime,
    DeletedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(SysFile::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(SysFile::FileId)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(SysFile::TenantId).big_integer().not_null())
                    .col(ColumnDef::new(SysFile::Storage).string_len(32).not_null())
                    .col(ColumnDef::new(SysFile::Path).string_len(512).not_null())
                    .col(
                        ColumnDef::new(SysFile::OriginalName)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(ColumnDef::new(SysFile::Size).big_integer().not_null())
                    .col(ColumnDef::new(SysFile::Mime).string_len(128).not_null())
                    .col(ColumnDef::new(SysFile::Sha256).string_len(64).not_null())
                    .col(ColumnDef::new(SysFile::BizType).string_len(64))
                    .col(ColumnDef::new(SysFile::BizId).big_integer())
                    .col(ColumnDef::new(SysFile::CreateBy).string_len(64))
                    .col(
                        ColumnDef::new(SysFile::CreateTime)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(ColumnDef::new(SysFile::UpdateBy).string_len(64))
                    .col(ColumnDef::new(SysFile::UpdateTime).timestamp_with_time_zone())
                    .col(ColumnDef::new(SysFile::DeletedAt).timestamp_with_time_zone())
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_sys_file_tenant_sha")
                    .table(SysFile::Table)
                    .col(SysFile::TenantId)
                    .col(SysFile::Sha256)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(SysFile::Table).to_owned())
            .await
    }
}
