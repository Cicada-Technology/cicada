use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

/// 菜单种子修订：清理框架自带外链菜单的旧文案，并补上数据监控的读权限按钮。
///
/// 1. `menu_id = 4` 是框架自带的外链菜单；旧库仍可能保留早期项目的名称与地址，
///    这里统一改为 Rust 官网，避免升级后侧栏继续出现旧品牌。
/// 2. 数据监控页需要一个 F 类权限点守卫只读统计接口；菜单 `monitor:data:view`
///    仍然只负责菜单可见性，不直接守卫 REST。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        db.execute_unprepared(
            "UPDATE sys_menu \
                SET menu_name = 'Rust 官网', \
                    path = 'https://www.rust-lang.org/', \
                    icon = 'Globe', \
                    update_time = now() \
              WHERE menu_id = 4",
        )
        .await?;

        db.execute_unprepared(
            "INSERT INTO sys_menu \
                 (menu_id, menu_name, parent_id, order_num, path, menu_type, visible, status, \
                  perms, icon, remark, create_by, create_time) \
             VALUES \
                 (1094, '数据查询', 111, 1, '#', 'F', 0, 0, \
                  'monitor:data:list', '', '数据监控只读统计', 'admin', now()) \
             ON CONFLICT (menu_id) DO NOTHING",
        )
        .await?;

        // 全新 schema 的迁移早于 `init-admin`，此时 role 1 还不存在；有则补授权，
        // 没有则交给 `init_admin`（它已经把 1094 加进默认管理员的菜单集合）。
        db.execute_unprepared(
            "INSERT INTO sys_role_menu (role_id, menu_id) \
             SELECT 1, 1094 \
              WHERE EXISTS (SELECT 1 FROM sys_role WHERE role_id = 1) \
             ON CONFLICT DO NOTHING",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DELETE FROM sys_role_menu WHERE role_id = 1 AND menu_id = 1094")
            .await?;
        db.execute_unprepared("DELETE FROM sys_menu WHERE menu_id = 1094")
            .await?;
        Ok(())
    }
}
