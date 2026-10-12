//! 分页参数与结果（集合响应）。

use serde::{Deserialize, Serialize};

/// 分页请求参数。`page` 从 1 开始。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageParams {
    pub page: u64,
    pub page_size: u64,
}

impl Default for PageParams {
    fn default() -> Self {
        Self {
            page: 1,
            page_size: 20,
        }
    }
}

/// 分页结果。集合响应形状：`{ items, total, page, page_size }`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_page_starts_at_one() {
        assert_eq!(
            PageParams::default(),
            PageParams {
                page: 1,
                page_size: 20
            }
        );
    }
}
