//! 排序白名单的配套件：给裸 `sea_orm::Select<E>` 的列表查询提供同名 `sorted_by`。
//!
//! [`crate::sort::SortExt`] 覆盖的是内核受控查询（`Query<E, S>`）。但有些列表拿的是原生
//! `Select`：全局表（`sys_dict_type`/`sys_config`/`sys_notice` 本就没有租户维度，不实现
//! `DataScoped`，进不了 `.scoped()` 通道），以及 handler 自行按 `tenant_id` + 软删过滤的
//! 表（`sys_role`/`sys_job`）。白名单判定仍然只有 `sort::resolve` 这一份——本模块只是把
//! 同一个决定接到 `Select` 的 `order_by` 上，不新增任何信任边界：客户端字符串依旧进不了
//! 标识符位置，名字不在白名单里时依旧回落到默认排序。

use sea_orm::sea_query::{Alias, Expr};
use sea_orm::{EntityTrait, QueryOrder, Select};

use crate::sort::{Dir, SortSpec, resolve};

/// [`crate::sort::SortExt`] 的 `Select` 版本；方法名与语义完全一致，调用点写法不变。
pub trait SelectSortExt<E: EntityTrait> {
    fn sorted_by(self, spec: &SortSpec, sort_by: Option<&str>, sort_order: Option<&str>) -> Self;
}

impl<E: EntityTrait> SelectSortExt<E> for Select<E> {
    fn sorted_by(self, spec: &SortSpec, sort_by: Option<&str>, sort_order: Option<&str>) -> Self {
        let Some((table, column, dir)) = resolve(spec, sort_by, sort_order) else {
            return self;
        };
        let expr = Expr::column((Alias::new(table), Alias::new(column)));
        match dir {
            Dir::Asc => self.order_by_asc(expr),
            Dir::Desc => self.order_by_desc(expr),
        }
    }
}
