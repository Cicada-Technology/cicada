//! 商品分类 模块声明（由 cicada 代码生成器产出）。
//!
//! `owns` 声明数据属主、`perms` 声明权限点、`routes` 交出带元数据的路由；
//! 装配期会把三者与路由上的 `#[require_perm]` 做双向对账，对不上就不启动。

use cicada_web::{Manifest, Module, Permission, TableName};

use super::handler::routes;

/// 商品分类模块。
pub struct CategoryModule;

impl Module for CategoryModule {
    fn manifest(self: Box<Self>) -> Manifest {
        Manifest::new("business:category")
            .routes(routes)
            .owns([TableName("biz_category")])
            .perms([
                Permission::new("business:category:list"),
                Permission::new("business:category:add"),
                Permission::new("business:category:edit"),
                Permission::new("business:category:remove"),
            ])
    }
}
