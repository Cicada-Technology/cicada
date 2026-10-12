//! 商品 DTO（由 cicada 代码生成器产出）。
//!
//! 时间类列在 DTO 里是 `String`（RFC3339 / `%Y-%m-%d` / `%H:%M:%S`），
//! 由 handler 解析成 `chrono` 类型，避免给生成工程强加 `chrono` 的 serde 特性。

use sea_orm::entity::prelude::{Decimal};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::entity::Model;

/// 列表查询参数。
#[derive(Debug, Deserialize, IntoParams)]
pub struct GoodsListQuery {
    /// 归属部门
    pub dept_id: Option<i64>,
    /// 商品名称
    pub goods_name: Option<String>,
    /// 商品类型
    pub type_col: Option<String>,
    /// 价格
    pub price: Option<Decimal>,
    /// 库存
    pub quantity: Option<i32>,
    /// 是否上架
    pub online: Option<bool>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

/// 行视图。
///
#[derive(Debug, Serialize, ToSchema)]
pub struct GoodsVo {
    /// 商品 id
    pub goods_id: i64,
    /// 归属部门
    pub dept_id: i64,
    /// 商品名称
    pub goods_name: String,
    /// 商品类型
    pub type_col: String,
    /// 价格
    pub price: Option<Decimal>,
    /// 库存
    pub quantity: i32,
    /// 是否上架
    pub online: bool,
    /// 上架日期
    pub publish_date: Option<String>,
    /// 上架时间
    pub publish_at: Option<String>,
    /// 备注
    pub remark: Option<String>,
}

/// 分页结果。
#[derive(Debug, Serialize, ToSchema)]
pub struct GoodsPage {
    pub items: Vec<GoodsVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

impl From<Model> for GoodsVo {
    fn from(m: Model) -> Self {
        Self {
            goods_id: m.goods_id,
            dept_id: m.dept_id,
            goods_name: m.goods_name,
            type_col: m.type_col,
            price: m.price,
            quantity: m.quantity,
            online: m.online,
            publish_date: m.publish_date.map(|v| v.to_string()),
            publish_at: m.publish_at.map(|v| v.to_rfc3339()),
            remark: m.remark,
        }
    }
}

/// 新增 / 修改入参。
#[derive(Debug, Deserialize, ToSchema)]
pub struct GoodsUpsert {
    /// 归属部门
    pub dept_id: i64,
    /// 商品名称
    pub goods_name: String,
    /// 商品类型
    pub type_col: String,
    /// 价格
    pub price: Option<Decimal>,
    /// 库存
    pub quantity: i32,
    /// 是否上架
    pub online: bool,
    /// 上架日期
    pub publish_date: Option<String>,
    /// 上架时间
    pub publish_at: Option<String>,
    /// 备注
    pub remark: Option<String>,
}
