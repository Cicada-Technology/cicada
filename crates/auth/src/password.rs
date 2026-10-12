//! 密码哈希与校验（Argon2id）。哈希串自带参数，登录时按需升级。

use argon2::password_hash::phc::PasswordHash;
use argon2::{Argon2, PasswordHasher, PasswordVerifier};

use crate::error::AuthError;

pub fn hash_password(pw: &str) -> Result<String, AuthError> {
    let hash = Argon2::default()
        .hash_password(pw.as_bytes())
        .map_err(|e| AuthError::Password(e.to_string()))?;
    Ok(hash.to_string())
}

/// 校验成功返回 `Ok(true)`；密码不匹配返回 `Ok(false)`（不泄露差异）。
///
/// 兼容参考实现搬迁来的 BCrypt 哈希：登录成功后可经
/// [`needs_rehash`] 判断并按需升级为 Argon2id。
pub fn verify_password(pw: &str, hash: &str) -> Result<bool, AuthError> {
    if is_bcrypt(hash) {
        return bcrypt::verify(pw, hash).map_err(|e| AuthError::Password(e.to_string()));
    }
    let parsed = PasswordHash::new(hash).map_err(|e| AuthError::Password(e.to_string()))?;
    Ok(Argon2::default()
        .verify_password(pw.as_bytes(), &parsed)
        .is_ok())
}

/// 该哈希是否需要在登录成功后升级为 Argon2id（非 Argon2 哈希一律需要）。
pub fn needs_rehash(hash: &str) -> bool {
    !hash.starts_with("$argon2")
}

fn is_bcrypt(hash: &str) -> bool {
    hash.starts_with("$2a$") || hash.starts_with("$2b$") || hash.starts_with("$2y$")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_bcrypt_and_flags_rehash() {
        // 参考实现初始管理员密码 "admin123" 的 BCrypt 哈希。
        let hash = "$2a$10$7JB720yubVSZvUI0rEqK/.VqGOZTH.ulu33dHOiBE8ByOhJIrdAu2";
        assert!(verify_password("admin123", hash).unwrap());
        assert!(!verify_password("wrong", hash).unwrap());
        assert!(needs_rehash(hash));
        assert!(!needs_rehash(&hash_password("x").unwrap()));
    }

    #[test]
    fn hash_then_verify() {
        let hash = hash_password("Cicada@2026").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password("Cicada@2026", &hash).unwrap());
        assert!(!verify_password("wrong", &hash).unwrap());
    }
}
