//! `biz_goods` 建表迁移（由 cicada 代码生成器产出）。
//!
//! 该表是从既有库结构发现的，迁移用于在**其他库**重建同名表；本库执行时
//! `if_not_exists` 会让它成为空操作。
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
#[allow(clippy::enum_variant_names)]
enum Goods {
    #[sea_orm(iden = "biz_goods")]
    Table,
    GoodsId,
    TenantId,
    DeptId,
    GoodsName,
    #[sea_orm(iden = "type")]
    TypeCol,
    Price,
    Quantity,
    Online,
    PublishDate,
    PublishAt,
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
                    .table(Goods::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Goods::GoodsId).big_integer().not_null().auto_increment().primary_key())
                    .col(ColumnDef::new(Goods::TenantId).big_integer().not_null())
                    .col(ColumnDef::new(Goods::DeptId).big_integer().not_null())
                    .col(ColumnDef::new(Goods::GoodsName).string().not_null())
                    .col(ColumnDef::new(Goods::TypeCol).string().not_null())
                    .col(ColumnDef::new(Goods::Price).decimal())
                    .col(ColumnDef::new(Goods::Quantity).integer().not_null())
                    .col(ColumnDef::new(Goods::Online).boolean().not_null())
                    .col(ColumnDef::new(Goods::PublishDate).date())
                    .col(ColumnDef::new(Goods::PublishAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(Goods::Remark).text())
                    .col(ColumnDef::new(Goods::CreateBy).string())
                    .col(ColumnDef::new(Goods::CreateTime).timestamp_with_time_zone().not_null())
                    .col(ColumnDef::new(Goods::UpdateBy).string())
                    .col(ColumnDef::new(Goods::UpdateTime).timestamp_with_time_zone())
                    .col(ColumnDef::new(Goods::DeletedAt).timestamp_with_time_zone())
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Goods::Table).to_owned())
            .await?;
        Ok(())
    }
}
