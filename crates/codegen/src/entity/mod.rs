//! 代码生成域实体：`gen_table` / `gen_table_column`。
//!
//! 命名约定：实体枚举 `…Entity`，列枚举 `…Col`，模型 `…Model`，活动模型 `…ActiveModel`。

pub mod gen_table;
pub mod gen_table_column;

pub use gen_table::{
    ActiveModel as GenTableActiveModel, Column as GenTableCol, Entity as GenTableEntity,
    Model as GenTableModel,
};
pub use gen_table_column::{
    ActiveModel as GenTableColumnActiveModel, Column as GenTableColumnCol,
    Entity as GenTableColumnEntity, Model as GenTableColumnModel,
};
