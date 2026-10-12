//! `biz_category` 建表迁移（由 cicada 代码生成器产出）。
//!
//! 该表是从既有库结构发现的，迁移用于在**其他库**重建同名表；本库执行时
//! `if_not_exists` 会让它成为空操作。
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
#[allow(clippy::enum_variant_names)]
enum Category {
    #[sea_orm(iden = "biz_category")]
    Table,
    CatId,
    TenantId,
    DeptId,
    ParentId,
    CatName,
    SortOrder,
    Online,
    Remark,
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
                    .table(Category::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Category::CatId).big_integer().not_null().auto_increment().primary_key())
                    .col(ColumnDef::new(Category::TenantId).big_integer().not_null())
                    .col(ColumnDef::new(Category::DeptId).big_integer().not_null())
                    .col(ColumnDef::new(Category::ParentId).big_integer().not_null())
                    .col(ColumnDef::new(Category::CatName).string().not_null())
                    .col(ColumnDef::new(Category::SortOrder).integer().not_null())
                    .col(ColumnDef::new(Category::Online).boolean().not_null())
                    .col(ColumnDef::new(Category::Remark).text())
                    .col(ColumnDef::new(Category::CreateBy).string())
                    .col(ColumnDef::new(Category::CreateTime).timestamp_with_time_zone().not_null())
                    .col(ColumnDef::new(Category::UpdateBy).string())
                    .col(ColumnDef::new(Category::UpdateTime).timestamp_with_time_zone())
                    .col(ColumnDef::new(Category::DeletedAt).timestamp_with_time_zone())
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Category::Table).to_owned())
            .await?;
        Ok(())
    }
}
