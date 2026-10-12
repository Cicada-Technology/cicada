//! 服务信息：CPU、内存、磁盘、负载、进程启动时间与运行时长。
//!
//! 对应参考实现的服务监控页，但只出数据不出内嵌 HTML 控制台；
//! HTTP 面在 `cicada-web` 的 `server` 模块，采集逻辑集中在这里。

use serde::Serialize;
use sysinfo::{CpuRefreshKind, Disks, System};

/// 主机与服务进程的快照，字段全部为瞬时值。
#[derive(Debug, Clone, Serialize)]
pub struct ServerStat {
    pub os_name: String,
    pub os_version: String,
    pub hostname: String,
    pub arch: String,
    pub cpu_brand: String,
    pub cpu_cores: usize,
    /// 全部核心的平均使用率（0.0–100.0）。
    pub cpu_usage: f32,
    pub mem_total_bytes: u64,
    pub mem_used_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
    pub load_avg_1m: f64,
    pub load_avg_5m: f64,
    pub load_avg_15m: f64,
    /// 主机开机以来的秒数。
    pub system_uptime_secs: u64,
    /// 本进程启动时的 Unix 时间戳（秒）。
    pub process_start_unix_secs: u64,
    /// 本进程已运行秒数。
    pub process_uptime_secs: u64,
    pub pid: u32,
    pub disks: Vec<DiskStat>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskStat {
    pub name: String,
    pub mount_point: String,
    pub fs_type: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
}

/// 采集一次系统与服务进程快照。
///
/// CPU 使用率是差值采样：第一次刷新建立基线，间隔 `MINIMUM_CPU_UPDATE_INTERVAL`
/// （Linux 上 200 ms）后再刷新一次才有意义。间隔短于该值时 sysinfo 直接返回 0，
/// 所以这里必须用它的常量而不是拍一个毫秒数。
pub fn collect() -> ServerStat {
    let mut sys = System::new();
    sys.refresh_cpu_specifics(CpuRefreshKind::everything());
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    sys.refresh_processes(
        sysinfo::ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(std::process::id())]),
        true,
    );

    let cpu_usage = if sys.cpus().is_empty() {
        0.0
    } else {
        sys.cpus().iter().map(|c| c.cpu_usage()).sum::<f32>() / sys.cpus().len() as f32
    };

    let load = System::load_average();
    let self_pid = std::process::id();
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let uptime = System::uptime();
    let process_uptime = sys
        .process(sysinfo::Pid::from_u32(self_pid))
        .map_or(uptime, |p| p.run_time());
    let process_start = now_unix.saturating_sub(process_uptime);

    let disks = Disks::new_with_refreshed_list()
        .iter()
        .filter_map(|d| {
            let total_bytes = d.total_space();
            // Docker 的 overlay 挂载点每个镜像层都会报一条 0 容量的记录，
            // 在容器化宿主机上能把真实磁盘挤出屏幕；容量为 0 即视为非真实卷。
            if total_bytes == 0 {
                return None;
            }
            Some(DiskStat {
                name: d.name().to_string_lossy().into_owned(),
                mount_point: d.mount_point().to_string_lossy().into_owned(),
                fs_type: d.file_system().to_string_lossy().into_owned(),
                total_bytes,
                used_bytes: total_bytes.saturating_sub(d.available_space()),
            })
        })
        .collect();

    ServerStat {
        os_name: System::name().unwrap_or_else(|| "unknown".into()),
        os_version: System::os_version().unwrap_or_else(|| "unknown".into()),
        hostname: System::host_name().unwrap_or_else(|| "unknown".into()),
        arch: std::env::consts::ARCH.to_string(),
        cpu_brand: sys
            .cpus()
            .first()
            .map(|c| c.brand().to_string())
            .unwrap_or_else(|| "unknown".into()),
        cpu_cores: sys.cpus().len(),
        cpu_usage,
        mem_total_bytes: sys.total_memory(),
        mem_used_bytes: sys.used_memory(),
        swap_total_bytes: sys.total_swap(),
        swap_used_bytes: sys.used_swap(),
        load_avg_1m: load.one,
        load_avg_5m: load.five,
        load_avg_15m: load.fifteen,
        system_uptime_secs: uptime,
        process_start_unix_secs: process_start,
        process_uptime_secs: process_uptime,
        pid: self_pid,
        disks,
    }
}
