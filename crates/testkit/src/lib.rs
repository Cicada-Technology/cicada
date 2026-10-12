//! cicada-testkit（测试夹具）
//!
//! 带归属维度的样例实体，供 typestate 的编译失败测试与组件测试复用；
//! 测试分层的权限矩阵 / 数据范围组件测试也落在本 crate 的 `tests/`。
//! 职责边界与「不做什么」见 `README.md`。

pub mod fixtures;
