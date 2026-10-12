//! JWT 访问令牌（HS256 开发用；生产切 EdDSA）+ 不透明刷新令牌。

use jsonwebtoken::{
    Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, decode_header, encode,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AuthError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenClaims {
    /// 用户 ID（`sub`）。
    pub sub: String,
    /// 租户 ID（`tid`）。
    pub tid: i64,
    /// 会话 ID（`jti`）。
    pub jti: String,
    pub username: String,
    /// 过期时间（Unix 秒）。
    pub exp: i64,
    /// 签发时间（Unix 秒）。
    pub iat: i64,
}

/// 「绑定票据」的 claims（OAuth 首次登录未绑定 → 进入绑定）。
///
/// `purpose` 固定为 `bind`，字段与 [`TokenClaims`] 完全不同，因此访问令牌与票据
/// **不可互换**（互相解析都会因缺字段失败）。票据只证明「某外部身份已被提供方确认」，
/// 不含任何账号身份，且短时效。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingTicketClaims {
    /// 固定 `bind`。
    pub purpose: String,
    pub tid: i64,
    /// 身份类型（`WeChatOpen` / `Oidc` …）。
    pub itype: String,
    /// 提供方标识（配置里的 `code`，如 `wechat`）。
    pub provider: String,
    /// 外部标识（微信 `unionid`/`openid`，OIDC 为 `{code}:{sub}`）。
    pub identifier: String,
    #[serde(default)]
    pub nickname: Option<String>,
    pub iat: i64,
    pub exp: i64,
}

/// 技术选型的算法名 → jsonwebtoken 的 `Algorithm`。
fn parse_alg(name: &str) -> Result<Algorithm, AuthError> {
    match name.trim().to_ascii_uppercase().as_str() {
        "EDDSA" | "ED25519" => Ok(Algorithm::EdDSA),
        "HS256" | "" => Ok(Algorithm::HS256),
        other => Err(AuthError::Config(format!(
            "auth.jwt.alg: 不支持 {other:?}（可用：EdDSA / HS256）"
        ))),
    }
}

/// 一把签名密钥：算法 + 私钥/共享密钥 + `kid`。
#[derive(Clone)]
struct Signer {
    alg: Algorithm,
    key: EncodingKey,
    kid: Option<String>,
}

/// 一把验签密钥。`kid = None` 表示「不认 kid」——HS256 单密钥的老写法就是这种，
/// 此时头的 `alg` 仍然是硬过滤（否则 alg=none/降级攻击就有了入口）。
#[derive(Clone)]
struct Verifier {
    alg: Algorithm,
    kid: Option<String>,
    key: DecodingKey,
}

/// JWT 签发与校验。
///
/// 支持技术选型的两种算法：**EdDSA(Ed25519) 优先**、HS256 同密钥回退，
/// 以及认证设计要求的 `kid` 多密钥轮换——签名只用当前那把（`auth.jwt.kid`），
/// 验签按头里的 `kid` 在「当前 + `auth.jwt.keys`」里挑，所以换密钥时旧 token
/// 在自身有效期内仍可验，不必一刀切把所有人踢下线。
#[derive(Clone)]
pub struct TokenService {
    signer: Signer,
    verifiers: Vec<Verifier>,
    access_ttl_secs: i64,
}

impl TokenService {
    /// HS256 单密钥（开发默认与测试夹具的形状）。
    pub fn new(secret: impl Into<String>, access_ttl_secs: i64) -> Self {
        let secret = secret.into();
        let key = secret.as_bytes().to_vec();
        Self {
            signer: Signer {
                alg: Algorithm::HS256,
                key: EncodingKey::from_secret(&key),
                kid: None,
            },
            verifiers: vec![Verifier {
                alg: Algorithm::HS256,
                kid: None,
                key: DecodingKey::from_secret(&key),
            }],
            access_ttl_secs,
        }
    }

    /// 按配置的 `auth.jwt` 装配。
    pub fn from_jwt(
        cfg: &cicada_infra::config::JwtConfig,
        access_ttl_secs: i64,
    ) -> Result<Self, AuthError> {
        let alg = parse_alg(&cfg.alg)?;
        let kid = (!cfg.kid.trim().is_empty()).then(|| cfg.kid.trim().to_string());
        let signer = match alg {
            Algorithm::EdDSA => Signer {
                alg,
                key: EncodingKey::from_ed_pem(cfg.private_key.expose_secret().as_bytes())
                    .map_err(|e| AuthError::Config(format!("auth.jwt.private_key: {e}")))?,
                kid: kid.clone(),
            },
            _ => Signer {
                alg,
                key: EncodingKey::from_secret(cfg.private_key.expose_secret().as_bytes()),
                kid: kid.clone(),
            },
        };
        let mut verifiers = vec![Verifier {
            alg,
            kid: kid.clone(),
            key: match alg {
                Algorithm::EdDSA => {
                    DecodingKey::from_ed_pem(cfg.public_key.expose_secret().as_bytes())
                        .map_err(|e| AuthError::Config(format!("auth.jwt.public_key: {e}")))?
                }
                _ => DecodingKey::from_secret(cfg.private_key.expose_secret().as_bytes()),
            },
        }];
        for key in &cfg.keys {
            let alg = if key.alg.trim().is_empty() {
                alg
            } else {
                parse_alg(&key.alg)?
            };
            let secret = key.key.expose_secret();
            // HS256 允许留空表示「与顶层同一把共享密钥」（换 kid 不换密钥的写法）。
            let raw = if secret.trim().is_empty() {
                cfg.private_key.expose_secret().as_str()
            } else {
                secret.as_str()
            };
            let decoding = match alg {
                Algorithm::EdDSA => DecodingKey::from_ed_pem(raw.as_bytes())
                    .map_err(|e| AuthError::Config(format!("auth.jwt.keys[{}]: {e}", key.kid)))?,
                _ => DecodingKey::from_secret(raw.as_bytes()),
            };
            verifiers.push(Verifier {
                alg,
                kid: Some(key.kid.trim().to_string()),
                key: decoding,
            });
        }
        Ok(Self {
            signer,
            verifiers,
            access_ttl_secs,
        })
    }

    /// 当前签名密钥的 `kid`（观测与模块契约的 JWKS 导出用）。
    pub fn key_id(&self) -> Option<&str> {
        self.signer.kid.as_deref()
    }

    /// 参与的验签密钥数量（轮换期会是 2+）。
    pub fn verifier_count(&self) -> usize {
        self.verifiers.len()
    }

    pub fn sign_access(
        &self,
        user_id: i64,
        tenant_id: i64,
        session_id: &str,
        username: &str,
    ) -> Result<String, AuthError> {
        let now = chrono::Utc::now().timestamp();
        let claims = TokenClaims {
            sub: user_id.to_string(),
            tid: tenant_id,
            jti: session_id.to_string(),
            username: username.to_string(),
            exp: now + self.access_ttl_secs,
            iat: now,
        };
        Ok(encode(&self.header(), &claims, &self.signer.key)?)
    }

    pub fn verify_access(&self, token: &str) -> Result<TokenClaims, AuthError> {
        self.verify(token)
    }

    pub fn access_ttl_secs(&self) -> i64 {
        self.access_ttl_secs
    }

    /// 签发外部身份绑定票据（与访问令牌同密钥、不同 claims 结构）。
    pub fn sign_binding_ticket(
        &self,
        tenant_id: i64,
        identity_type: &str,
        provider: &str,
        identifier: &str,
        nickname: Option<&str>,
        ttl_secs: u64,
    ) -> Result<String, AuthError> {
        let now = chrono::Utc::now().timestamp();
        let claims = BindingTicketClaims {
            purpose: BINDING_TICKET_PURPOSE.to_string(),
            tid: tenant_id,
            itype: identity_type.to_string(),
            provider: provider.to_string(),
            identifier: identifier.to_string(),
            nickname: nickname.map(str::to_string),
            iat: now,
            exp: now + ttl_secs as i64,
        };
        Ok(encode(&self.header(), &claims, &self.signer.key)?)
    }

    /// 校验绑定票据：签名、有效期与 `purpose` 三者都要过。
    ///
    /// 票据短时效且签发/校验都在同一进程，因此**不留时钟宽限**（默认 60s 宽限会让刚过期的
    /// 票据仍然可用，与「短时效」的初衷相悖）。
    pub fn verify_binding_ticket(&self, token: &str) -> Result<BindingTicketClaims, AuthError> {
        let claims: BindingTicketClaims = self.verify_with_leeway(token, 0)?;
        if claims.purpose != BINDING_TICKET_PURPOSE {
            return Err(AuthError::InvalidToken);
        }
        Ok(claims)
    }

    /// 当前签名用的 JWT 头：算法 + `kid`（认证设计）。
    fn header(&self) -> Header {
        let mut header = Header::new(self.signer.alg);
        header.kid = self.signer.kid.clone();
        header
    }

    /// 按头里的 `kid` + `alg` 挑验签密钥，然后解码。
    ///
    /// 先 `decode_header` 再选密钥，是为了**由头决定用哪套校验**：反过来（固定算法
    /// 再解码）会让携带异法头的 token 在库内部报含糊错误，也更容易被 alg 混淆攻击利用。
    fn verify_with_leeway<T>(&self, token: &str, leeway: u64) -> Result<T, AuthError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let header = decode_header(token).map_err(|_| AuthError::InvalidToken)?;
        let Some(matched) = self.verifiers.iter().find(|v| {
            v.alg == header.alg
                && match (&v.kid, header.kid.as_deref()) {
                    // 密钥没标 kid：只服务「头里也没 kid」的旧形状，不能反过来放宽。
                    (None, None) => true,
                    (Some(kid), Some(found)) => kid == found,
                    _ => false,
                }
        }) else {
            // 认不出 kid/算法 = 不是这个服务该验的 token。报「无效」而不是 500：
            // 密钥轮换窗口里被删掉的旧 kid 就是这个表现。
            return Err(AuthError::InvalidToken);
        };
        let mut validation = Validation::new(matched.alg);
        validation.leeway = leeway;
        // EdDSA 与 HS256 的密钥类型不同，但 jsonwebtoken 的 DecodingKey 已是统一载体。
        Ok(decode::<T>(token, &matched.key, &validation)?.claims)
    }

    fn verify<T>(&self, token: &str) -> Result<T, AuthError>
    where
        T: for<'de> Deserialize<'de>,
    {
        self.verify_with_leeway(token, 60)
    }

    /// 不透明刷新令牌：`{jti}.{随机串}`。会话 id 前置，刷新时无需额外索引即可定位会话；
    /// 库中只存整串哈希。
    pub fn new_refresh_token(&self, session_id: &str) -> String {
        format!("{session_id}.{}", Uuid::new_v4().simple())
    }

    /// 从刷新令牌解析会话 id（`jti`）。
    pub fn refresh_session_id(token: &str) -> Option<&str> {
        let (jti, secret) = token.split_once('.')?;
        (!jti.is_empty() && !secret.is_empty()).then_some(jti)
    }
}

/// 绑定票据的 `purpose` 标记。
pub const BINDING_TICKET_PURPOSE: &str = "bind";

#[cfg(test)]
mod tests {
    use super::*;

    // 两把固定的 Ed25519 密钥（`openssl genpkey -algorithm ed25519` 生成，只用于测试）。
    // 写死而不是运行时生成：验签要的是公钥，成对出现才测得出 kid 选密钥这件事。
    const PEM_A: &str = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEILj1jX7bYZ2eWYAkoTD9BWGc64nYHBPW04NS5+HMXgk5\n-----END PRIVATE KEY-----\n";
    const PUB_A: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAGFibMWK2v9xzzxWH9H+tmogrKOmaqWBbqnGhoFN/23c=\n-----END PUBLIC KEY-----\n";
    const PEM_B: &str = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIG1f90Ct+WS2QGSeVYPAyctQfebiJqqq0FlhdSTR9d8y\n-----END PRIVATE KEY-----\n";
    const PUB_B: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAZ52W7e1k2E8w7g2wcwxTaAft0+oG2KO0NS6Vgn09w2Q=\n-----END PUBLIC KEY-----\n";

    fn jwt(
        alg: &str,
        kid: &str,
        private_key: &str,
        public_key: &str,
        keys: Vec<JwtKey>,
    ) -> JwtConfig {
        JwtConfig {
            alg: alg.to_string(),
            kid: kid.to_string(),
            private_key: cicada_infra::config::Secret::new(private_key.to_string()),
            public_key: cicada_infra::config::Secret::new(public_key.to_string()),
            keys,
        }
    }

    use cicada_infra::config::{JwtConfig, JwtKey};

    /// 技术选型「EdDSA(Ed25519) 优先」：换算法不该改变 claims 的形状与往返能力。
    #[test]
    fn eddsa_round_trip_with_kid_in_the_header() {
        let svc = TokenService::from_jwt(&jwt("EdDSA", "k1", PEM_A, PUB_A, vec![]), 900).unwrap();
        assert_eq!(svc.key_id(), Some("k1"));
        let token = svc.sign_access(7, 3, "s-1", "alice").unwrap();
        let header = decode_header(&token).unwrap();
        assert_eq!(header.alg, Algorithm::EdDSA);
        assert_eq!(header.kid.as_deref(), Some("k1"), "kid 要写进头里");
        let claims = svc.verify_access(&token).unwrap();
        assert_eq!(
            (claims.sub.as_str(), claims.tid, claims.jti.as_str()),
            ("7", 3, "s-1")
        );
    }

    /// 认证设计「支持 kid 多密钥轮换」：换到新密钥后，旧 kid 的在途 token 仍可验，
    /// 而新签发的都用新 kid——不必把所有人一次性踢下线。
    #[test]
    fn rotation_keeps_the_retired_key_verifiable() {
        let old = TokenService::from_jwt(&jwt("EdDSA", "k1", PEM_A, PUB_A, vec![]), 900).unwrap();
        let old_token = old.sign_access(1, 1, "s", "bob").unwrap();

        let rotated = TokenService::from_jwt(
            &jwt(
                "EdDSA",
                "k2",
                PEM_B,
                PUB_B,
                vec![JwtKey {
                    kid: "k1".to_string(),
                    key: cicada_infra::config::Secret::new(PUB_A.to_string()),
                    alg: String::new(),
                }],
            ),
            900,
        )
        .unwrap();
        assert_eq!(rotated.verifier_count(), 2);
        let fresh = rotated.sign_access(1, 1, "s2", "bob").unwrap();
        assert_eq!(
            decode_header(&fresh).unwrap().kid.as_deref(),
            Some("k2"),
            "签新 token 只用当前密钥"
        );

        assert!(
            rotated.verify_access(&old_token).is_ok(),
            "旧 kid 的在途 token 必须还能验"
        );
        assert!(rotated.verify_access(&fresh).is_ok());

        // 把旧 kid 从验签集合里摘掉，它就当场失效——这正是「轮换窗口结束」的动作。
        let retired_only =
            TokenService::from_jwt(&jwt("EdDSA", "k2", PEM_B, PUB_B, vec![]), 900).unwrap();
        assert!(
            retired_only.verify_access(&old_token).is_err(),
            "不再列出的 kid 不该还能验签"
        );
    }

    /// 跨密钥与跨算法都必须拒：换密钥不能变成「谁都能签」，
    /// 也不能让 HS256 的 token 蒙过 EdDSA 的校验（alg 混淆攻击的经典入口）。
    #[test]
    fn wrong_key_or_wrong_algorithm_is_rejected() {
        let a = TokenService::from_jwt(&jwt("EdDSA", "k1", PEM_A, PUB_A, vec![]), 900).unwrap();
        let b = TokenService::from_jwt(&jwt("EdDSA", "k1", PEM_B, PUB_B, vec![]), 900).unwrap();
        assert!(
            b.verify_access(&a.sign_access(1, 1, "s", "x").unwrap())
                .is_err(),
            "同一 kid 不同密钥必须验不过"
        );

        let hs = TokenService::new("cicada-dev-shared-secret-0123456789", 900);
        let hs_token = hs.sign_access(1, 1, "s", "x").unwrap();
        assert!(
            a.verify_access(&hs_token).is_err(),
            "HS256 的 token 不该被 EdDSA 的 keyring 接受"
        );
        assert!(
            TokenService::from_jwt(&jwt("EdDSA", "k1", "not a pem", PUB_A, vec![]), 900).is_err()
        );
        assert!(
            TokenService::from_jwt(&jwt("ECDSA", "k1", PEM_A, PUB_A, vec![]), 900).is_err(),
            "算法名要认识"
        );
    }

    #[test]
    fn sign_then_verify() {
        let svc = TokenService::new("dev-secret", 900);
        let token = svc.sign_access(1, 1, "session-1", "admin").unwrap();
        let claims = svc.verify_access(&token).unwrap();
        assert_eq!(claims.sub, "1");
        assert_eq!(claims.tid, 1);
        assert_eq!(claims.username, "admin");
    }

    #[test]
    fn tampered_token_is_rejected() {
        let svc = TokenService::new("dev-secret", 900);
        let token = svc.sign_access(1, 1, "s", "admin").unwrap();
        let tampered = format!("{token}x");
        assert!(svc.verify_access(&tampered).is_err());
    }

    #[test]
    fn binding_ticket_round_trips_and_is_not_an_access_token() {
        let svc = TokenService::new("dev-secret", 900);
        let ticket = svc
            .sign_binding_ticket(1, "Oidc", "keycloak", "keycloak:abc", Some("张三"), 600)
            .unwrap();
        let claims = svc.verify_binding_ticket(&ticket).unwrap();
        assert_eq!(claims.tid, 1);
        assert_eq!(claims.itype, "Oidc");
        assert_eq!(claims.identifier, "keycloak:abc");
        assert_eq!(claims.nickname.as_deref(), Some("张三"));
        // 访问令牌不能被当作绑定票据使用（claims 结构不同）。
        let access = svc.sign_access(1, 1, "s", "admin").unwrap();
        assert!(svc.verify_binding_ticket(&access).is_err());
        // 反向亦然：票据不能当访问令牌。
        assert!(svc.verify_access(&ticket).is_err());
    }

    #[test]
    fn expired_binding_ticket_is_rejected() {
        let svc = TokenService::new("dev-secret", 900);
        // ttl = 0 => 立即到期；跨过 1 秒后必须被拒（`verify_binding_ticket` 不留宽限）。
        let expired = svc
            .sign_binding_ticket(1, "Oidc", "keycloak", "k:1", None, 0)
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(svc.verify_binding_ticket(&expired).is_err());
    }

    #[test]
    fn refresh_token_round_trips_session_id() {
        let svc = TokenService::new("dev-secret", 900);
        let token = svc.new_refresh_token("session-1");
        assert_eq!(TokenService::refresh_session_id(&token), Some("session-1"));
        assert!(TokenService::refresh_session_id("nodot").is_none());
        assert!(TokenService::refresh_session_id(".only").is_none());
    }
}
