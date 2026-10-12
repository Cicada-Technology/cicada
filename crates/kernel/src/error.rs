//! 统一错误类型。此处只放**领域变体**；`Db` / `Internal` 等依赖基础设施/传输层的
//! 变体由上层（infra/web）扩展，符合  #3「库 crate 公开 API 不出现 anyhow」。

use crate::permission::Permission;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("未认证")]
    Unauthorized,
    #[error("无权限：{0}")]
    Forbidden(Permission),
    #[error("资源不存在：{0}")]
    NotFound(String),
    #[error("参数校验失败：{0}")]
    Validation(String),
    #[error("账号已临时锁定")]
    Locked,
    #[error("请求过于频繁")]
    RateLimited,
    #[error("数据冲突：{0}")]
    Conflict(String),
    #[error("乐观锁冲突")]
    OptimisticLock,
    #[error("上游服务不可用：{0}")]
    Upstream(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbidden_carries_permission() {
        let e = AppError::Forbidden(Permission::new("system:user:add"));
        assert_eq!(e.to_string(), "无权限：system:user:add");
    }
}
