//! 商品 模块声明（由 cicada 代码生成器产出）。
//!
//! `owns` 声明数据属主、`perms` 声明权限点、`routes` 交出带元数据的路由；
//! 装配期会把三者与路由上的 `#[require_perm]` 做双向对账，对不上就不启动。

use cicada_web::{Manifest, Module, Permission, TableName};

use super::handler::routes;

/// 商品模块。
pub struct GoodsModule;

impl Module for GoodsModule {
    fn manifest(self: Box<Self>) -> Manifest {
        Manifest::new("business:goods")
            .routes(routes)
            .owns([TableName("biz_goods")])
            .perms([
                Permission::new("business:goods:list"),
                Permission::new("business:goods:add"),
                Permission::new("business:goods:edit"),
                Permission::new("business:goods:remove"),
            ])
    }
}
