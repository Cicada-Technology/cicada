# ADR 0002：通知渠道统一走 HTTP 网关，不内置 SMTP

- 日期：2026-10-10
- 状态：已采纳

## 背景

M4 出口条件要求「通知三通道可用」（`sms` / `email` / `webhook`）。SMTP 需要额外
依赖（`lettre` 及其 TLS 栈），而生产上短信/邮件大多经厂商 HTTP API 或内部网关投递。

## 结论

三种渠道都以 `ChannelConfig { endpoint, token, from, sign, headers, timeout_secs }`
驱动 `reqwest` POST，各自约定请求体（见 `crates/notify/src/channel.rs::encode_body`）：

- `webhook`：`{ title, body, to, payload }`（保留原始 payload 供订阅方自取）；
- `email`：`{ from, to, subject, text }`；
- `sms`：`{ sign, phone, content }`；
- `im`：同 webhook 形状的群机器人形态。

投递缝是 `NotifySender`（crate 内两实现：`HttpSender` 生产、`RecordingSender` 测试）。
要接 SMTP 或厂商 SDK，按同一 trait 加 adapter，不动 `NotifyService`。

## 代价

- 不内置 SMTP：纯内网 SMTP 中继场景需要自己加 adapter（或让网关代发）。
- 网关请求体是「约定」而非标准，接入具体厂商时可能需要在网关侧做一层映射。
