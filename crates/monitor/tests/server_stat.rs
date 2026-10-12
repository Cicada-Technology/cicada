//! 服务信息采集的采集层回归测试。
//!
//! 这里只测 `collect()` 给出的事实是否自洽：HTTP 面的形状与鉴权在
//! `cicada-web` 的 `server_info` 测试里。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use cicada_monitor::server::collect;

#[test]
fn cpu_usage_is_sampled_not_always_zero() {
    // 回归：采样间隔短于 sysinfo 的 MINIMUM_CPU_UPDATE_INTERVAL 时，
    // CPU 使用率会恒为 0（服务监控页上表现为「空环 + 0%」）。
    // 因此在真实负载下采集，要求它给出非零读数。
    let stop = Arc::new(AtomicBool::new(false));
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut acc: u64 = 0;
                while !stop.load(Ordering::Relaxed) {
                    acc = acc.wrapping_add(1);
                    std::hint::spin_loop();
                }
                acc
            })
        })
        .collect();

    // 让负载先跑起来，避免基线刷新时还没开始忙。
    std::thread::sleep(std::time::Duration::from_millis(50));
    let stat = collect();
    stop.store(true, Ordering::Relaxed);
    for worker in workers {
        let _ = worker.join();
    }

    assert!(stat.cpu_cores > 0);
    assert!(
        stat.cpu_usage > 0.0,
        "负载下 CPU 使用率仍为 0，说明两次刷新的间隔不足"
    );
    assert!(stat.cpu_usage <= 100.0 * stat.cpu_cores as f32);
}

#[test]
fn disks_exclude_zero_capacity_mounts() {
    let stat = collect();
    assert!(
        !stat.disks.is_empty(),
        "至少应能看到根文件系统；容器内跑测试时若确实无挂载点请放宽此断言"
    );
    for disk in &stat.disks {
        assert!(disk.total_bytes > 0, "{} 报出 0 容量", disk.mount_point);
        assert!(
            disk.used_bytes <= disk.total_bytes,
            "{} 已用超过总量",
            disk.mount_point
        );
    }
}

#[test]
fn process_uptime_tracks_wall_clock() {
    let (tx, rx) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let first = collect();
        std::thread::sleep(std::time::Duration::from_millis(1_200));
        let second = collect();
        tx.send((first, second)).ok();
    });
    let (first, second) = rx.recv().expect("collector thread died");
    handle.join().unwrap();

    assert!(second.process_uptime_secs >= first.process_uptime_secs);
    assert!(second.system_uptime_secs >= first.system_uptime_secs);
    assert!(second.mem_total_bytes > 0);
    assert!(second.mem_used_bytes <= second.mem_total_bytes);
    assert!(second.pid == std::process::id());
}
