//! `NotifyService`：模板渲染 → 渠道投递 → 记录留痕。
//!
//! 每次投递都先落一条 `sys_notify_record`（0 待发），投递结果回写（1 已发 / 2 失败）。
//! 投递失败**不回滚记录**：留痕本身就是目的；失败原因写进 `error` 列，调用方据此决定
//! 重试或告警。

use std::sync::Arc;

use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

use cicada_infra::Db;
use cicada_kernel::TenantId;

use crate::channel::{ChannelConfig, ChannelKind, Delivery, NotifyError, NotifySender};
use crate::entity::{
    ChannelColumn, ChannelEntity, RecordActiveModel, RecordColumn, RecordEntity, TemplateColumn,
    TemplateEntity,
};
use crate::render::render;

/// 投递请求。
#[derive(Clone, Debug)]
pub struct SendRequest {
    /// 渠道 `code`。
    pub channel_code: String,
    /// 模板 `code`。
    pub template_code: String,
    /// 收件人：手机号 / 邮箱 / webhook 地址。
    pub recipient: String,
    /// 语言，缺省 `zh-CN`。
    pub locale: Option<String>,
    /// 模板上下文。
    pub payload: serde_json::Value,
    /// 操作人（写进记录）。
    pub operator: Option<String>,
}

/// 投递结果（记录已落库，含失败）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendOutcome {
    pub record_id: i64,
    /// 1 已发 / 2 失败。
    pub status: i16,
    pub error: Option<String>,
    /// 模板渲染后的正文，便于管理台「测试发送」直接回显。
    pub rendered_body: String,
}

impl SendOutcome {
    pub const SENT: i16 = 1;
    pub const FAILED: i16 = 2;
}

/// 记录状态码（与 `sys_notify_record.status` 对齐）。
pub const RECORD_STATUS_PENDING: i16 = 0;

#[derive(Clone)]
pub struct NotifyService {
    db: Db,
    sender: Arc<dyn NotifySender>,
}

impl NotifyService {
    pub fn new(db: Db, sender: Arc<dyn NotifySender>) -> Self {
        Self { db, sender }
    }

    /// 在候选语言里按「精确匹配 → zh-CN → 第一个」选出模板位置。
    pub fn choose_locale(available: &[String], preferred: &str) -> Option<usize> {
        if let Some(i) = available.iter().position(|l| l == preferred) {
            return Some(i);
        }
        if let Some(i) = available.iter().position(|l| l == "zh-CN") {
            return Some(i);
        }
        if available.is_empty() { None } else { Some(0) }
    }

    /// 走完整链路投递一次。
    pub async fn send(
        &self,
        tenant: TenantId,
        req: SendRequest,
    ) -> Result<SendOutcome, NotifyError> {
        let channel = ChannelEntity::find()
            .filter(ChannelColumn::TenantId.eq(tenant.get()))
            .filter(ChannelColumn::Code.eq(req.channel_code.clone()))
            .one(self.db.conn())
            .await?
            .ok_or_else(|| NotifyError::NotFound(format!("渠道 {}", req.channel_code)))?;
        if !channel.enabled {
            return Err(NotifyError::Disabled(channel.code.clone()));
        }
        let kind = ChannelKind::parse(&channel.kind)
            .ok_or_else(|| NotifyError::Config(format!("未知渠道类型 {}", channel.kind)))?;

        let templates = TemplateEntity::find()
            .filter(TemplateColumn::TenantId.eq(tenant.get()))
            .filter(TemplateColumn::Code.eq(req.template_code.clone()))
            .filter(TemplateColumn::ChannelKind.eq(kind.as_str()))
            .filter(TemplateColumn::Enabled.eq(true))
            .all(self.db.conn())
            .await?;
        if templates.is_empty() {
            return Err(NotifyError::NotFound(format!(
                "模板 {}（{}）",
                req.template_code,
                kind.as_str()
            )));
        }
        let locales: Vec<String> = templates.iter().map(|t| t.locale.clone()).collect();
        let preferred = req.locale.clone().unwrap_or_else(|| "zh-CN".to_string());
        let idx = Self::choose_locale(&locales, &preferred).expect("templates 非空");
        let template = &templates[idx];

        let (title, body) = render(&template.title, &template.body, &req.payload)?;

        let record = RecordActiveModel {
            tenant_id: Set(tenant.get()),
            channel_id: Set(channel.id),
            template_code: Set(req.template_code.clone()),
            recipient: Set(req.recipient.clone()),
            payload: Set(req.payload.clone()),
            status: Set(RECORD_STATUS_PENDING),
            create_by: Set(req.operator.clone()),
            ..Default::default()
        }
        .insert(self.db.conn())
        .await?;

        let config: ChannelConfig = serde_json::from_value(channel.config.clone())
            .map_err(|e| NotifyError::Config(format!("渠道 {} 配置非法：{e}", channel.code)))?;
        let delivery = Delivery {
            kind,
            config,
            recipient: req.recipient.clone(),
            title,
            body: body.clone(),
            payload: req.payload.clone(),
        };
        let delivered = self.sender.deliver(&delivery).await;

        let (status, error) = match delivered {
            Ok(_) => (SendOutcome::SENT, None),
            Err(e) => {
                tracing::warn!(error = %e, channel = %channel.code, "通知投递失败");
                (SendOutcome::FAILED, Some(e.to_string()))
            }
        };

        RecordEntity::update_many()
            .col_expr(
                RecordColumn::Status,
                sea_orm::sea_query::Expr::value(status),
            )
            .col_expr(
                RecordColumn::Error,
                sea_orm::sea_query::Expr::value(error.clone()),
            )
            .col_expr(
                RecordColumn::SentAt,
                sea_orm::sea_query::Expr::value(if status == SendOutcome::SENT {
                    Some(chrono_now())
                } else {
                    None
                }),
            )
            .filter(RecordColumn::Id.eq(record.id))
            .exec(self.db.conn())
            .await?;

        Ok(SendOutcome {
            record_id: record.id,
            status,
            error,
            rendered_body: body,
        })
    }
}

fn chrono_now() -> sea_orm::prelude::DateTimeUtc {
    sea_orm::prelude::DateTimeUtc::from(std::time::SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_preference_is_exact_then_zh_cn_then_first() {
        let locales = vec!["en-US".to_string(), "zh-CN".to_string()];
        assert_eq!(NotifyService::choose_locale(&locales, "en-US"), Some(0));
        assert_eq!(NotifyService::choose_locale(&locales, "ja-JP"), Some(1));
        let only_en = vec!["en-US".to_string()];
        assert_eq!(NotifyService::choose_locale(&only_en, "ja-JP"), Some(0));
        assert_eq!(NotifyService::choose_locale(&[], "ja-JP"), None);
    }
}
