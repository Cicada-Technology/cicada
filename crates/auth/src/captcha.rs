//! 登录图形验证码：连续登录失败后强制校验，对应参考实现
//! `sys.account.captchaEnabled`。答案只存服务端（Redis / 内存），一次性消费，
//! 接口只回传 `captcha_id` 与图片 base64，不回传答案。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use captcha::Captcha;
use uuid::Uuid;

use cicada_infra::Cache;

use crate::error::AuthError;

/// 验证码参数（配置 `auth.captcha`）。
#[derive(Clone, Copy, Debug)]
pub struct CaptchaParams {
    /// 总开关。
    pub enabled: bool,
    /// 字符数。
    pub length: u32,
    /// 答案有效期（秒）。
    pub ttl_secs: u64,
    /// 连续失败多少次后强制要求验证码；`0` 表示登录一律要验证码。
    pub required_after: u32,
}

impl Default for CaptchaParams {
    fn default() -> Self {
        Self {
            enabled: true,
            length: 4,
            ttl_secs: 120,
            required_after: 3,
        }
    }
}

/// 验证码答案存储的窄接口。生产用 Redis，测试用内存实现。
#[async_trait]
pub trait CaptchaStore: Send + Sync {
    /// 写入答案，TTL 秒后自动过期。
    async fn issue(&self, id: &str, code: &str, ttl_secs: u64) -> Result<(), AuthError>;
    /// 一次性取出：读到即删除，防止同一验证码多次尝试。
    async fn consume(&self, id: &str) -> Result<Option<String>, AuthError>;
}

fn answer_key(id: &str) -> String {
    format!("captcha_ans:{id}")
}

/// Redis 实现：答案键 `captcha_ans:{uuid}`，TTL 即有效期。
#[derive(Clone)]
pub struct RedisCaptchaStore {
    cache: Cache,
}

impl RedisCaptchaStore {
    pub fn new(cache: Cache) -> Self {
        Self { cache }
    }
}

#[async_trait]
impl CaptchaStore for RedisCaptchaStore {
    async fn issue(&self, id: &str, code: &str, ttl_secs: u64) -> Result<(), AuthError> {
        self.cache
            .set_ex(&answer_key(id), code, ttl_secs)
            .await
            .map_err(|e| AuthError::Session(e.to_string()))
    }

    async fn consume(&self, id: &str) -> Result<Option<String>, AuthError> {
        let key = answer_key(id);
        let code = self
            .cache
            .get(&key)
            .await
            .map_err(|e| AuthError::Session(e.to_string()))?;
        if code.is_some() {
            self.cache
                .del(&key)
                .await
                .map_err(|e| AuthError::Session(e.to_string()))?;
        }
        Ok(code)
    }
}

/// 内存实现：单测 / 无 Redis 环境用。
#[derive(Default)]
pub struct InMemoryCaptchaStore {
    inner: Mutex<HashMap<String, (String, Instant)>>,
}

impl InMemoryCaptchaStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl CaptchaStore for InMemoryCaptchaStore {
    async fn issue(&self, id: &str, code: &str, ttl_secs: u64) -> Result<(), AuthError> {
        let mut guard = self.inner.lock().unwrap();
        guard.insert(
            id.to_string(),
            (
                code.to_string(),
                Instant::now() + Duration::from_secs(ttl_secs),
            ),
        );
        Ok(())
    }

    async fn consume(&self, id: &str) -> Result<Option<String>, AuthError> {
        let entry = self.inner.lock().unwrap().remove(id);
        Ok(entry
            .filter(|(_, until)| Instant::now() < *until)
            .map(|(code, _)| code))
    }
}

/// 验证码服务：生成（id + base64 PNG）与一次性校验。
pub struct CaptchaService {
    store: std::sync::Arc<dyn CaptchaStore>,
    params: CaptchaParams,
}

impl CaptchaService {
    pub fn new(store: std::sync::Arc<dyn CaptchaStore>, params: CaptchaParams) -> Self {
        Self { store, params }
    }

    pub fn params(&self) -> CaptchaParams {
        self.params
    }

    /// 按当前连续失败次数判断本次登录是否必须带验证码。
    pub fn is_required(&self, failures: u32) -> bool {
        self.params.enabled && failures >= self.params.required_after
    }

    /// 生成验证码：返回 `(captcha_id, base64 PNG)`；base64 不含 data URI 前缀。
    pub async fn generate(&self) -> Result<(String, String), AuthError> {
        // `Captcha` 不是 `Send`，渲染必须在进入 await 之前完成并离开作用域。
        let (code, img) = {
            let mut cap = Captcha::new();
            cap.add_chars(self.params.length);
            let code = cap.chars_as_string();
            let img = cap
                .as_base64()
                .ok_or_else(|| AuthError::Session("验证码图片生成失败".to_string()))?;
            (code, img)
        };
        let id = Uuid::new_v4().to_string();
        self.store.issue(&id, &code, self.params.ttl_secs).await?;
        Ok((id, img))
    }

    /// 校验并消费验证码；空值按失败处理（fail closed）。
    pub async fn verify(&self, id: Option<&str>, code: Option<&str>) -> Result<bool, AuthError> {
        let (Some(id), Some(code)) = (id.filter(|s| !s.is_empty()), code.filter(|s| !s.is_empty()))
        else {
            return Ok(false);
        };
        match self.store.consume(id).await? {
            Some(expected) => Ok(expected.eq_ignore_ascii_case(code.trim())),
            None => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> CaptchaService {
        CaptchaService::new(
            std::sync::Arc::new(InMemoryCaptchaStore::new()),
            CaptchaParams::default(),
        )
    }

    #[tokio::test]
    async fn generated_captcha_has_answer_and_image() {
        let svc = service();
        let (id, img) = svc.generate().await.unwrap();
        assert!(!id.is_empty());
        assert!(!img.is_empty());
        let stored = svc.store.consume(&id).await.unwrap().expect("答案应已写入");
        assert_eq!(stored.chars().count(), 4);
    }

    #[tokio::test]
    async fn verify_is_case_insensitive_and_one_shot() {
        let svc = service();
        svc.store.issue("fixed", "AbCd", 60).await.unwrap();
        assert!(svc.verify(Some("fixed"), Some("abcd")).await.unwrap());
        // 一次性消费：同一个 id 不能再用。
        assert!(!svc.verify(Some("fixed"), Some("abcd")).await.unwrap());
    }

    #[tokio::test]
    async fn wrong_code_and_missing_are_rejected() {
        let svc = service();
        let (id, _) = svc.generate().await.unwrap();
        assert!(!svc.verify(Some(&id), Some("0000")).await.unwrap());
        assert!(!svc.verify(None, None).await.unwrap());
        assert!(!svc.verify(Some(""), Some("1")).await.unwrap());
    }

    #[test]
    fn required_after_consecutive_failures() {
        let svc = service(); // required_after = 3
        assert!(!svc.is_required(2));
        assert!(svc.is_required(3));
        assert!(svc.is_required(9));

        let off = CaptchaService::new(
            std::sync::Arc::new(InMemoryCaptchaStore::new()),
            CaptchaParams {
                enabled: false,
                ..Default::default()
            },
        );
        assert!(!off.is_required(100));

        let always = CaptchaService::new(
            std::sync::Arc::new(InMemoryCaptchaStore::new()),
            CaptchaParams {
                required_after: 0,
                ..Default::default()
            },
        );
        assert!(always.is_required(0));
    }
}
