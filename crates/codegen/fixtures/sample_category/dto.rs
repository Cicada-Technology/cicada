//! 商品分类 DTO（由 cicada 代码生成器产出）。
//!
//! 时间类列在 DTO 里是 `String`（RFC3339 / `%Y-%m-%d` / `%H:%M:%S`），
//! 由 handler 解析成 `chrono` 类型，避免给生成工程强加 `chrono` 的 serde 特性。

use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::entity::Model;

/// 列表查询参数。
#[derive(Debug, Deserialize, IntoParams)]
pub struct CategoryListQuery {
    /// 归属部门
    pub dept_id: Option<i64>,
    /// 分类名称
    pub cat_name: Option<String>,
    /// 显示顺序
    pub sort_order: Option<i32>,
    /// 是否启用
    pub online: Option<bool>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

/// 行视图。
///
/// 树表：`children` 由树接口填充；分页/详情接口里恒为空（序列化时省略）。
#[derive(Debug, Serialize, ToSchema)]
pub struct CategoryVo {
    /// 分类 id
    pub cat_id: i64,
    /// 归属部门
    pub dept_id: i64,
    /// 上级分类
    pub parent_id: i64,
    /// 分类名称
    pub cat_name: String,
    /// 显示顺序
    pub sort_order: i32,
    /// 是否启用
    pub online: bool,
    /// 备注
    pub remark: Option<String>,
    /// 子节点（树接口填充）。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<CategoryVo>,
}

/// 分页结果。
#[derive(Debug, Serialize, ToSchema)]
pub struct CategoryPage {
    pub items: Vec<CategoryVo>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

impl From<Model> for CategoryVo {
    fn from(m: Model) -> Self {
        Self {
            cat_id: m.cat_id,
            dept_id: m.dept_id,
            parent_id: m.parent_id,
            cat_name: m.cat_name,
            sort_order: m.sort_order,
            online: m.online,
            remark: m.remark,
            children: Vec::new(),
        }
    }
}

/// 新增 / 修改入参。
#[derive(Debug, Deserialize, ToSchema)]
pub struct CategoryUpsert {
    /// 归属部门
    pub dept_id: i64,
    /// 上级分类
    pub parent_id: i64,
    /// 分类名称
    pub cat_name: String,
    /// 显示顺序
    pub sort_order: i32,
    /// 是否启用
    pub online: bool,
    /// 备注
    pub remark: Option<String>,
}
