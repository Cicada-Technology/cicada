//! 渠道类型、驱动配置与投递缝（`sys_notify_channel`）。
//!
//! `sms` / `webhook` / `im` 走 **HTTP 网关**：`endpoint` + `token` 由渠道配置给出，
//! 适配器把「收件人 + 渲染后的标题/正文 + 原始 payload」编码成网关的请求体。
//! `email` 有两条后端（选的是 lettre/SMTP）：配了 `host` 就直接说 SMTP
//! （`smtp::SmtpSender`），只配 `endpoint` 则仍然走 HTTP 邮件网关——同一份渠道配置
//! 形状，运营选哪种由他们填的字段决定，`RoutingSender` 负责分岔。
//!
//! 缝纪律：`NotifySender` 在 crate 内有生产（`HttpSender`）与测试（`RecordingSender`）
//! 两个 adapter，因此值得抽成 trait；它不是跨模块端口，不进 `cicada-ports`。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// 渠道类型（`sys_notify_channel.kind`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelKind {
    Sms,
    Email,
    Webhook,
    Im,
}

impl ChannelKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sms => "sms",
            Self::Email => "email",
            Self::Webhook => "webhook",
            Self::Im => "im",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "sms" => Self::Sms,
            "email" => Self::Email,
            "webhook" => Self::Webhook,
            "im" => Self::Im,
            _ => return None,
        })
    }
}

/// 渠道驱动配置（`sys_notify_channel.config` 的 JSON 结构）。
///
/// 里的 `token` / `password` 是凭据：写读接口都不回显（见 `cicada-web` 的
/// `mask_channel_secrets`），值本身可以写成 `${SMTP_PASSWORD}` 这类占位符，
/// 投递时按配置「敏感值来自环境变量」展开，避免口令落进数据库那一行。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ChannelConfig {
    /// HTTP 网关地址（`sms` / `webhook` / `im` 必填；`email` 与 `host` 二选一）。
    #[serde(default)]
    pub endpoint: String,
    /// SMTP 服务器（仅 `email`）：填了就直连 SMTP，不再走 HTTP 网关。
    #[serde(default)]
    pub host: Option<String>,
    /// SMTP 端口；缺省按 `tls` 推（`ssl` -> 465，其余 587）。
    #[serde(default)]
    pub port: Option<u16>,
    /// SMTP 用户名；与 `password` 成对出现。
    #[serde(default)]
    pub username: Option<String>,
    /// SMTP 口令（支持 `${ENV}` 占位符）。
    #[serde(default)]
    pub password: Option<String>,
    /// SMTP 加密方式：`starttls`（默认）/ `ssl` / `none`。
    /// `none` 只允许指向回环地址——凭据和邮件正文都不该在明文里跑。
    #[serde(default)]
    pub tls: Option<String>,
    /// 鉴权令牌，作为 `Authorization: Bearer <token>` 发送。
    #[serde(default)]
    pub token: Option<String>,
    /// email 的发件人 / sms 的签名等厂商参数。
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub sign: Option<String>,
    /// 附加请求头。
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// 一次投递的输入（已渲染完成）。
#[derive(Clone, Debug)]
pub struct Delivery {
    pub kind: ChannelKind,
    pub config: ChannelConfig,
    pub recipient: String,
    pub title: String,
    pub body: String,
    pub payload: serde_json::Value,
}

/// 投递结果：网关返回的状态码与响应体摘要（进 `sys_notify_record.error` 或日志）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliveryOutcome {
    pub status: u16,
    pub detail: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    #[error("渠道配置非法：{0}")]
    Config(String),
    #[error("模板渲染失败：{0}")]
    Render(String),
    #[error("渠道或模板不存在：{0}")]
    NotFound(String),
    #[error("渠道已停用：{0}")]
    Disabled(String),
    #[error("投递失败：{0}")]
    Transport(String),
    #[error("数据库错误：{0}")]
    Db(String),
}

impl From<sea_orm::DbErr> for NotifyError {
    fn from(e: sea_orm::DbErr) -> Self {
        NotifyError::Db(e.to_string())
    }
}

/// 投递缝：把「渠道类型 + 配置」编码成具体网关请求。
#[async_trait]
pub trait NotifySender: Send + Sync {
    async fn deliver(&self, delivery: &Delivery) -> Result<DeliveryOutcome, NotifyError>;
}

/// 生产实现：`reqwest` HTTP 网关。
pub struct HttpSender {
    client: reqwest::Client,
}

impl Default for HttpSender {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpSender {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("构建 HTTP 客户端"),
        }
    }
}

/// 按渠道类型把投递内容编码成请求体（三通道各自的「网关约定」）。
pub fn encode_body(delivery: &Delivery) -> serde_json::Value {
    match delivery.kind {
        ChannelKind::Webhook => serde_json::json!({
            "title": delivery.title,
            "body": delivery.body,
            "to": delivery.recipient,
            "payload": delivery.payload,
        }),
        ChannelKind::Email => serde_json::json!({
            "from": delivery.config.from,
            "to": delivery.recipient,
            "subject": delivery.title,
            "text": delivery.body,
        }),
        ChannelKind::Sms => serde_json::json!({
            "sign": delivery.config.sign,
            "phone": delivery.recipient,
            "content": delivery.body,
        }),
        ChannelKind::Im => serde_json::json!({
            "to": delivery.recipient,
            "title": delivery.title,
            "text": delivery.body,
            "payload": delivery.payload,
        }),
    }
}

#[async_trait]
impl NotifySender for HttpSender {
    async fn deliver(&self, delivery: &Delivery) -> Result<DeliveryOutcome, NotifyError> {
        if delivery.config.endpoint.trim().is_empty() {
            return Err(NotifyError::Config("endpoint 为空".to_string()));
        }
        let mut req = self
            .client
            .post(&delivery.config.endpoint)
            .json(&encode_body(delivery));
        if let Some(timeout) = delivery.config.timeout_secs {
            req = req.timeout(Duration::from_secs(timeout));
        }
        if let Some(token) = &delivery.config.token {
            req = req.bearer_auth(token);
        }
        for (k, v) in &delivery.config.headers {
            req = req.header(k, v);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| NotifyError::Transport(e.to_string()))?;
        let status = resp.status().as_u16();
        let detail = resp.text().await.ok().filter(|t| !t.is_empty());
        if !(200..300).contains(&status) {
            return Err(NotifyError::Transport(format!(
                "网关返回 {status}：{}",
                detail.clone().unwrap_or_default()
            )));
        }
        Ok(DeliveryOutcome { status, detail })
    }
}

/// 测试用：记录投递内容，不发出真实请求。
#[derive(Default)]
pub struct RecordingSender {
    deliveries: std::sync::Mutex<Vec<Delivery>>,
    fail: bool,
}

impl RecordingSender {
    pub fn new() -> Self {
        Self::default()
    }

    /// 模拟网关故障。
    pub fn failing() -> Self {
        Self {
            deliveries: std::sync::Mutex::new(Vec::new()),
            fail: true,
        }
    }

    pub fn deliveries(&self) -> Vec<Delivery> {
        self.deliveries.lock().unwrap().clone()
    }
}

#[async_trait]
impl NotifySender for RecordingSender {
    async fn deliver(&self, delivery: &Delivery) -> Result<DeliveryOutcome, NotifyError> {
        if self.fail {
            return Err(NotifyError::Transport("模拟网关故障".to_string()));
        }
        self.deliveries.lock().unwrap().push(delivery.clone());
        Ok(DeliveryOutcome {
            status: 200,
            detail: None,
        })
    }
}

/// 便捷构造：`Arc<dyn NotifySender>`。
pub fn shared(sender: impl NotifySender + 'static) -> Arc<dyn NotifySender> {
    Arc::new(sender)
}

impl ChannelConfig {
    /// 该渠道是否直连 SMTP（`email` 且填了 `host`）。
    pub fn uses_smtp(&self) -> bool {
        self.host.as_deref().is_some_and(|h| !h.trim().is_empty())
    }

    /// 配置自检（横切能力）：建/改渠道时就 422，而不是等第一次投递才发现少填了什么。
    /// 返回的问题描述面向运维，指明**字段名**（渠道配置是 JSON，报错里没字段名等于没说）。
    pub fn validate_for(&self, kind: ChannelKind) -> Result<(), String> {
        if kind != ChannelKind::Email {
            if self.endpoint.trim().is_empty() {
                return Err("config.endpoint 不能为空".to_string());
            }
            return Ok(());
        }
        if !self.uses_smtp() {
            if self.endpoint.trim().is_empty() {
                return Err(
                    "email 渠道需要 config.endpoint（HTTP 邮件网关）或 config.host（SMTP）"
                        .to_string(),
                );
            }
            return Ok(());
        }
        let tls = self
            .tls
            .as_deref()
            .unwrap_or("starttls")
            .to_ascii_lowercase();
        if !matches!(tls.as_str(), "starttls" | "ssl" | "none") {
            return Err(format!(
                "config.tls 只能是 starttls / ssl / none，收到 {:?}",
                self.tls
            ));
        }
        let host = self.host.as_deref().unwrap_or_default().trim();
        let loopback = host.eq_ignore_ascii_case("localhost")
            || host.starts_with("127.")
            || host.eq_ignore_ascii_case("::1");
        if tls == "none" && !loopback {
            // 明文 SMTP 会把口令与邮件正文一起送出去；本地 relay 才是 `none` 的适用场景。
            return Err(format!(
                "config.tls = none 只允许回环地址（当前 {host:?}），远端请用 starttls 或 ssl"
            ));
        }
        if self.from.as_deref().unwrap_or("").trim().is_empty() {
            return Err("email 走 SMTP 时 config.from（发件人）不能为空".to_string());
        }
        let user = self.username.as_deref().unwrap_or("").trim();
        let pass = self.password.as_deref().unwrap_or("").trim();
        if user.is_empty() != pass.is_empty() {
            return Err("config.username 与 config.password 必须成对填写".to_string());
        }
        if let Some(port) = self.port
            && !(1..=65535).contains(&port)
        {
            return Err(format!("config.port 越界：{port}"));
        }
        Ok(())
    }
}

/// 按渠道类型把投递分派给具体传输（技术选型：邮件的选型是 lettre/SMTP）。
///
/// 分岔只看**运营实际填了哪个字段**：`email` + `host` 直连 SMTP，其余一律 HTTP 网关。
/// 留 HTTP 邮件网关这条路不是历史包袱——很多部署只有厂商的 HTTP 发信接口，
/// 而 `NotifySender` 的语义本来就不关心底下是哪条协议。
#[derive(Default)]
pub struct RoutingSender {
    http: HttpSender,
    smtp: crate::smtp::SmtpSender,
}

impl RoutingSender {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl NotifySender for RoutingSender {
    async fn deliver(&self, delivery: &Delivery) -> Result<DeliveryOutcome, NotifyError> {
        if delivery.kind == ChannelKind::Email && delivery.config.uses_smtp() {
            return self.smtp.deliver(delivery).await;
        }
        self.http.deliver(delivery).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delivery(kind: ChannelKind) -> Delivery {
        Delivery {
            kind,
            config: ChannelConfig {
                endpoint: "https://example.invalid/hook".to_string(),
                token: Some("t".to_string()),
                from: Some("noreply@cicada.dev".to_string()),
                sign: Some("cicada".to_string()),
                headers: BTreeMap::new(),
                timeout_secs: None,
                ..Default::default()
            },
            recipient: "13800000000".to_string(),
            title: "标题".to_string(),
            body: "正文".to_string(),
            payload: serde_json::json!({ "k": 1 }),
        }
    }

    #[test]
    fn kind_roundtrip() {
        for k in [
            ChannelKind::Sms,
            ChannelKind::Email,
            ChannelKind::Webhook,
            ChannelKind::Im,
        ] {
            assert_eq!(ChannelKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(ChannelKind::parse("pigeon"), None);
    }

    #[test]
    fn each_kind_encodes_its_gateway_shape() {
        assert_eq!(
            encode_body(&delivery(ChannelKind::Sms))["phone"],
            "13800000000"
        );
        assert_eq!(
            encode_body(&delivery(ChannelKind::Email))["subject"],
            "标题"
        );
        assert_eq!(
            encode_body(&delivery(ChannelKind::Webhook))["to"],
            "13800000000"
        );
        // webhook 保留原始 payload 供订阅方自取。
        assert_eq!(
            encode_body(&delivery(ChannelKind::Webhook))["payload"]["k"],
            1
        );
    }
}
