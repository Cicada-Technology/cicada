//! 编译失败用例：未升格就执行终结算子（datascope-typestate.md ）。
//!
//! `Query<E, Unchecked>` 没有 `all` / `one` / `count` / `fetch_page`。

use cicada_kernel::scope::Query;
use cicada_testkit::fixtures;

fn main() {
    let _ = Query::<fixtures::Entity>::open().all(&42);
}
