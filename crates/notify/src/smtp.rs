//! SMTP 投递（选型 `lettre`、 的邮件渠道）。
//!
//! 拆成「先出计划、再交给传输」两步不是绕路：`MailPlan` 是可以直接断言的数据
//! （主机、端口、加密方式、收发件人、正文），而 `lettre` 的传输类型是泛型的，
//! 测试注入 `AsyncStubTransport` 就能在没有 SMTP 服务器的 CI 里验出真实形状。
//!
//! 配置的边界在这里落地：`username` / `password` 支持 `${VAR}` 占位符，
//! 投递那一刻才从环境展开——渠道配置那行 JSON 因此可以只存变量名，不存口令。

use async_trait::async_trait;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::channel::{Delivery, DeliveryOutcome, NotifyError, NotifySender};

/// 一封待投递邮件的完整计划（已与 lettre 无关，纯数据）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailPlan {
    pub host: String,
    pub port: u16,
    /// `starttls` / `ssl` / `none`——保留字符串而不是 lettre 的枚举，
    /// 这样断言与日志都不必依赖第三方类型。
    pub tls: &'static str,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub body: String,
}

/// 环境变量展开（配置「敏感值一律来自环境变量或密钥管理」）。
fn from_env(raw: &str) -> String {
    cicada_infra::config::expand_placeholders(raw, &|k| std::env::var(k).ok())
}

/// 端口缺省按加密方式推：465 是隐式 TLS（SMTPS），587 是提交 + STARTTLS，25 是明文 relay。
fn default_port(tls: &str) -> u16 {
    match tls {
        "ssl" => 465,
        "none" => 25,
        _ => 587,
    }
}

/// 把渠道配置 + 已渲染内容翻成 [`MailPlan`]。
///
/// 校验（`ChannelConfig::validate_for`）应当已经拦下缺项，这里仍然逐项检查：
/// 投递路径也会被绕过 API 直接写入的存量数据触发。
pub fn plan(delivery: &Delivery) -> Result<MailPlan, NotifyError> {
    let cfg = &delivery.config;
    let host = cfg.host.clone().unwrap_or_default().trim().to_string();
    if host.is_empty() {
        return Err(NotifyError::Config("config.host 为空".to_string()));
    }
    let tls_kind: &'static str = match cfg
        .tls
        .as_deref()
        .unwrap_or("starttls")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "ssl" => "ssl",
        "none" => "none",
        // 认不出的值一律按最安全的默认走，而不是静默当 starttls：拼错 `sttls`
        // 的人应当从日志里看到自己写了什么。
        "starttls" => "starttls",
        other => {
            tracing::warn!(tls = other, "无法识别的 config.tls，按 starttls 投递");
            "starttls"
        }
    };
    let from = cfg.from.clone().unwrap_or_default().trim().to_string();
    if from.is_empty() {
        return Err(NotifyError::Config("config.from（发件人）为空".to_string()));
    }
    let username = cfg
        .username
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(from_env);
    let password = cfg
        .password
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .map(from_env);
    Ok(MailPlan {
        port: cfg.port.unwrap_or_else(|| default_port(tls_kind)),
        host,
        tls: tls_kind,
        username,
        password,
        from,
        to: delivery.recipient.trim().to_string(),
        subject: delivery.title.clone(),
        body: delivery.body.clone(),
    })
}

/// 按计划在给定传输上投递。泛型是刻意的：生产用 `AsyncSmtpTransport<Tokio1Executor>`，
/// 测试用 `AsyncStubTransport`，两条走的是同一段组码逻辑。
pub async fn deliver_with<T>(transport: &T, plan: &MailPlan) -> Result<DeliveryOutcome, NotifyError>
where
    T: AsyncTransport + Sync,
    T::Ok: std::fmt::Debug,
    T::Error: std::fmt::Display,
{
    let from: Mailbox = plan
        .from
        .parse()
        .map_err(|e| NotifyError::Config(format!("config.from 不是合法地址：{e}")))?;
    let to: Mailbox = plan
        .to
        .parse()
        .map_err(|e| NotifyError::Config(format!("收件人不是合法地址 {:?}：{e}", plan.to)))?;
    let mail = Message::builder()
        .from(from)
        .to(to)
        .subject(&plan.subject)
        .body(plan.body.clone())
        .map_err(|e| NotifyError::Render(e.to_string()))?;
    let response = transport
        .send(mail)
        .await
        .map_err(|e| NotifyError::Transport(e.to_string()))?;
    // SMTP 没有 HTTP 状态码；`250`（OK）是投递被接受时服务端回的那个数，
    // 用它填 `sys_notify_record.status` 比借用一个 200 更诚实。
    Ok(DeliveryOutcome {
        status: 250,
        detail: Some(format!("{response:?}")),
    })
}

/// 生产实现：真 SMTP，TLS 走 rustls（技术选型「不用 openssl」在这里同样成立）。
#[derive(Default)]
pub struct SmtpSender;

impl SmtpSender {
    fn build_transport(plan: &MailPlan) -> Result<AsyncSmtpTransport<Tokio1Executor>, NotifyError> {
        // 两种 TLS 都要在参数里带上服务器主机名：证书校验靠它，缺了就是「加密但不验身份」。
        let params = TlsParameters::new(plan.host.clone())
            .map_err(|e| NotifyError::Config(format!("TLS 参数构造失败：{e}")))?;
        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&plan.host)
            .port(plan.port)
            .tls(match plan.tls {
                "none" => Tls::None,
                "ssl" => Tls::Wrapper(params),
                _ => Tls::Required(params),
            });
        if let (Some(user), Some(pass)) = (plan.username.clone(), plan.password.clone()) {
            builder = builder.credentials(Credentials::new(user, pass));
        }
        Ok(builder.build())
    }
}

#[async_trait]
impl NotifySender for SmtpSender {
    async fn deliver(&self, delivery: &Delivery) -> Result<DeliveryOutcome, NotifyError> {
        let plan = plan(delivery)?;
        let transport = Self::build_transport(&plan)?;
        deliver_with(&transport, &plan).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::{ChannelConfig, ChannelKind};
    use lettre::transport::stub::AsyncStubTransport;

    fn delivery(host: Option<&str>, tls: Option<&str>) -> Delivery {
        Delivery {
            kind: ChannelKind::Email,
            config: ChannelConfig {
                endpoint: String::new(),
                host: host.map(str::to_string),
                port: None,
                username: Some("notify@example.com".to_string()),
                password: Some("hunter2".to_string()),
                tls: tls.map(str::to_string),
                from: Some("NoReply <noreply@example.com>".to_string()),
                sign: None,
                token: None,
                headers: Default::default(),
                timeout_secs: None,
            },
            recipient: "zhangsan@corp.com".to_string(),
            title: "导出完成".to_string(),
            body: "link: /api/v1/system/files/9".to_string(),
            payload: serde_json::json!({ "task_id": 9 }),
        }
    }

    #[test]
    fn plan_maps_tls_mode_to_the_conventional_port() {
        let ssl = plan(&delivery(Some("smtp.example.com"), Some("ssl"))).unwrap();
        assert_eq!(
            (ssl.host.as_str(), ssl.port, ssl.tls),
            ("smtp.example.com", 465, "ssl")
        );
        let starttls = plan(&delivery(Some("smtp.example.com"), None)).unwrap();
        assert_eq!((starttls.port, starttls.tls), (587, "starttls"));
        // 回环 relay 才允许 `none`（校验在 ChannelConfig 里，这里只验证映射）。
        let plain = plan(&delivery(Some("localhost"), Some("none"))).unwrap();
        assert_eq!((plain.port, plain.tls), (25, "none"));
    }

    #[test]
    fn plan_refuses_to_invent_a_host_or_sender() {
        let no_host = plan(&delivery(None, None)).unwrap_err();
        assert!(
            matches!(&no_host, NotifyError::Config(m) if m.contains("host")),
            "{no_host}"
        );
        let mut d = delivery(Some("smtp.example.com"), None);
        d.config.from = Some("   ".to_string());
        let no_from = plan(&d).unwrap_err();
        assert!(
            matches!(&no_from, NotifyError::Config(m) if m.contains("from")),
            "{no_from}"
        );
    }

    /// 真实组码：收发件人、主题、正文都要进信封与报文，且 `payload` 不参与（那是站内信的料）。
    #[tokio::test]
    async fn the_message_carries_the_rendered_content() {
        let stub = AsyncStubTransport::new_ok();
        let p = plan(&delivery(Some("smtp.example.com"), Some("ssl"))).unwrap();
        let out = deliver_with(&stub, &p).await.unwrap();
        assert_eq!(out.status, 250, "SMTP 的成功以 250 记账");
        let messages = stub.messages().await;
        assert_eq!(messages.len(), 1);
        let (envelope, wire) = messages.first().expect("stub 应当留下这一封");
        // 信封用的是纯地址（显示名只进信头）：SMTP 的 MAIL FROM 不接受显示名。
        assert_eq!(
            envelope.from().expect("信封要有发件人").to_string(),
            "noreply@example.com"
        );
        assert_eq!(envelope.to().len(), 1);
        // 非 ASCII 主题按 RFC 2047 编码进信头（`=?utf-8?b?…?=`），不是原样出现——
        // 断言写中文字面量会「看起来该过」却永远过不了，所以断的是编码后的形状。
        assert!(
            wire.contains("Subject: =?utf-8?b?5a+85Ye65a6M5oiQ?="),
            "主题没进信头：{wire}"
        );
        assert!(
            wire.contains("To: zhangsan@corp.com"),
            "收件人没进信头：{wire}"
        );
        assert!(
            wire.contains("link: /api/v1/system/files/9"),
            "正文没进报文：{wire}"
        );
        assert!(
            !wire.contains("task_id"),
            "payload 不该被塞进邮件正文：{wire}"
        );
    }

    #[tokio::test]
    async fn a_bad_address_is_a_config_error_not_a_transport_failure() {
        let stub = AsyncStubTransport::new_ok();
        let mut p = plan(&delivery(Some("smtp.example.com"), None)).unwrap();
        p.to = "not-an-address".to_string();
        let err = deliver_with(&stub, &p).await.unwrap_err();
        assert!(matches!(err, NotifyError::Config(_)), "{err:?}");

        let failing = AsyncStubTransport::new_error();
        let p = plan(&delivery(Some("smtp.example.com"), None)).unwrap();
        let err = deliver_with(&failing, &p).await.unwrap_err();
        assert!(matches!(err, NotifyError::Transport(_)), "{err:?}");
    }

    /// 配置校验（横切能力）：建渠道时就拦住的几条。
    #[test]
    fn channel_validation_catches_the_shape_mistakes() {
        let ok = delivery(Some("smtp.example.com"), None).config;
        ok.validate_for(ChannelKind::Email)
            .expect("完整配置应当通过");

        let remote_plain = ChannelConfig {
            host: Some("smtp.example.com".to_string()),
            tls: Some("none".to_string()),
            ..ok.clone()
        };
        let err = remote_plain.validate_for(ChannelKind::Email).unwrap_err();
        assert!(err.contains("回环"), "明文只该允许本地 relay：{err}");

        let half_creds = ChannelConfig {
            username: None,
            ..ok.clone()
        };
        let err = half_creds.validate_for(ChannelKind::Email).unwrap_err();
        assert!(err.contains("成对"), "{err}");

        let no_sender = ChannelConfig {
            from: None,
            ..ok.clone()
        };
        let err = no_sender.validate_for(ChannelKind::Email).unwrap_err();
        assert!(err.contains("from"), "{err}");

        // 只配 endpoint 的邮件渠道仍然合法（HTTP 邮件网关那条路）。
        let gateway = ChannelConfig {
            endpoint: "https://mail.internal/send".to_string(),
            host: None,
            tls: None,
            from: ok.from.clone(),
            username: None,
            password: None,
            port: None,
            sign: None,
            token: None,
            headers: Default::default(),
            timeout_secs: None,
        };
        gateway.validate_for(ChannelKind::Email).expect("网关形状");
        assert!(!gateway.uses_smtp());
        assert!(ok.uses_smtp());

        // 非邮件渠道：endpoint 仍是硬要求。
        let sms = ChannelConfig {
            endpoint: String::new(),
            ..gateway
        };
        let err = sms.validate_for(ChannelKind::Sms).unwrap_err();
        assert!(err.contains("endpoint"), "{err}");
    }
}
