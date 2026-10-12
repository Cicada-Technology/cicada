//! 分页排序白名单（请求 `page` + `page_size` + 排序白名单 `sort_by` / `sort_order`）。
//!
//! 为什么是白名单而不是「把请求里的列名拼进 ORDER BY」：分页与查询定的规矩是不让客户端控制
//! SQL 标识符。一个自由文本的 `sort_by` 既是注入面，也是结构探测面（报错里带出
//! `column "totp_secret" does not exist` 等于把表结构送给外人）。所以对外只认
//! **服务端声明过的名字**，真实列名从常量里取，客户端那串字符永远进不了标识符位置。
//!
//! 名字不在白名单里时的选择是**忽略**（回落到该接口自己的默认排序），不是 422：
//! 列表页传了个不该传的排序字段，最该发生的是「照常返回数据」，而不是整页打不开。
//! `sort_order` 只认字面量 `asc`（大小写不敏感），其余一律 `desc`——后台列表的默认排序
//! 几乎都是「新的在前」，把拼错的顺序当成升序反而更反直觉。

use sea_orm::EntityTrait;
use sea_orm::sea_query::{Alias, Expr};
use serde::Deserialize;
use utoipa::ToSchema;

use cicada_kernel::scope::{Query, QueryState};

/// 排序方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Asc,
    Desc,
}

/// 一个可排序列：`(对外名字, 表名, 真实列名)`，三段全是 `&'static str`。
///
/// 对外名字与真实列名分开，是为了让接口契约不绑死库内命名：参考实现前端的表格列习惯
/// `createTime` 驼峰，库里是 `create_time`，两个名字都允许，落到 SQL 的只有常量那一份。
pub type SortColumn = (&'static str, &'static str, &'static str);

/// 一个接口的排序白名单。
pub struct SortSpec {
    pub columns: &'static [SortColumn],
}

impl SortSpec {
    pub const fn new(columns: &'static [SortColumn]) -> Self {
        Self { columns }
    }
}

/// 纯解析：把客户端的两个参数落成白名单里的 `(表, 列, 方向)`。
///
/// 刻意与 SeaORM 类型无关——「哪些名字会被接受」是这条安全性质的全部，
/// 它能单独测，就不必把 SQL 构造拖进测试里。
pub fn resolve(
    spec: &SortSpec,
    sort_by: Option<&str>,
    sort_order: Option<&str>,
) -> Option<(&'static str, &'static str, Dir)> {
    let requested = sort_by.map(str::trim).filter(|v| !v.is_empty())?;
    let (_, table, column) = spec
        .columns
        .iter()
        .find(|(name, _, _)| name.eq_ignore_ascii_case(requested))?;
    let dir = if sort_order.is_some_and(|o| o.eq_ignore_ascii_case("asc")) {
        Dir::Asc
    } else {
        Dir::Desc
    };
    Some((table, column, dir))
}

/// 把 `sort_by` / `sort_order` 施加到查询上；不匹配白名单时原样返回（默认排序保持）。
pub fn apply<E: EntityTrait, S: QueryState>(
    query: Query<E, S>,
    spec: &SortSpec,
    sort_by: Option<&str>,
    sort_order: Option<&str>,
) -> Query<E, S> {
    let Some((table, column, dir)) = resolve(spec, sort_by, sort_order) else {
        return query;
    };
    let expr = Expr::column((Alias::new(table), Alias::new(column)));
    match dir {
        Dir::Asc => query.order_by(expr),
        Dir::Desc => query.order_by_desc(expr),
    }
}

/// 让调用点写成链式（`query.sorted_by(&SPEC, …)`），与 `ScopedQuery` 的其余算子一致。
///
/// 存在只是为了读起来不像在调工具函数：排序是查询的一部分，不是查询之外的装饰。
pub trait SortExt: Sized {
    fn sorted_by(self, spec: &SortSpec, sort_by: Option<&str>, sort_order: Option<&str>) -> Self;
}

impl<E: EntityTrait, S: QueryState> SortExt for Query<E, S> {
    fn sorted_by(self, spec: &SortSpec, sort_by: Option<&str>, sort_order: Option<&str>) -> Self {
        apply(self, spec, sort_by, sort_order)
    }
}

/// 列表 DTO 共用的一对可选排序参数。
///
/// 做成独立结构而不是给每个 `*ListQuery` 手写两个字段：参数形状是全接口共享的契约
/// ，而白名单必须逐个接口声明（哪张表能按哪几列排是属主的知识）。
/// DTO 用 `#[serde(flatten)]` 嵌进来，OpenAPI 里仍是平铺的两个 query 参数。
#[derive(Debug, Default, Clone, Deserialize, ToSchema)]
pub struct SortParams {
    /// 排序列名，必须在白名单里；不在则忽略。
    pub sort_by: Option<String>,
    /// `asc` 升序，其余（含缺省）按 `desc`。
    pub sort_order: Option<String>,
}

impl SortParams {
    /// 取出两个参数喂给 [`apply`]。
    pub fn parts(&self) -> (Option<&str>, Option<&str>) {
        (self.sort_by.as_deref(), self.sort_order.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::{Dir, SortSpec, resolve};

    const SPEC: SortSpec = SortSpec::new(&[
        ("user_name", "sys_user", "user_name"),
        ("create_time", "sys_user", "create_time"),
        ("createTime", "sys_user", "create_time"),
    ]);

    #[test]
    fn whitelisted_names_resolve_to_the_declared_column_and_direction() {
        assert_eq!(
            resolve(&SPEC, Some("create_time"), Some("asc")),
            Some(("sys_user", "create_time", Dir::Asc))
        );
        // 缺省方向是 desc：后台列表的直觉是「新的在前」。
        assert_eq!(
            resolve(&SPEC, Some("user_name"), None),
            Some(("sys_user", "user_name", Dir::Desc))
        );
        // 驼峰别名与大小写都不该要求客户端猜库内命名。
        assert_eq!(
            resolve(&SPEC, Some("CREATETIME"), Some("ASC")),
            Some(("sys_user", "create_time", Dir::Asc))
        );
        // 真实列名永远来自常量，不是来自请求。
        assert_eq!(
            resolve(&SPEC, Some("createTime"), Some("asc")),
            Some(("sys_user", "create_time", Dir::Asc))
        );
    }

    #[test]
    fn anything_outside_the_whitelist_is_dropped_not_passed_through() {
        // 这一条同时是注入面与结构探测面的闸门：越界的名字连「真实列名」那一层都到不了。
        for hostile in [
            "totp_secret",
            "password",
            "1; DROP TABLE sys_user",
            "(SELECT password FROM sys_user LIMIT 1)",
            "user_name DESC, password",
            "user_name\"); DROP TABLE sys_user; --",
        ] {
            assert_eq!(
                resolve(&SPEC, Some(hostile), Some("asc")),
                None,
                "{hostile} 不该被接受"
            );
        }
    }

    #[test]
    fn empty_or_missing_params_keep_the_default_order() {
        assert_eq!(resolve(&SPEC, None, Some("desc")), None);
        assert_eq!(resolve(&SPEC, Some("   "), Some("asc")), None);
    }

    #[test]
    fn only_literal_asc_means_ascending() {
        // 拼错的顺序值不能被当成升序：升序是后台列表里更少人想要的那个。
        for other in ["ascending", "up", "", "0", "true", "DESC "] {
            assert_eq!(
                resolve(&SPEC, Some("user_name"), Some(other)).map(|(_, _, d)| d),
                Some(Dir::Desc),
                "{other:?}"
            );
        }
    }
}
