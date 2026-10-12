//! 通知中心实体。

pub mod channel;
pub mod record;
pub mod template;

pub use channel::{
    ActiveModel as ChannelActiveModel, Column as ChannelColumn, Entity as ChannelEntity,
    Model as ChannelModel, Relation as ChannelRelation,
};
pub use record::{
    ActiveModel as RecordActiveModel, Column as RecordColumn, Entity as RecordEntity,
    Model as RecordModel, Relation as RecordRelation,
};
pub use template::{
    ActiveModel as TemplateActiveModel, Column as TemplateColumn, Entity as TemplateEntity,
    Model as TemplateModel, Relation as TemplateRelation,
};
