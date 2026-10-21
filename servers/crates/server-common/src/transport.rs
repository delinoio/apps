// SPDX-License-Identifier: FSL-1.1-ALv2
use axum::Router;
use hyper_util::{
    rt::{TokioExecutor, TokioIo, TokioTimer},
    server::conn::auto::Builder,
};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::{
    net::TcpListener,
    sync::{Notify, watch},
    task::JoinSet,
};
use tokio_io_timeout::TimeoutStream;
use tower::ServiceExt;

#[derive(Clone, Copy)]
pub struct Limits {
    pub header: Duration,
    pub idle: Duration,
    pub write: Duration,
    pub shutdown: Duration,
}

fn timed_io<T: tokio::io::AsyncRead + tokio::io::AsyncWrite>(
    io: T,
    limits: Limits,
) -> TimeoutStream<T> {
    let mut stream = TimeoutStream::new(io);
    stream.set_read_timeout(Some(limits.idle));
    stream.set_write_timeout(Some(limits.write));
    stream
}

/// Bound header parsing and stalled socket I/O before requests enter Axum.
/// The HTTP policy separately bounds the complete body/handler execution.
pub async fn serve(
    listener: TcpListener,
    app: Router,
    limits: Limits,
    shutdown: impl Future<Output = ()>,
) -> std::io::Result<()> {
    let (stop, _) = watch::channel(false);
    let mut connections = JoinSet::new();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            Some(_) = connections.join_next(), if !connections.is_empty() => {},
            accepted = listener.accept() => {
                let (socket, _) = accepted?;
                let app = app.clone();
                let mut stopped = stop.subscribe();
                connections.spawn(async move {
                    let first_request = Arc::new(Notify::new());
                    let received = first_request.clone();
                    let service = hyper::service::service_fn(move |request| {
                        received.notify_one();
                        app.clone().oneshot(request)
                    });
                    let mut builder = Builder::new(TokioExecutor::new());
                    builder.http1().timer(TokioTimer::new()).header_read_timeout(limits.header).max_buf_size(16 << 10);
                    builder.http2().timer(TokioTimer::new()).keep_alive_interval(limits.idle).keep_alive_timeout(limits.header).max_header_list_size(16 << 10);
                    let io = TokioIo::new(Box::pin(timed_io(socket, limits)));
                    let connection = builder.serve_connection(io, service);
                    tokio::pin!(connection);
                    // The auto-protocol detector runs before Hyper's HTTP/1 header timer.
                    // Bound the first request too, including silent sockets and partial prefaces.
                    let first = tokio::time::sleep(limits.header);
                    tokio::pin!(first);
                    tokio::select! {
                        _ = &mut connection => return,
                        _ = &mut first => { tracing::debug!(category = "header_timeout", "connection closed"); return; },
                        _ = stopped.changed() => {},
                        _ = first_request.notified() => {
                            tokio::select! {
                                result = &mut connection => {
                                    if result.is_err() { tracing::debug!(category = "transport", "connection closed"); }
                                    return;
                                },
                                _ = stopped.changed() => {},
                            }
                        },
                    }
                    connection.as_mut().graceful_shutdown();
                    let _ = tokio::time::timeout(limits.shutdown, &mut connection).await;
                });
            }
        }
    }
    let _ = stop.send(true);
    while connections.join_next().await.is_some() {}
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    fn limits() -> Limits {
        Limits {
            header: Duration::from_millis(60),
            idle: Duration::from_millis(100),
            write: Duration::from_millis(60),
            shutdown: Duration::from_millis(100),
        }
    }
    #[tokio::test]
    async fn closes_silent_and_partial_headers_before_router() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let app = Router::new().route("/healthz", axum::routing::get(|| async { "ok" }));
        let server = tokio::spawn(serve(listener, app, limits(), async {
            let _ = stopped.await;
        }));
        for partial in [false, true] {
            let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
            if partial {
                socket
                    .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nX-Slow: ")
                    .await
                    .unwrap();
            }
            let mut raw = Vec::new();
            let read = tokio::time::timeout(Duration::from_secs(2), socket.read_to_end(&mut raw))
                .await
                .unwrap();
            assert!(
                read.is_ok()
                    || read.is_err_and(|e| e.kind() == std::io::ErrorKind::ConnectionReset)
            );
            assert!(!String::from_utf8_lossy(&raw).contains("200 OK"));
        }
        assert_eq!(
            reqwest::get(format!("http://{address}/healthz"))
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            "ok"
        );
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    #[tokio::test]
    async fn closes_idle_reads_and_blocked_writes() {
        let (io, _peer) = tokio::io::duplex(1);
        let mut io = Box::pin(timed_io(io, limits()));
        let error = io.read_u8().await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        let error = io.write_all(&[1, 2]).await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }
}
