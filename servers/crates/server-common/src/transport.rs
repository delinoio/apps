// SPDX-License-Identifier: FSL-1.1-ALv2
use axum::{Router, body::Body as AxumBody};
use hyper::body::{Body, Bytes, Frame, SizeHint};
use hyper_util::{
    rt::{TokioExecutor, TokioIo, TokioTimer},
    server::conn::auto::Builder,
};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context, Poll, Waker},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpListener,
    sync::{Notify, watch},
    task::JoinSet,
    time::Sleep,
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

/// Connection-wide switch for the idle-read timer.
///
/// Hyper keeps polling the socket for EOF while an HTTP/1 handler runs, so an unconditional read
/// timer would tear down a valid request whose body is complete and whose response is pending.
/// The timer is suspended only between request-body completion and response completion; the
/// socket is still polled throughout, so a real close still cancels the handler. HTTP/2 never
/// suspends it because streams multiplex on one socket.
#[derive(Default)]
struct IdleControl {
    suspended: AtomicBool,
    /// Bumped on every transition so a timer armed in an earlier phase is restarted.
    epoch: AtomicU64,
    /// Reader parked while suspended; woken on a transition so it re-arms or drops its timer.
    parked: Mutex<Option<Waker>>,
}

impl IdleControl {
    fn set_suspended(&self, suspended: bool) {
        if self.suspended.swap(suspended, Ordering::SeqCst) != suspended {
            self.epoch.fetch_add(1, Ordering::SeqCst);
            let parked = self.parked.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(waker) = parked {
                waker.wake();
            }
        }
    }
}

/// One HTTP/1 request/response exchange.
struct Exchange {
    control: Arc<IdleControl>,
    finished: AtomicBool,
}

impl Exchange {
    fn request_body_complete(&self) {
        self.control.set_suspended(true);
        // A response that finished first must win even if this runs concurrently with it.
        if self.finished.load(Ordering::SeqCst) {
            self.control.set_suspended(false);
        }
    }

    fn response_finished(&self) {
        self.finished.store(true, Ordering::SeqCst);
        self.control.set_suspended(false);
    }
}

/// Request body that reports end-of-stream to its exchange.
struct RequestBody<B> {
    inner: B,
    exchange: Arc<Exchange>,
}

impl<B: Body + Unpin> Body for RequestBody<B> {
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let poll = Pin::new(&mut self.inner).poll_frame(cx);
        let complete = match &poll {
            Poll::Ready(None) => true,
            Poll::Ready(Some(Ok(_))) => self.inner.is_end_stream(),
            _ => false,
        };
        if complete {
            self.exchange.request_body_complete();
        }
        poll
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

/// Response body that restores idle protection once the response has been produced.
struct ResponseBody {
    inner: AxumBody,
    exchange: Arc<Exchange>,
}

impl Body for ResponseBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let poll = Pin::new(&mut self.inner).poll_frame(cx);
        let finished = match &poll {
            Poll::Ready(None) | Poll::Ready(Some(Err(_))) => true,
            Poll::Ready(Some(Ok(_))) => self.inner.is_end_stream(),
            Poll::Pending => false,
        };
        if finished {
            self.exchange.response_finished();
        }
        poll
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for ResponseBody {
    fn drop(&mut self) {
        self.exchange.response_finished();
    }
}

/// Wraps an HTTP/1 request so its body and response report their lifecycle. HTTP/2 requests
/// pass through untouched, which keeps the per-connection idle policy for multiplexed streams.
fn track_request<B>(
    request: hyper::Request<B>,
    control: &Arc<IdleControl>,
) -> (hyper::Request<AxumBody>, Option<Arc<Exchange>>)
where
    B: Body<Data = Bytes> + Unpin + Send + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    if request.version() == hyper::Version::HTTP_2 {
        return (request.map(AxumBody::new), None);
    }
    let exchange = Arc::new(Exchange {
        control: control.clone(),
        finished: AtomicBool::new(false),
    });
    // Empty bodies are complete on arrival and never produce a final frame.
    if request.body().is_end_stream() {
        exchange.request_body_complete();
    }
    let tracked = exchange.clone();
    let request = request.map(move |inner| {
        AxumBody::new(RequestBody {
            inner,
            exchange: tracked,
        })
    });
    (request, Some(exchange))
}

/// Read-idle timer that can be suspended while the socket keeps being polled.
struct IdleRead<T> {
    inner: Pin<Box<TimeoutStream<T>>>,
    idle: Duration,
    control: Arc<IdleControl>,
    timer: Option<Pin<Box<Sleep>>>,
    armed_epoch: u64,
}

impl<T: AsyncRead + AsyncWrite> AsyncRead for IdleRead<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        if let ready @ Poll::Ready(_) = this.inner.as_mut().poll_read(cx, buf) {
            this.timer = None;
            return ready;
        }
        if this.control.suspended.load(Ordering::SeqCst) {
            // The inner read stays registered for socket events; only the idle deadline is off.
            *this
                .control
                .parked
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(cx.waker().clone());
            // Re-check after parking so a concurrent resume cannot be missed.
            if this.control.suspended.load(Ordering::SeqCst) {
                this.timer = None;
                return Poll::Pending;
            }
        }
        let epoch = this.control.epoch.load(Ordering::SeqCst);
        if this.timer.is_none() || this.armed_epoch != epoch {
            this.timer = Some(Box::pin(tokio::time::sleep(this.idle)));
            this.armed_epoch = epoch;
        }
        match this.timer.as_mut().map(|timer| timer.as_mut().poll(cx)) {
            Some(Poll::Ready(())) => {
                this.timer = None;
                Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "idle read timed out",
                )))
            }
            _ => Poll::Pending,
        }
    }
}

impl<T: AsyncRead + AsyncWrite> AsyncWrite for IdleRead<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.inner.as_mut().poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner.as_mut().poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner.as_mut().poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.inner.as_mut().poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

fn timed_io<T: AsyncRead + AsyncWrite>(
    io: T,
    limits: Limits,
    control: Arc<IdleControl>,
) -> IdleRead<T> {
    // The read timer lives in `IdleRead`; `TimeoutStream` only enforces the write bound.
    let mut stream = TimeoutStream::new(io);
    stream.set_write_timeout(Some(limits.write));
    IdleRead {
        inner: Box::pin(stream),
        idle: limits.idle,
        control,
        timer: None,
        armed_epoch: 0,
    }
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
                    let control = Arc::new(IdleControl::default());
                    let tracking = control.clone();
                    let service = hyper::service::service_fn(move |request| {
                        received.notify_one();
                        let (request, exchange) = track_request(request, &tracking);
                        let app = app.clone();
                        async move {
                            let response = app.oneshot(request).await?;
                            Ok::<_, std::convert::Infallible>(match exchange {
                                Some(exchange) => response.map(|inner| {
                                    // Bodyless responses are finished once produced.
                                    if inner.is_end_stream() {
                                        exchange.response_finished();
                                    }
                                    AxumBody::new(ResponseBody { inner, exchange })
                                }),
                                None => response,
                            })
                        }
                    });
                    let mut builder = Builder::new(TokioExecutor::new());
                    builder.http1().timer(TokioTimer::new()).header_read_timeout(limits.header).max_buf_size(16 << 10);
                    builder.http2().timer(TokioTimer::new()).keep_alive_interval(limits.idle).keep_alive_timeout(limits.header).max_header_list_size(16 << 10);
                    let io = TokioIo::new(Box::pin(timed_io(socket, limits, control)));
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
        let mut io = Box::pin(timed_io(io, limits(), Arc::default()));
        let error = io.read_u8().await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        let error = io.write_all(&[1, 2]).await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }

    // Phase-aware idle reads. The fixture mirrors the original reproduction: idle 50ms,
    // handler 150ms, and a client that keeps its socket open after sending the whole request.
    use std::{sync::atomic::AtomicUsize, time::Instant};

    #[derive(Default)]
    struct Probe {
        entered: AtomicUsize,
        completed: AtomicUsize,
        cancelled: AtomicUsize,
    }

    impl Probe {
        fn count(counter: &AtomicUsize) -> usize {
            counter.load(Ordering::SeqCst)
        }
    }

    struct Guard(Arc<Probe>, bool);
    impl Drop for Guard {
        fn drop(&mut self) {
            if !self.1 {
                self.0.cancelled.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    struct Fixture {
        address: std::net::SocketAddr,
        probe: Arc<Probe>,
        stop: Option<tokio::sync::oneshot::Sender<()>>,
        server: Option<tokio::task::JoinHandle<io::Result<()>>>,
    }

    fn fixture_limits(idle_ms: u64) -> Limits {
        Limits {
            header: Duration::from_millis(500),
            idle: Duration::from_millis(idle_ms),
            write: Duration::from_millis(350),
            shutdown: Duration::from_millis(400),
        }
    }

    async fn fixture(idle_ms: u64, handler_ms: u64) -> Fixture {
        let probe = Arc::new(Probe::default());
        let shared = probe.clone();
        let handler = move |body: axum::body::Bytes| {
            let probe = shared.clone();
            async move {
                probe.entered.fetch_add(1, Ordering::SeqCst);
                let mut guard = Guard(probe.clone(), false);
                tokio::time::sleep(Duration::from_millis(handler_ms)).await;
                guard.1 = true;
                probe.completed.fetch_add(1, Ordering::SeqCst);
                format!("handler-complete:{}", body.len())
            }
        };
        let app = Router::new()
            .route("/", axum::routing::any(handler))
            .route("/ok", axum::routing::get(|| async { "ok" }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(serve(listener, app, fixture_limits(idle_ms), async {
            let _ = stopped.await;
        }));
        Fixture {
            address,
            probe,
            stop: Some(stop),
            server: Some(server),
        }
    }

    impl Fixture {
        async fn connect(&self) -> tokio::net::TcpStream {
            tokio::net::TcpStream::connect(self.address).await.unwrap()
        }

        async fn finish(mut self) {
            self.stop.take().unwrap().send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(2), self.server.take().unwrap())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
    }

    /// Reads until `marker` appears, the peer closes or `limit` passes.
    async fn read_until(
        socket: &mut tokio::net::TcpStream,
        marker: &str,
        limit: Duration,
    ) -> (String, bool) {
        let mut raw = Vec::new();
        let mut closed = false;
        let _ = tokio::time::timeout(limit, async {
            let mut buf = [0u8; 4096];
            loop {
                match socket.read(&mut buf).await {
                    Ok(0) | Err(_) => {
                        closed = true;
                        break;
                    }
                    Ok(n) => {
                        raw.extend_from_slice(&buf[..n]);
                        if String::from_utf8_lossy(&raw).contains(marker) {
                            break;
                        }
                    }
                }
            }
        })
        .await;
        (String::from_utf8_lossy(&raw).into_owned(), closed)
    }

    const WAIT: Duration = Duration::from_millis(900);

    #[tokio::test]
    async fn completed_bodies_survive_the_idle_interval_while_the_handler_runs() {
        // (label, writes with a pause after the first one, expected body length)
        let cases: [(&str, &[&[u8]], usize); 5] = [
            (
                "content-length",
                &[
                    b"POST / HTTP/1.1\r\nHost: l\r\nContent-Length: 2\r\n\r\n",
                    b"xy",
                ],
                2,
            ),
            (
                "body in the initial read",
                &[b"POST / HTTP/1.1\r\nHost: l\r\nContent-Length: 2\r\n\r\nxy"],
                2,
            ),
            (
                "chunked",
                &[
                    b"POST / HTTP/1.1\r\nHost: l\r\nTransfer-Encoding: chunked\r\n\r\n",
                    b"2\r\nxy\r\n0\r\n\r\n",
                ],
                2,
            ),
            (
                "empty content-length",
                &[b"POST / HTTP/1.1\r\nHost: l\r\nContent-Length: 0\r\n\r\n"],
                0,
            ),
            ("bodyless get", &[b"GET / HTTP/1.1\r\nHost: l\r\n\r\n"], 0),
        ];
        for (label, writes, length) in cases {
            let fixture = fixture(50, 150).await;
            let mut socket = fixture.connect().await;
            for (index, write) in writes.iter().enumerate() {
                if index > 0 {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                socket.write_all(write).await.unwrap();
            }
            let started = Instant::now();
            let (raw, closed) = read_until(&mut socket, "handler-complete", WAIT).await;
            assert!(
                raw.contains(&format!("handler-complete:{length}")) && !closed,
                "{label}: {raw:?}"
            );
            assert!(started.elapsed() >= Duration::from_millis(140), "{label}");
            assert_eq!(Probe::count(&fixture.probe.completed), 1, "{label}");
            assert_eq!(Probe::count(&fixture.probe.cancelled), 0, "{label}");
            // Idle protection returns for the next keep-alive wait.
            let waited = Instant::now();
            let (_, closed) = read_until(&mut socket, "never", WAIT).await;
            assert!(closed, "{label}: idle keep-alive connection stays open");
            assert!(waited.elapsed() < Duration::from_millis(500), "{label}");
            fixture.finish().await;
        }
    }

    #[tokio::test]
    async fn fast_handlers_and_a_larger_idle_interval_still_succeed() {
        for (idle, handler) in [(50, 20), (400, 150)] {
            let fixture = fixture(idle, handler).await;
            let mut socket = fixture.connect().await;
            socket
                .write_all(b"POST / HTTP/1.1\r\nHost: l\r\nContent-Length: 2\r\n\r\nxy")
                .await
                .unwrap();
            let (raw, _) = read_until(&mut socket, "handler-complete", WAIT).await;
            assert!(raw.contains("handler-complete:2"), "{raw:?}");
            fixture.finish().await;
        }
    }

    #[tokio::test]
    async fn an_incomplete_body_is_still_bounded_by_the_idle_interval() {
        let fixture = fixture(50, 150).await;
        let mut socket = fixture.connect().await;
        socket
            .write_all(b"POST / HTTP/1.1\r\nHost: l\r\nContent-Length: 2\r\n\r\nx")
            .await
            .unwrap();
        let started = Instant::now();
        let (raw, closed) = read_until(&mut socket, "handler-complete", WAIT).await;
        assert!(closed && !raw.contains("handler-complete"), "{raw:?}");
        assert!(started.elapsed() >= Duration::from_millis(40));
        assert!(started.elapsed() < Duration::from_millis(500));
        assert_eq!(Probe::count(&fixture.probe.entered), 0);
        fixture.finish().await;
    }

    #[tokio::test]
    async fn idle_keep_alive_connections_close_but_active_reuse_succeeds() {
        let fixture = fixture(100, 0).await;
        let mut socket = fixture.connect().await;
        for _ in 0..3 {
            socket
                .write_all(b"GET /ok HTTP/1.1\r\nHost: l\r\n\r\n")
                .await
                .unwrap();
            let (raw, closed) = read_until(&mut socket, "\r\n\r\nok", WAIT).await;
            assert!(raw.contains("200 OK") && !closed, "{raw:?}");
            tokio::time::sleep(Duration::from_millis(60)).await;
        }
        let started = Instant::now();
        let (_, closed) = read_until(&mut socket, "never", WAIT).await;
        assert!(closed);
        assert!(started.elapsed() < Duration::from_millis(400));
        fixture.finish().await;
    }

    #[tokio::test]
    async fn a_closed_client_cancels_the_handler_promptly() {
        // The idle interval is longer than the whole scenario, so only socket closure can cancel.
        let fixture = fixture(5_000, 3_000).await;
        let mut socket = fixture.connect().await;
        socket
            .write_all(b"POST / HTTP/1.1\r\nHost: l\r\nContent-Length: 2\r\n\r\nxy")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(Probe::count(&fixture.probe.entered), 1);
        drop(socket);
        for _ in 0..50 {
            if Probe::count(&fixture.probe.cancelled) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(Probe::count(&fixture.probe.cancelled), 1);
        assert_eq!(Probe::count(&fixture.probe.completed), 0);
        fixture.finish().await;
    }

    #[tokio::test]
    async fn idle_reads_are_suspended_only_while_a_completed_request_awaits_its_response() {
        let (peer, io) = tokio::io::duplex(64);
        let control = Arc::new(IdleControl::default());
        let mut io = Box::pin(timed_io(io, limits(), control.clone()));
        let reader = tokio::spawn(async move {
            let first = io.read_u8().await;
            (first, io.read_u8().await)
        });
        // Suspending while the 100ms timer is already pending must disarm it.
        tokio::time::sleep(Duration::from_millis(30)).await;
        control.set_suspended(true);
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(!reader.is_finished(), "suspended reads must not time out");
        let mut peer = peer;
        peer.write_all(&[7]).await.unwrap();
        // Resuming restarts a fresh timer for the next wait.
        tokio::time::sleep(Duration::from_millis(20)).await;
        control.set_suspended(false);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!reader.is_finished(), "the restored timer starts fresh");
        let (first, second) = tokio::time::timeout(Duration::from_secs(1), reader)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.unwrap(), 7);
        assert_eq!(second.unwrap_err().kind(), io::ErrorKind::TimedOut);
        drop(peer);
    }

    #[tokio::test]
    async fn tracking_marks_http1_exchanges_and_leaves_http2_untouched() {
        for (version, tracked) in [
            (hyper::Version::HTTP_11, true),
            (hyper::Version::HTTP_2, false),
        ] {
            let control = Arc::new(IdleControl::default());
            let request = hyper::Request::builder()
                .version(version)
                .body(AxumBody::from("xy"))
                .unwrap();
            let (request, exchange) = track_request(request, &control);
            assert_eq!(exchange.is_some(), tracked);
            assert!(!control.suspended.load(Ordering::SeqCst), "body unread");
            let bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(bytes.len(), 2);
            assert_eq!(control.suspended.load(Ordering::SeqCst), tracked);
            if let Some(exchange) = exchange {
                // A finished response wins over a late request-body completion.
                exchange.response_finished();
                assert!(!control.suspended.load(Ordering::SeqCst));
                exchange.request_body_complete();
                assert!(!control.suspended.load(Ordering::SeqCst));
            }
        }
        let control = Arc::new(IdleControl::default());
        let request = hyper::Request::builder().body(AxumBody::empty()).unwrap();
        let (_request, exchange) = track_request(request, &control);
        assert!(
            control.suspended.load(Ordering::SeqCst),
            "empty bodies are complete"
        );
        drop(ResponseBody {
            inner: AxumBody::from("x"),
            exchange: exchange.unwrap(),
        });
        assert!(!control.suspended.load(Ordering::SeqCst));
    }
}
