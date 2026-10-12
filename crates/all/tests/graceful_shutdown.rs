//! 优雅停机：收到停止信号后停止接收新请求，并在有界时间内排空在途请求；
//! 进程无状态、可水平扩展。
//!
//! 为什么单独测这个函数而不是 `App::serve()`：`serve()` 要真数据库与完整装配，
//! 而停机路径的两个关键行为——「收到停止信号就收摊」和「排空有上限」——
//! 只跟 `serve_with_drain` 有关。用裸 `Router` + 临时端口就能把两条都钉住。

use std::time::{Duration, Instant};

use axum::Router;
use axum::routing::get;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::net::TcpStream;

/// 一个立刻完成的停止信号。
async fn stop_now() {}

/// 一条慢慢来的 handler：比任何合理的 `grace` 都长。
async fn slow() -> &'static str {
    tokio::time::sleep(Duration::from_secs(30)).await;
    "done"
}

async fn bind() -> (TcpListener, std::net::SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    (listener, addr)
}

/// 发一个请求但不等回应——目的是让这条连接在停机时正「在途」。
async fn kick_off_a_request(addr: std::net::SocketAddr) {
    let mut sock = TcpStream::connect(addr).await.unwrap();
    sock.write_all(b"GET /slow HTTP/1.1\r\nhost: localhost\r\n\r\n")
        .await
        .unwrap();
    // 连接由 `sock` 持有；调用方 drop 它之前，服务端那一侧的请求不会结束。
    tokio::time::sleep(Duration::from_millis(120)).await;
    // 故意不关闭：留一条挂着的请求给停机逻辑处置。
    std::mem::forget(sock);
}

#[tokio::test]
async fn the_server_stops_when_the_signal_completes() {
    let (listener, _addr) = bind().await;
    let started = Instant::now();
    cicada::bootstrap::serve_with_drain(
        listener,
        Router::new(),
        stop_now(),
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    // `stop_now` 早已就绪，信号那一路应立刻生效。
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "停不下来：{started:?}"
    );
}

/// 在途请求超过 `grace` 时**必须**放手，而不是等那条 30 秒的 handler 跑完。
///
/// 这一条盯的是容器里的真实事故形状：一个卡住的长连接让 `docker stop` 等满
/// 宽限期后被 SIGKILL，进程连退出码都不体面。`grace` 是给收尾的时间，不是通行证。
#[tokio::test]
async fn the_drain_is_bounded_by_grace_not_by_the_slowest_request() {
    let (listener, addr) = bind().await;
    let router = Router::new().route("/slow", get(slow));
    let (tx, rx) = tokio::sync::watch::channel(false);
    let serve = tokio::spawn(cicada::bootstrap::serve_with_drain(
        listener,
        router,
        async move {
            let mut rx = rx;
            rx.changed().await.ok();
        },
        Duration::from_millis(200),
    ));

    kick_off_a_request(addr).await;
    tx.send(true).unwrap();

    let started = Instant::now();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("停机被拖到超过 5 秒：grace 没有生效")
        .unwrap()
        .unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "本该在 grace(200ms) 附近就退出，实际 {elapsed:?}"
    );
    // 端口随监听器一起释放：下一次绑定不该失败。
    let (again, _) = bind().await;
    drop(again);
}

/// 排空上限为 0 的语义是「不等」：信号一到就走。
#[tokio::test]
async fn zero_grace_means_no_waiting() {
    let (listener, addr) = bind().await;
    let router = Router::new().route("/slow", get(slow));
    let (tx, rx) = tokio::sync::watch::channel(false);
    let serve = tokio::spawn(cicada::bootstrap::serve_with_drain(
        listener,
        router,
        async move {
            let mut rx = rx;
            rx.changed().await.ok();
        },
        Duration::ZERO,
    ));
    kick_off_a_request(addr).await;
    let started = Instant::now();
    tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(3), serve)
        .await
        .expect("grace=0 却还在等在途请求")
        .unwrap()
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
}
