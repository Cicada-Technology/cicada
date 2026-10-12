//! 租户限额：套餐 → 用户数 / 存储量 / 接口 QPS 上限。
//!
//! 限额是**提示性闸门**：`-1` 表示不限；`sys_tenant.package_id` 为空表示无套餐、不限额。
//! 调用点在 `system` 域自己的写路径（建号）与 `storage` 域的写路径（上传）上，
//! 由组装根注入同一个实例；跨域调用走端口而不直连本模块内部。

use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use cicada_infra::Db;
use cicada_kernel::TenantId;

use crate::entity::{
    FileColumn, FileEntity, TenantColumn, TenantEntity, TenantPackageEntity, TenantPackageModel,
    UserColumn, UserEntity,
};

/// 一次限额校验的失败原因，交给接入层映射成 403/409。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum QuotaError {
    #[error("租户用户数已达上限 {limit}（当前 {current}）")]
    UserLimit { limit: i32, current: i64 },
    #[error("租户存储已达上限 {limit_mb} MB（当前已用 {used_mb} MB）")]
    StorageLimit { limit_mb: i64, used_mb: i64 },
    #[error("租户限额查询失败：{0}")]
    Db(String),
}

/// 租户当前用量。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TenantUsage {
    pub users: i64,
    pub storage_bytes: i64,
}

impl TenantUsage {
    pub fn storage_mb(&self) -> i64 {
        (self.storage_bytes + 1024 * 1024 - 1) / (1024 * 1024)
    }
}

/// 租户套餐与用量的读取入口。
#[derive(Clone)]
pub struct TenantQuota {
    db: Db,
}

impl TenantQuota {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// 租户当前生效的套餐；无套餐（或套餐被删）返回 `None`，即不限额。
    pub async fn package(
        &self,
        tenant: TenantId,
    ) -> Result<Option<TenantPackageModel>, QuotaError> {
        let Some(row) = TenantEntity::find()
            .filter(TenantColumn::TenantId.eq(tenant.get()))
            .one(self.db.conn())
            .await
            .map_err(|e| QuotaError::Db(e.to_string()))?
        else {
            return Ok(None);
        };
        let Some(package_id) = row.package_id else {
            return Ok(None);
        };
        TenantPackageEntity::find_by_id(package_id)
            .one(self.db.conn())
            .await
            .map_err(|e| QuotaError::Db(e.to_string()))
    }

    /// 当前用量：未删除用户数 + 未删除文件字节数。
    pub async fn usage(&self, tenant: TenantId) -> Result<TenantUsage, QuotaError> {
        let users = UserEntity::find()
            .filter(UserColumn::TenantId.eq(tenant.get()))
            .filter(UserColumn::DeletedAt.is_null())
            .all(self.db.conn())
            .await
            .map_err(|e| QuotaError::Db(e.to_string()))?
            .len() as i64;

        let files = FileEntity::find()
            .filter(FileColumn::TenantId.eq(tenant.get()))
            .filter(FileColumn::DeletedAt.is_null())
            .all(self.db.conn())
            .await
            .map_err(|e| QuotaError::Db(e.to_string()))?;
        let storage_bytes = files.iter().map(|f| f.size).sum();

        Ok(TenantUsage {
            users,
            storage_bytes,
        })
    }

    /// 租户的接口 QPS 上限（套餐 `api_qps_limit`）：`None` 表示不限
    /// （无套餐，或值 <= 0）。供接入层限流层（第 7 层）解析配额。
    pub async fn api_qps_limit(&self, tenant: TenantId) -> Result<Option<u32>, QuotaError> {
        let Some(package) = self.package(tenant).await? else {
            return Ok(None);
        };
        if package.api_qps_limit <= 0 {
            return Ok(None);
        }
        Ok(Some(package.api_qps_limit as u32))
    }

    /// 建号前的闸门：`additional` 是本次要新增的账号数（单建 1、批量导入为行数）。
    /// `-1` 不限。
    pub async fn check_user_slot(
        &self,
        tenant: TenantId,
        additional: i64,
    ) -> Result<(), QuotaError> {
        let Some(package) = self.package(tenant).await? else {
            return Ok(());
        };
        if package.user_limit < 0 {
            return Ok(());
        }
        let usage = self.usage(tenant).await?;
        if usage.users + additional > package.user_limit as i64 {
            return Err(QuotaError::UserLimit {
                limit: package.user_limit,
                current: usage.users,
            });
        }
        Ok(())
    }

    /// 上传前的闸门：本次写入后不得越过存储上限。
    pub async fn check_storage_slot(
        &self,
        tenant: TenantId,
        additional_bytes: i64,
    ) -> Result<(), QuotaError> {
        let Some(package) = self.package(tenant).await? else {
            return Ok(());
        };
        if package.storage_limit_mb < 0 {
            return Ok(());
        }
        let usage = self.usage(tenant).await?;
        let limit_bytes = package.storage_limit_mb.saturating_mul(1024 * 1024);
        if usage.storage_bytes.saturating_add(additional_bytes) > limit_bytes {
            return Err(QuotaError::StorageLimit {
                limit_mb: package.storage_limit_mb,
                used_mb: usage.storage_mb(),
            });
        }
        Ok(())
    }
}
