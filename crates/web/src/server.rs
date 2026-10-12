//! 服务监控 HTTP 面：主机与服务进程信息的只读接口。
//!
//! 对应参考实现的服务监控页，但只出 JSON 不出内嵌 HTML。权限点清单的
//! `monitor:server:view` 是 C 类菜单权限点、不做 REST 守卫（与缓存监控同一约定，
//! 可观测性），观测各自新增一个 F 类权限点 `monitor:server:list`。

use axum::Json;
use axum::extract::State;
use axum::routing::get;
use serde::Serialize;
use utoipa::ToSchema;

use cicada_macros::require_perm;
use cicada_monitor::server::ServerStat;

use crate::Permission;
use crate::extractor::RequestContextExt;
use crate::module::{Manifest, Module};
use crate::routes::Routes;
use crate::state::AppState;

use crate::problem::{ApiErr, BizCode, Problem};

/// 观测权限点：读服务信息。
pub const PERM_LIST: &str = "monitor:server:list";

const PERMS: &[&str] = &[PERM_LIST];

#[derive(Debug, Serialize, ToSchema)]
pub struct DiskVo {
    pub name: String,
    pub mount_point: String,
    pub fs_type: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ServerInfoVo {
    pub os_name: String,
    pub os_version: String,
    pub hostname: String,
    pub arch: String,
    pub cpu_brand: String,
    pub cpu_cores: usize,
    pub cpu_usage: f32,
    pub mem_total_bytes: u64,
    pub mem_used_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
    pub load_avg_1m: f64,
    pub load_avg_5m: f64,
    pub load_avg_15m: f64,
    pub system_uptime_secs: u64,
    pub process_start_unix_secs: u64,
    pub process_uptime_secs: u64,
    pub pid: u32,
    /// 应用版本（编译期注入）。
    pub version: String,
    /// 构建 commit（`GIT_COMMIT` 编译期注入，缺省 `unknown`）。
    pub build_commit: String,
    pub disks: Vec<DiskVo>,
}

fn vo_from_stat(s: ServerStat) -> ServerInfoVo {
    ServerInfoVo {
        os_name: s.os_name,
        os_version: s.os_version,
        hostname: s.hostname,
        arch: s.arch,
        cpu_brand: s.cpu_brand,
        cpu_cores: s.cpu_cores,
        cpu_usage: s.cpu_usage,
        mem_total_bytes: s.mem_total_bytes,
        mem_used_bytes: s.mem_used_bytes,
        swap_total_bytes: s.swap_total_bytes,
        swap_used_bytes: s.swap_used_bytes,
        load_avg_1m: s.load_avg_1m,
        load_avg_5m: s.load_avg_5m,
        load_avg_15m: s.load_avg_15m,
        system_uptime_secs: s.system_uptime_secs,
        process_start_unix_secs: s.process_start_unix_secs,
        process_uptime_secs: s.process_uptime_secs,
        pid: s.pid,
        version: env!("CARGO_PKG_VERSION").to_string(),
        build_commit: option_env!("GIT_COMMIT").unwrap_or("unknown").to_string(),
        disks: s
            .disks
            .into_iter()
            .map(|d| DiskVo {
                name: d.name,
                mount_point: d.mount_point,
                fs_type: d.fs_type,
                total_bytes: d.total_bytes,
                used_bytes: d.used_bytes,
            })
            .collect(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/monitor/server",
    responses((status = 200, description = "服务信息", body = ServerInfoVo))
)]
#[require_perm("monitor:server:list")]
pub async fn server_info(
    State(_state): State<AppState>,
    _ctx: RequestContextExt,
) -> Result<Json<ServerInfoVo>, ApiErr> {
    // 采集含 50ms CPU 采样阻塞，挪到阻塞线程池，不占事件循环。
    let stat = tokio::task::spawn_blocking(cicada_monitor::server::collect)
        .await
        .map_err(|e| Problem::new(BizCode::Internal, format!("服务信息采集失败：{e}")))?;
    Ok(Json(vo_from_stat(stat)))
}

/// 服务监控模块：权限点与路由声明，装配期双向对账。
pub struct ServerModule;

impl Module for ServerModule {
    fn manifest(self: Box<Self>) -> Manifest {
        let perms = PERMS.iter().map(|p| Permission::new(*p));
        Manifest::new("cicada:monitor-server")
            .perms(perms)
            .routes(routes)
    }
}

pub fn routes(_state: AppState) -> Routes {
    Routes::new().route(
        "/api/v1/monitor/server",
        get(server_info),
        server_info__perms(),
    )
}
