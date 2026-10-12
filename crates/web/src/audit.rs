//! 审计中间件：读路由上 `#[audit]` 声明的 `RouteAudit` 元数据，请求结束后写
//! `sys_oper_log`。
//!
//! 审计 sink 经应用级 `Extension<Arc<dyn AuditSink>>` 注入，因此路由声明器不必
//! 持有 `AppState`。写日志失败不影响响应（`AuditSink` 实现内部只 `warn`）。

use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::{Extension, Request};
use axum::middleware::Next;
use axum::response::Response;

use serde_json::Value;

use axum::body::{Body, Bytes};
use axum::http::header;
use cicada_audit::{AuditSink, ChangeLog, OperLog};
use cicada_kernel::RequestContext;
use futures_util::StreamExt;

use crate::routes::RouteAudit;

/// 字段级变更留痕。
///
/// 快照由调用点显式给出「这次接口改动了哪些列」，而不是把整行 Model 序列化：
/// 一是不管实体加什么列都不会把凭据带进留痕表，二是留痕内容与接口语义一致。
/// 差异计算与凭据滤除在 `cicada_audit::change` 里；写失败只 `warn`，不影响业务响应。
pub async fn record_change(
    sink: &std::sync::Arc<dyn AuditSink>,
    ctx: &RequestContext,
    table: &str,
    record_id: impl Into<String>,
    before: Value,
    after: Value,
) {
    let tenant_id = ctx.tenant_id.get();
    let oper_id = ctx.principal.as_ref().map(|p| p.user_id.get());
    let trace_id = Some(ctx.trace_id.as_str().to_string());
    sink.record_change(ChangeLog::updated(
        tenant_id, table, record_id, &before, &after, oper_id, trace_id,
    ))
    .await;
}

pub async fn audit_middleware(
    Extension(sink): Extension<Arc<dyn AuditSink>>,
    mut req: Request,
    next: Next,
) -> Response {
    let meta = req.extensions().get::<RouteAudit>().map(|a| a.0);
    let ctx = req.extensions().get::<RequestContext>().cloned();
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let query = req.uri().query().map(|q| q.to_string());
    let user_agent = req
        .headers()
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|v| clip(v, USER_AGENT_LEN));
    // 审计设计要「脱敏后的请求参数」，而查询串里通常什么也没有——真正的内容在请求体里。
    let capture = if meta.is_some() {
        attach_body_capture(&mut req).await
    } else {
        None
    };

    let started = Instant::now();
    let resp = next.run(req).await;
    let biz_code = resp
        .headers()
        .get(crate::problem::BIZ_CODE_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_string());

    if let Some(meta) = meta {
        let status = resp.status();
        let (tenant_id, operator, ip) = match &ctx {
            Some(c) => (
                c.tenant_id.get(),
                c.principal.as_ref().map(|p| p.user_name.clone()),
                Some(c.ip.to_string()).filter(|s| s != "0.0.0.0"),
            ),
            None => (0, None, None),
        };
        let failed = !status.is_success();
        sink.record_oper(OperLog {
            tenant_id,
            title: meta.title(),
            business_type: meta.business_type,
            method: format!("{} {}", method.as_str(), clip(&path, 200)),
            request_method: method.as_str().to_string(),
            operator,
            oper_url: path,
            oper_ip: ip,
            oper_param: query,
            status: if failed { 1 } else { 0 },
            error_msg: failed.then(|| status.as_str().to_string()),
            cost_ms: started.elapsed().as_millis() as i64,
            user_agent,
            // 业务码只有失败响应才有（成功响应不带 problem+json 的头）。
            biz_code: biz_code.filter(|_| failed),
            req_body: capture.as_ref().and_then(Capture::masked_body),
        })
        .await;
    }

    resp
}

/// 请求体进审计的上限（字节）：超过就整段不记，而不是记一半。
const MAX_AUDIT_BODY: usize = 8 * 1024;
/// `user_agent` 列宽（与迁移 `m20261019_000017_oper_log_context` 一致）。
const USER_AGENT_LEN: usize = 512;
/// `req_body` 列宽。
const REQ_BODY_LEN: usize = 4096;

/// 键名含这些片段的值不进审计（大小写不敏感、子串匹配）。
///
/// 宁可多遮一点也不漏：`pwd` / `pwd2` 这种命名在存量系统里很常见，
/// 而审计的读者是「查谁改了什么」的人，不是需要看到口令的人。
const SECRET_HINTS: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "secret",
    "token",
    "credential",
    "otp",
    "totp",
    "mfa",
    "private_key",
    "access_key",
    "apikey",
    "api_key",
    "authorization",
    "refresh",
    "jwt",
    "signature",
    "sign",
];

/// 按 `char` 边界截断，绝不在多字节字符中间切断（PG 会直接拒绝无效 UTF-8）。
fn clip(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut out: String = value.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn mask_secrets(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                let lowered = key.to_ascii_lowercase();
                if SECRET_HINTS.iter().any(|hint| lowered.contains(hint)) {
                    if !child.is_null() {
                        *child = serde_json::Value::String("***".to_string());
                    }
                } else {
                    mask_secrets(child);
                }
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(mask_secrets),
        other => {
            let _ = other;
        }
    }
}

/// 请求体采集方式。两者共同点只有一个：**都不改变 handler 收到的字节**。
enum Capture {
    /// 声明了 `Content-Length` 且在上限内：整段读进内存，再把同一份字节交回请求。
    /// 这条路可信是因为 hyper 按声明长度框定流，声明之内不可能读超；于是权限守卫
    /// 短路、handler 压根没碰请求体的那些失败尝试，也照样留下参数（审计设计要记的
    /// 正是「谁试图改什么」）。
    Buffered(Vec<u8>),
    /// 没声明长度（chunked）、或声明超过上限：与主流程**共用同一个流**的旁路记录器。
    /// 这两种只能走旁路——Body 是一次性的流，「读出来才发现超限」没法把字节变回去。
    Tee(BodyTee),
}

/// 与主流程共用同一个流的旁路记录器：每块原样转发给 handler，同时留副本。
/// 超限或流出错时作废的是**副本**，请求结果不受影响。
#[derive(Clone, Default)]
struct BodyTee(Arc<Mutex<TeeState>>);

#[derive(Default)]
struct TeeState {
    bytes: Vec<u8>,
    /// 副本作废：超限，或流本身出错。之后不再攒字节。
    given_up: bool,
}

impl BodyTee {
    fn observe(&self, item: &Result<Bytes, axum::Error>) {
        let mut state = self.0.lock().expect("请求体记录器不该中毒");
        if state.given_up {
            return;
        }
        match item {
            Ok(bytes) if state.bytes.len() + bytes.len() <= MAX_AUDIT_BODY => {
                state.bytes.extend_from_slice(bytes);
            }
            // 宁可一条都不留，也不留一份截断的、看起来却像完整 JSON 的参数。
            Ok(_) | Err(_) => {
                state.given_up = true;
                state.bytes.clear();
            }
        }
    }

    /// 副本是否还有效；无效（超限/出错）时返回 `None`。
    fn copy(&self) -> Option<Vec<u8>> {
        let state = self.0.lock().expect("请求体记录器不该中毒");
        (!state.given_up).then(|| state.bytes.clone())
    }
}

impl Capture {
    /// 响应处理完之后才取：旁路的字节是 handler 读流时一块块攒进来的。
    fn masked_body(&self) -> Option<String> {
        let bytes = match self {
            Self::Buffered(bytes) => Some(bytes.clone()),
            Self::Tee(tee) => tee.copy(),
        }?;
        render_masked(&bytes)
    }
}

/// 脱敏 + 按列宽截断；空体不记。
fn render_masked(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    let Ok(mut parsed) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        // 声明了 JSON 却不是合法 JSON：长度已限死在 8KB 内，原样记下来也比丢掉强
        // （脱敏无从谈起，但这类体本身就不是结构化的凭据载体）。
        return Some(clip(&String::from_utf8_lossy(bytes), REQ_BODY_LEN));
    };
    mask_secrets(&mut parsed);
    Some(clip(
        &serde_json::to_string(&parsed).unwrap_or_default(),
        REQ_BODY_LEN,
    ))
}

/// 给 JSON 请求体接上采集器；表单 / multipart / 二进制不接——它们不是结构化凭据
/// 载体，接上只是白白多一份副本。
async fn attach_body_capture(req: &mut Request) -> Option<Capture> {
    let is_json = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if !is_json {
        return None;
    }
    let declared = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|len| (len as usize) <= MAX_AUDIT_BODY) {
        let taken = std::mem::take(req.body_mut());
        return Some(match axum::body::to_bytes(taken, MAX_AUDIT_BODY).await {
            Ok(buf) => {
                *req.body_mut() = Body::from(buf.clone());
                Capture::Buffered(buf.to_vec())
            }
            // 读断了：连接本身已经坏掉，handler 撞的是同一个错误，审计只放弃副本。
            Err(_) => {
                *req.body_mut() = Body::empty();
                Capture::Buffered(Vec::new())
            }
        });
    }
    let tee = BodyTee::default();
    let shared = tee.clone();
    let stream = std::mem::take(req.body_mut())
        .into_data_stream()
        .map(move |item| {
            shared.observe(&item);
            item
        });
    *req.body_mut() = Body::from_stream(stream);
    Some(Capture::Tee(tee))
}

#[cfg(test)]
mod tests {
    use axum::body::Bytes;

    use super::*;

    /// 按给定分块发体的请求。`declared` 决定是否带 `Content-Length`——
    /// 不带就等价于 HTTP/1.1 的 chunked 上传。
    fn json_request(chunks: &[&str], declared: bool) -> Request {
        let body = Body::from_stream(futures_util::stream::iter(
            chunks
                .iter()
                .map(|c| Ok::<_, std::io::Error>(Bytes::from(c.to_string())))
                .collect::<Vec<_>>(),
        ));
        let text: String = chunks.concat();
        let mut builder = Request::builder()
            .method("POST")
            .uri("/api/v1/system/roles")
            .header("content-type", "application/json");
        if declared {
            builder = builder.header("content-length", text.len());
        }
        builder.body(body).unwrap()
    }

    /// 走一遍「接采集器 → handler 把体读完」，返回 handler 实际收到的字节与采集器。
    async fn deliver(mut req: Request) -> (Vec<u8>, Option<Capture>) {
        let capture = attach_body_capture(&mut req).await;
        let bytes = axum::body::to_bytes(std::mem::take(req.body_mut()), 1024 * 1024)
            .await
            .expect("handler 侧必须能读完")
            .to_vec();
        (bytes, capture)
    }

    /// 旁路记录器与 handler 共用同一个流：跨块的 JSON 采得到、凭据遮得住，字节一块不少。
    #[tokio::test]
    async fn tee_masks_json_without_touching_the_bytes() {
        let text = r#"{"role_name":"审计上下文","password":"s3cr3t","spec":{"clientSecret":"k"},"list":[{"accessToken":"t"}]}"#;
        let (head, tail) = text.split_at(text.find("password").unwrap());
        let (bytes, capture) = deliver(json_request(&[head, tail], false)).await;
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            text,
            "转发的字节必须原样"
        );

        let recorded = capture
            .expect("chunked 请求体也该采到")
            .masked_body()
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&recorded).unwrap();
        assert_eq!(parsed["role_name"], "审计上下文");
        assert_eq!(parsed["password"], "***");
        assert_eq!(parsed["spec"]["clientSecret"], "***");
        assert_eq!(parsed["list"][0]["accessToken"], "***");
    }

    /// 超限（或读流出错）时作废的是副本，请求结果不受影响。
    #[tokio::test]
    async fn oversized_body_gives_up_the_copy_but_still_delivers() {
        let big = "x".repeat(MAX_AUDIT_BODY + 16);
        let chunked = [r#"{"role_name":""#, big.as_str(), r#""}"#];
        let prefix = r#"{"role_name":""#;
        let (bytes, capture) = deliver(json_request(&chunked, false)).await;
        assert_eq!(bytes.len(), prefix.len() + MAX_AUDIT_BODY + 16 + 2);
        assert!(
            capture.expect("已接上记录器").masked_body().is_none(),
            "超限不记半份"
        );
    }

    /// 声明了长度的请求走「读进内存再交回」：权限守卫短路、handler 一字节都没读时，
    /// 这次尝试的参数照样留在审计里。
    #[tokio::test]
    async fn declared_body_survives_a_handler_that_never_reads_it() {
        let text = r#"{"role_name":"越权尝试","password":"s3cr3t"}"#;
        let mut req = json_request(&[text], true);
        let capture = attach_body_capture(&mut req)
            .await
            .expect("声明长度的 JSON 该走缓冲");
        drop(std::mem::take(req.body_mut()));

        let recorded = capture.masked_body().expect("被短路的请求也要留下参数");
        assert!(!recorded.contains("s3cr3t"), "{recorded}");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&recorded).unwrap()["role_name"],
            "越权尝试"
        );
    }

    /// 非 JSON 体不接采集器（表单 / 上传不该攒副本），但体本身照常送达。
    #[tokio::test]
    async fn non_json_body_is_not_recorded() {
        let mut req = json_request(&["a=1&password=s3cr3t"], true);
        req.headers_mut().insert(
            "content-type",
            "application/x-www-form-urlencoded".parse().unwrap(),
        );
        let (bytes, capture) = deliver(req).await;
        assert_eq!(String::from_utf8(bytes).unwrap(), "a=1&password=s3cr3t");
        assert!(capture.is_none());
    }
}
