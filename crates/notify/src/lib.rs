//! cicada-notify（L2 领域模块）：通知中心。
//!
//! 渠道（`sys_notify_channel`）+ 模板（`sys_notify_template`）+ 记录（`sys_notify_record`）：
//! 模板用 minijinja 渲染，渠道按 `kind` 走各自的传输——`sms`/`webhook`/`im` 是 HTTP 网关，
//! `email` 配了 `host` 就直连 SMTP（`smtp`，技术选型的 lettre），否则也走网关。每次投递留痕。
//!
//! 本 crate 拥有这三张表；管理接口在 `cicada-web`，组装根注入 `NotifyService`。

pub mod channel;
pub mod entity;
pub mod render;
pub mod service;
pub mod smtp;

pub use channel::{
    ChannelConfig, ChannelKind, Delivery, DeliveryOutcome, HttpSender, NotifyError, NotifySender,
    RecordingSender, RoutingSender, shared,
};
pub use entity::{
    ChannelActiveModel, ChannelColumn, ChannelEntity, ChannelModel, RecordActiveModel,
    RecordColumn, RecordEntity, RecordModel, TemplateActiveModel, TemplateColumn, TemplateEntity,
    TemplateModel,
};
pub use render::{render, validate};
pub use service::{NotifyService, RECORD_STATUS_PENDING, SendOutcome, SendRequest};
