//! 业务模块（业务代码与框架物理分离）。
//!
//! 代码生成器的产物落在这里，一个业务表 = 一个子目录（entity / dto / handler / module）。
//! 由 `src/main.rs` 经 `#[path = "../business/mod.rs"]` 挂上，因此业务代码不必挤进 `src/`。

pub mod goods;

// 新表进来后在这里加一行：`pub mod <表名>;`
