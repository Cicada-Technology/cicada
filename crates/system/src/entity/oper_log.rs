//! `sys_oper_log`：操作日志（租户隔离、无软删、无归属维度 → `NoOwnership`）。
//!
//! 由 `cicada-audit` 的 sink 写入；本实体只用于 `monitor:operlog:*` 的只读/清理接口。

use sea_orm::entity::prelude::*;

use cicada_macros::DataScoped;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, DeriveActiveModelBehavior, DataScoped)]
#[sea_orm(table_name = "sys_oper_log")]
#[data_scope(tenant = Column::TenantId, no_ownership)]
pub struct Model {
    #[sea_orm(primary_key)]
    pub oper_id: i64,
    pub tenant_id: i64,
    pub title: String,
    /// 见 `cicada_audit::OperType`（0 其它 … 9 清空）。
    pub business_type: i16,
    pub method: String,
    pub request_method: String,
    pub operator_type: i16,
    pub oper_name: Option<String>,
    pub dept_name: Option<String>,
    pub oper_url: String,
    pub oper_ip: Option<String>,
    pub oper_param: Option<String>,
    pub json_result: Option<String>,
    /// 0 正常 / 1 异常。
    pub status: i16,
    pub error_msg: Option<String>,
    pub oper_time: DateTimeUtc,
    pub cost_ms: i64,
    /// 以下三列由迁移 `m20261019_000017_oper_log_context` 补上。
    pub user_agent: Option<String>,
    /// 业务码目录的业务码（失败才有），可按它聚合「哪些接口在报什么错」。
    pub biz_code: Option<String>,
    /// 脱敏后的 JSON 请求体；查询串在 `oper_param`。
    pub req_body: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
