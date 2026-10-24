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
    time::{Instant, Sleep},
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

// ---------------------------------------------------------------------------------------------
// HTTP/2 header-block deadline
//
// WORKAROUND: the pinned hyper and h2 releases expose no deadline for assembling a HEADERS (plus
// CONTINUATION) block: hyper only dispatches complete requests, its HTTP/2 keep-alive settings
// measure PING acknowledgements, and the socket idle timeout is reset by every received byte. The
// observer below therefore walks HTTP/2 frame boundaries on the raw socket reads and enforces
// `Limits.header` as an absolute deadline for each header block after the first-request guard in
// `serve` has been disarmed. Scope: inbound bytes only; bytes are forwarded unchanged and the
// observer never buffers payloads or decodes HPACK. Removal condition: delete this section when
// the pinned hyper/h2 versions provide an equivalent header-block deadline for HTTP/2 servers and
// the regression tests in this module pass against that API.
// ---------------------------------------------------------------------------------------------

/// HTTP/2 client connection preface that starts every prior-knowledge connection.
const H2_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
const FRAME_HEADER_LEN: usize = 9;
const FRAME_TYPE_HEADERS: u8 = 0x1;
const FRAME_TYPE_CONTINUATION: u8 = 0x9;
const FLAG_END_HEADERS: u8 = 0x4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Matching the 24-byte client preface; the value is the number of matched bytes.
    Preface(u8),
    /// Walking HTTP/2 frame boundaries.
    Frames,
    /// Not HTTP/2 (HTTP/1 or a mismatched preface): never inspected again.
    Passthrough,
}

/// What one chunk of inbound bytes did to the header block state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Observation {
    /// A header block that was already open when the chunk arrived reached `END_HEADERS`.
    closed_existing: bool,
    /// A header block is still incomplete after the chunk.
    open_at_exit: bool,
    /// The block that is open after the chunk started inside this chunk.
    opened_in_chunk: bool,
}

/// Bounded HTTP/2 frame walker. It keeps the 9-byte frame header, the remaining payload length
/// and whether one header block is open. It never buffers payloads, decodes HPACK or validates
/// the protocol; hyper and h2 remain responsible for rejecting malformed input.
#[derive(Debug)]
struct FrameTracker {
    phase: Phase,
    header: [u8; FRAME_HEADER_LEN],
    header_len: usize,
    payload_remaining: u32,
    /// The current frame is the HEADERS or CONTINUATION frame that starts or extends the block.
    frame_extends_block: bool,
    block_open: bool,
    /// The open block started during the current `observe` call.
    block_new_in_chunk: bool,
}

impl FrameTracker {
    fn new() -> Self {
        Self {
            phase: Phase::Preface(0),
            header: [0; FRAME_HEADER_LEN],
            header_len: 0,
            payload_remaining: 0,
            frame_extends_block: false,
            block_open: false,
            block_new_in_chunk: false,
        }
    }

    fn observe(&mut self, mut bytes: &[u8]) -> Observation {
        let mut closed_existing = false;
        self.block_new_in_chunk = false;
        while !bytes.is_empty() {
            match self.phase {
                Phase::Passthrough => break,
                Phase::Preface(matched) => {
                    let matched = usize::from(matched);
                    let take = bytes.len().min(H2_PREFACE.len() - matched);
                    if bytes[..take] != H2_PREFACE[matched..matched + take] {
                        self.phase = Phase::Passthrough;
                        break;
                    }
                    bytes = &bytes[take..];
                    let matched = matched + take;
                    self.phase = if matched == H2_PREFACE.len() {
                        Phase::Frames
                    } else {
                        Phase::Preface(matched as u8)
                    };
                }
                Phase::Frames if self.header_len < FRAME_HEADER_LEN => {
                    let take = bytes.len().min(FRAME_HEADER_LEN - self.header_len);
                    for byte in &bytes[..take] {
                        self.header[self.header_len] = *byte;
                        self.header_len += 1;
                        // The frame type is identifiable at the fourth octet (index 3).
                        if self.header_len == 4 {
                            if *byte == FRAME_TYPE_HEADERS && !self.block_open {
                                self.block_open = true;
                                self.block_new_in_chunk = true;
                                self.frame_extends_block = true;
                            } else {
                                self.frame_extends_block =
                                    *byte == FRAME_TYPE_CONTINUATION && self.block_open;
                            }
                        }
                    }
                    bytes = &bytes[take..];
                    if self.header_len == FRAME_HEADER_LEN {
                        self.payload_remaining =
                            u32::from_be_bytes([0, self.header[0], self.header[1], self.header[2]]);
                        if self.payload_remaining == 0 {
                            self.finish_frame(&mut closed_existing);
                        }
                    }
                }
                Phase::Frames => {
                    let take = bytes.len().min(self.payload_remaining as usize);
                    self.payload_remaining -= take as u32;
                    bytes = &bytes[take..];
                    if self.payload_remaining == 0 {
                        self.finish_frame(&mut closed_existing);
                    }
                }
            }
        }
        Observation {
            closed_existing,
            open_at_exit: self.block_open,
            opened_in_chunk: self.block_open && self.block_new_in_chunk,
        }
    }

    /// Completes the current frame. Only the frame that starts or extends the open block may
    /// close it, so out-of-sequence frames cannot clear a pending deadline.
    fn finish_frame(&mut self, closed_existing: &mut bool) {
        if self.frame_extends_block && self.header[4] & FLAG_END_HEADERS != 0 {
            self.block_open = false;
            if !self.block_new_in_chunk {
                *closed_existing = true;
            }
        }
        self.header_len = 0;
        self.frame_extends_block = false;
    }
}

#[derive(Default)]
struct GuardState {
    deadline: Option<Instant>,
    expired: bool,
}

/// Header-block deadline shared between the socket observer and the connection driver.
/// Expiry is sticky: once set it is never cleared or replaced.
struct HeaderGuard {
    limit: Duration,
    state: Mutex<GuardState>,
    changed: Notify,
}

impl HeaderGuard {
    fn new(limit: Duration) -> Arc<Self> {
        Arc::new(Self {
            limit,
            state: Mutex::new(GuardState::default()),
            changed: Notify::new(),
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, GuardState> {
        // The state is plain data, so a poisoned lock is still consistent.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn is_expired(&self) -> bool {
        self.state().expired
    }

    /// Records the observation of one inbound chunk. Returns false when the deadline had already
    /// elapsed (or elapsed while the chunk was pending), in which case the chunk must be dropped.
    fn record(&self, observation: Observation, now: Instant) -> bool {
        let mut state = self.state();
        if state.expired {
            return false;
        }
        if state.deadline.is_some_and(|deadline| now >= deadline) {
            state.expired = true;
            drop(state);
            self.changed.notify_one();
            return false;
        }
        let before = state.deadline;
        state.deadline = if !observation.open_at_exit {
            None
        } else if observation.opened_in_chunk || before.is_none() {
            Some(now + self.limit)
        } else {
            before
        };
        let changed = before != state.deadline;
        drop(state);
        if changed {
            self.changed.notify_one();
        }
        true
    }

    /// Marks an already elapsed deadline as expired. Used by the pre-read check in the observer.
    fn check(&self, now: Instant) -> bool {
        let mut state = self.state();
        if !state.expired && state.deadline.is_some_and(|deadline| now >= deadline) {
            state.expired = true;
            drop(state);
            self.changed.notify_one();
            return false;
        }
        !state.expired
    }

    /// Resolves once the pending header block has outlived the limit. Never resolves otherwise.
    async fn expired(self: Arc<Self>) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            // Register before reading the state so a concurrent update cannot be missed.
            notified.as_mut().enable();
            let deadline = {
                let mut state = self.state();
                if state.expired {
                    return;
                }
                if state.deadline.is_some_and(|d| Instant::now() >= d) {
                    state.expired = true;
                    return;
                }
                state.deadline
            };
            match deadline {
                Some(deadline) => {
                    tokio::select! {
                        _ = tokio::time::sleep_until(deadline) => {},
                        _ = &mut notified => {},
                    }
                }
                None => notified.await,
            }
        }
    }
}

/// Read-side observer that feeds raw socket bytes to a `FrameTracker` and rejects the read that
/// would complete a header block after its deadline. Writes pass through untouched.
struct Observed<T> {
    inner: T,
    tracker: FrameTracker,
    guard: Arc<HeaderGuard>,
}

impl<T> Observed<T> {
    fn new(inner: T, guard: Arc<HeaderGuard>) -> Self {
        Self {
            inner,
            tracker: FrameTracker::new(),
            guard,
        }
    }
}

fn header_timeout_error() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "header block deadline exceeded")
}

impl<T: AsyncRead + Unpin> AsyncRead for Observed<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        if !this.guard.check(Instant::now()) {
            return Poll::Ready(Err(header_timeout_error()));
        }
        let start = buf.filled().len();
        match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                let observation = this.tracker.observe(&buf.filled()[start..]);
                if !this.guard.record(observation, Instant::now()) {
                    // Never hand bytes that complete an expired block to hyper.
                    buf.set_filled(start);
                    return Poll::Ready(Err(header_timeout_error()));
                }
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Observed<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
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

fn log_closed(errored: bool, guard: &HeaderGuard) {
    if guard.is_expired() {
        tracing::debug!(category = "header_timeout", "connection closed");
    } else if errored {
        tracing::debug!(category = "transport", "connection closed");
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
                    let guard = HeaderGuard::new(limits.header);
                    let io = TokioIo::new(Box::pin(timed_io(Observed::new(socket, guard.clone()), limits, control)));
                    let connection = builder.serve_connection(io, service);
                    tokio::pin!(connection);
                    // The auto-protocol detector runs before Hyper's HTTP/1 header timer.
                    // Bound the first request too, including silent sockets and partial prefaces.
                    let first = tokio::time::sleep(limits.header);
                    tokio::pin!(first);
                    // Absolute deadline for any later HTTP/2 header block; see the observer above.
                    let header_block = guard.clone().expired();
                    tokio::pin!(header_block);
                    tokio::select! {
                        result = &mut connection => {
                            log_closed(result.is_err(), &guard);
                            return;
                        },
                        _ = &mut header_block => { tracing::debug!(category = "header_timeout", "connection closed"); return; },
                        _ = &mut first => { tracing::debug!(category = "header_timeout", "connection closed"); return; },
                        _ = stopped.changed() => {},
                        _ = first_request.notified() => {
                            tokio::select! {
                                result = &mut connection => {
                                    log_closed(result.is_err(), &guard);
                                    return;
                                },
                                _ = &mut header_block => { tracing::debug!(category = "header_timeout", "connection closed"); return; },
                                _ = stopped.changed() => {},
                            }
                        },
                    }
                    connection.as_mut().graceful_shutdown();
                    // Graceful draining must not bypass an active header deadline.
                    tokio::select! {
                        _ = tokio::time::timeout(limits.shutdown, &mut connection) => {},
                        _ = &mut header_block => { tracing::debug!(category = "header_timeout", "connection closed"); },
                    }
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

    // --- Frame tracker and observer unit tests ---------------------------------------------

    fn frame(kind: u8, flags: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
        let len = payload.len() as u32;
        let mut out = vec![(len >> 16) as u8, (len >> 8) as u8, len as u8, kind, flags];
        out.extend_from_slice(&stream.to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    fn primed_tracker() -> FrameTracker {
        let mut tracker = FrameTracker::new();
        assert_eq!(tracker.observe(H2_PREFACE), Observation::default());
        tracker
    }

    #[test]
    fn tracker_opens_at_type_octet_and_closes_at_end_headers_payload() {
        let bytes = frame(0x1, 0x5, 1, &[1, 2, 3, 4]);
        for split in 1..bytes.len() {
            let mut tracker = primed_tracker();
            let first = tracker.observe(&bytes[..split]);
            // Type octet is index 3, so the block opens once four bytes arrived.
            assert_eq!(first.open_at_exit, split >= 4, "split {split}");
            assert_eq!(first.opened_in_chunk, split >= 4, "split {split}");
            let second = tracker.observe(&bytes[split..]);
            assert!(!second.open_at_exit);
            assert_eq!(second.closed_existing, split >= 4, "split {split}");
        }
    }

    #[test]
    fn tracker_handles_every_byte_boundary_with_continuation() {
        let mut bytes = frame(0x1, 0x1, 3, &[9; 5]);
        bytes.extend(frame(0x9, 0x0, 3, &[8; 3]));
        bytes.extend(frame(0x9, 0x4, 3, &[7; 2]));
        let mut tracker = primed_tracker();
        let mut open_after_last = false;
        let mut closed = 0;
        for (index, byte) in bytes.iter().enumerate() {
            let observation = tracker.observe(std::slice::from_ref(byte));
            if observation.closed_existing {
                closed += 1;
            }
            open_after_last = observation.open_at_exit;
            if index + 1 < bytes.len() && index >= 3 {
                assert!(observation.open_at_exit, "byte {index}");
            }
        }
        assert_eq!(closed, 1);
        assert!(!open_after_last);
    }

    #[test]
    fn tracker_handles_zero_length_and_multiple_frames_per_read() {
        let mut tracker = primed_tracker();
        // Zero-length HEADERS|END_HEADERS opens and closes inside one chunk.
        let observation = tracker.observe(&frame(0x1, 0x4, 1, &[]));
        assert_eq!(
            observation,
            Observation {
                closed_existing: false,
                open_at_exit: false,
                opened_in_chunk: false
            }
        );
        // Zero-length HEADERS without END_HEADERS stays open; empty CONTINUATION closes it.
        let observation = tracker.observe(&frame(0x1, 0x1, 3, &[]));
        assert!(observation.open_at_exit && observation.opened_in_chunk);
        let mut chunk = frame(0x9, 0x4, 3, &[]);
        chunk.extend(frame(0x4, 0x0, 0, &[]));
        chunk.extend(frame(0x1, 0x0, 5, &[1]));
        let observation = tracker.observe(&chunk);
        assert!(observation.closed_existing);
        assert!(observation.open_at_exit && observation.opened_in_chunk);
    }

    #[test]
    fn tracker_ignores_out_of_sequence_frames_and_non_header_frames() {
        let mut tracker = primed_tracker();
        // DATA with END_HEADERS-valued flag bit and a stray CONTINUATION never open a block.
        assert_eq!(
            tracker.observe(&frame(0x0, 0x4, 1, &[1, 2])),
            Observation::default()
        );
        assert_eq!(
            tracker.observe(&frame(0x9, 0x4, 1, &[1])),
            Observation::default()
        );
        // An unrelated frame carrying 0x4 must not close an open block.
        let observation = tracker.observe(&frame(0x1, 0x1, 1, &[1]));
        assert!(observation.open_at_exit);
        let observation = tracker.observe(&frame(0x0, 0x4, 1, &[1]));
        assert!(observation.open_at_exit && !observation.closed_existing);
        // A second HEADERS cannot replace or close the pending block.
        let observation = tracker.observe(&frame(0x1, 0x4, 3, &[1]));
        assert!(observation.open_at_exit && !observation.closed_existing);
        assert!(!observation.opened_in_chunk);
    }

    #[test]
    fn tracker_passes_through_http1_and_mismatched_preface() {
        let mut tracker = FrameTracker::new();
        assert_eq!(
            tracker.observe(b"GET / HTTP/1.1\r\n"),
            Observation::default()
        );
        assert_eq!(tracker.phase, Phase::Passthrough);
        let mut tracker = FrameTracker::new();
        tracker.observe(&H2_PREFACE[..10]);
        assert_eq!(tracker.phase, Phase::Preface(10));
        tracker.observe(&H2_PREFACE[10..]);
        assert_eq!(tracker.phase, Phase::Frames);
        let mut tracker = FrameTracker::new();
        tracker.observe(&H2_PREFACE[..10]);
        tracker.observe(b"X");
        assert_eq!(tracker.phase, Phase::Passthrough);
        assert!(
            std::mem::size_of::<FrameTracker>() <= 64,
            "tracker state must stay bounded"
        );
    }

    #[tokio::test]
    async fn observer_forwards_bytes_unchanged() {
        let (mut client, server) = tokio::io::duplex(4096);
        let guard = HeaderGuard::new(Duration::from_secs(5));
        let mut observed = Observed::new(server, guard);
        let mut sent = H2_PREFACE.to_vec();
        sent.extend(frame(0x1, 0x4, 1, &[1, 2, 3]));
        sent.extend(frame(0x0, 0x1, 1, &[4, 5]));
        client.write_all(&sent).await.unwrap();
        let mut received = vec![0; sent.len()];
        observed.read_exact(&mut received).await.unwrap();
        assert_eq!(received, sent);
    }

    #[tokio::test]
    async fn observer_does_not_deliver_bytes_completing_an_expired_block() {
        let (mut client, server) = tokio::io::duplex(4096);
        let guard = HeaderGuard::new(Duration::from_millis(40));
        let mut observed = Observed::new(server, guard.clone());
        let mut head = H2_PREFACE.to_vec();
        head.extend(frame(0x1, 0x4, 3, &[1, 2, 3, 4])[..11].to_vec());
        client.write_all(&head).await.unwrap();
        let mut buf = vec![0; head.len()];
        observed.read_exact(&mut buf).await.unwrap();
        // Let the deadline elapse, then make the completing bytes ready before the next poll.
        tokio::time::sleep(Duration::from_millis(80)).await;
        client.write_all(&[3, 4]).await.unwrap();
        let mut rest = [0u8; 2];
        let error = observed.read_exact(&mut rest).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(rest, [0, 0], "late completion must not be delivered");
        assert!(guard.is_expired());
        // Expiry is sticky for later reads and for a replacing HEADERS frame.
        client.write_all(&frame(0x1, 0x4, 5, &[1])).await.unwrap();
        assert!(observed.read_u8().await.is_err());
        assert!(guard.is_expired());
    }

    #[tokio::test]
    async fn expired_future_fires_for_pending_block_and_not_for_idle() {
        let guard = HeaderGuard::new(Duration::from_millis(50));
        let idle = tokio::time::timeout(Duration::from_millis(120), guard.clone().expired()).await;
        assert!(idle.is_err(), "no pending block must never expire");
        let waiter = tokio::spawn(guard.clone().expired());
        tokio::time::sleep(Duration::from_millis(10)).await;
        guard.record(
            Observation {
                closed_existing: false,
                open_at_exit: true,
                opened_in_chunk: true,
            },
            Instant::now(),
        );
        tokio::time::timeout(Duration::from_millis(500), waiter)
            .await
            .expect("pending block must expire")
            .unwrap();
        assert!(guard.is_expired());
    }

    #[tokio::test]
    async fn completed_block_clears_the_deadline() {
        let guard = HeaderGuard::new(Duration::from_millis(50));
        let open = Observation {
            closed_existing: false,
            open_at_exit: true,
            opened_in_chunk: true,
        };
        assert!(guard.record(open, Instant::now()));
        assert!(guard.record(
            Observation {
                closed_existing: true,
                ..Default::default()
            },
            Instant::now()
        ));
        let outcome =
            tokio::time::timeout(Duration::from_millis(150), guard.clone().expired()).await;
        assert!(outcome.is_err());
        assert!(!guard.is_expired());
    }

    // --- Loopback HTTP/2 raw-frame tests -----------------------------------------------------

    use std::sync::atomic::{AtomicUsize, Ordering};

    const GET_HEALTHZ: [u8; 23] = [
        0x82, 0x86, 0x04, 0x08, b'/', b'h', b'e', b'a', b'l', b't', b'h', b'z', 0x01, 0x09, b'l',
        b'o', b'c', b'a', b'l', b'h', b'o', b's', b't',
    ];
    const POST_HEALTHZ: [u8; 23] = [
        0x83, 0x86, 0x04, 0x08, b'/', b'h', b'e', b'a', b'l', b't', b'h', b'z', 0x01, 0x09, b'l',
        b'o', b'c', b'a', b'l', b'h', b'o', b's', b't',
    ];

    fn h2_limits() -> Limits {
        Limits {
            header: Duration::from_millis(200),
            idle: Duration::from_secs(2),
            write: Duration::from_secs(2),
            shutdown: Duration::from_secs(3),
        }
    }

    struct Fixture {
        address: std::net::SocketAddr,
        calls: Arc<AtomicUsize>,
        stop: Option<tokio::sync::oneshot::Sender<()>>,
        server: tokio::task::JoinHandle<io::Result<()>>,
    }

    impl Fixture {
        async fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let calls = Arc::new(AtomicUsize::new(0));
            let counter = calls.clone();
            let post_counter = calls.clone();
            let app = Router::new().route(
                "/healthz",
                axum::routing::get(move || {
                    let counter = counter.clone();
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        "ok"
                    }
                })
                .post(move |request: axum::extract::Request| {
                    let counter = post_counter.clone();
                    async move {
                        // Count at dispatch, before the body is read.
                        counter.fetch_add(1, Ordering::SeqCst);
                        axum::body::to_bytes(request.into_body(), 1 << 16)
                            .await
                            .map(|bytes| bytes.to_vec())
                            .unwrap_or_default()
                    }
                }),
            );
            let (stop, stopped) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(serve(listener, app, h2_limits(), async {
                let _ = stopped.await;
            }));
            Self {
                address,
                calls,
                stop: Some(stop),
                server,
            }
        }

        async fn connect(&self) -> tokio::net::TcpStream {
            let mut socket = tokio::net::TcpStream::connect(self.address).await.unwrap();
            socket.write_all(H2_PREFACE).await.unwrap();
            socket.write_all(&frame(0x4, 0, 0, &[])).await.unwrap();
            socket
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        async fn finish(mut self) {
            let _ = self.stop.take().unwrap().send(());
            tokio::time::timeout(Duration::from_secs(5), self.server)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
    }

    async fn read_frame(socket: &mut tokio::net::TcpStream) -> io::Result<(u8, u8, u32, Vec<u8>)> {
        let mut header = [0u8; 9];
        socket.read_exact(&mut header).await?;
        let len = u32::from_be_bytes([0, header[0], header[1], header[2]]) as usize;
        let stream = u32::from_be_bytes([header[5] & 0x7f, header[6], header[7], header[8]]);
        let mut payload = vec![0; len];
        socket.read_exact(&mut payload).await?;
        Ok((header[3], header[4], stream, payload))
    }

    /// Reads frames (acknowledging SETTINGS) until the response HEADERS of `stream` arrive.
    async fn await_response(socket: &mut tokio::net::TcpStream, stream: u32) -> io::Result<u8> {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let (kind, flags, id, payload) = read_frame(socket).await?;
                if kind == 0x4 && flags & 0x1 == 0 {
                    socket.write_all(&frame(0x4, 0x1, 0, &[])).await?;
                } else if kind == 0x1 && id == stream {
                    return Ok(payload[0]);
                }
            }
        })
        .await
        .unwrap_or_else(|_| Err(io::Error::new(io::ErrorKind::TimedOut, "no response")))
    }

    async fn assert_closed(socket: &mut tokio::net::TcpStream, within: Duration) {
        let closed = tokio::time::timeout(within, async {
            loop {
                match read_frame(socket).await {
                    Ok(_) => continue,
                    Err(_) => return,
                }
            }
        })
        .await;
        assert!(
            closed.is_ok(),
            "connection must be closed within {within:?}"
        );
    }

    const STATUS_200: u8 = 0x88;

    async fn first_request(fixture: &Fixture, socket: &mut tokio::net::TcpStream) {
        socket
            .write_all(&frame(0x1, 0x5, 1, &GET_HEALTHZ))
            .await
            .unwrap();
        assert_eq!(await_response(socket, 1).await.unwrap(), STATUS_200);
        assert_eq!(fixture.calls(), 1);
    }

    #[tokio::test]
    async fn later_incomplete_h2_headers_are_closed_before_router() {
        let fixture = Fixture::start().await;
        let mut socket = fixture.connect().await;
        first_request(&fixture, &mut socket).await;
        let request = frame(0x1, 0x5, 3, &GET_HEALTHZ);
        socket.write_all(&request[..11]).await.unwrap();
        // Between the header limit (200 ms) and the idle limit (2 s).
        tokio::time::sleep(Duration::from_millis(450)).await;
        let _ = socket.write_all(&request[11..]).await;
        assert_closed(&mut socket, Duration::from_secs(1)).await;
        assert_eq!(fixture.calls(), 1, "late block must not reach the router");
        fixture.finish().await;
    }

    #[tokio::test]
    async fn later_h2_headers_completed_promptly_are_accepted() {
        let fixture = Fixture::start().await;
        let mut socket = fixture.connect().await;
        first_request(&fixture, &mut socket).await;
        // Pure idle longer than the header limit but below the idle limit does not close.
        tokio::time::sleep(Duration::from_millis(450)).await;
        let request = frame(0x1, 0x5, 3, &GET_HEALTHZ);
        socket.write_all(&request[..11]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        socket.write_all(&request[11..]).await.unwrap();
        assert_eq!(await_response(&mut socket, 3).await.unwrap(), STATUS_200);
        assert_eq!(fixture.calls(), 2);
        drop(socket);
        fixture.finish().await;
    }

    #[tokio::test]
    async fn drip_fed_bytes_do_not_extend_the_deadline() {
        let fixture = Fixture::start().await;
        let mut socket = fixture.connect().await;
        first_request(&fixture, &mut socket).await;
        let request = frame(0x1, 0x5, 3, &GET_HEALTHZ);
        for byte in &request[..request.len() - 1] {
            if socket.write_all(std::slice::from_ref(byte)).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        let _ = socket.write_all(&request[request.len() - 1..]).await;
        assert_closed(&mut socket, Duration::from_secs(1)).await;
        assert_eq!(fixture.calls(), 1);
        fixture.finish().await;
    }

    #[tokio::test]
    async fn continuation_blocks_follow_the_same_deadline() {
        let fixture = Fixture::start().await;
        let mut socket = fixture.connect().await;
        first_request(&fixture, &mut socket).await;
        // Prompt HEADERS + CONTINUATION is accepted.
        socket
            .write_all(&frame(0x1, 0x1, 3, &GET_HEALTHZ[..10]))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        socket
            .write_all(&frame(0x9, 0x4, 3, &GET_HEALTHZ[10..]))
            .await
            .unwrap();
        assert_eq!(await_response(&mut socket, 3).await.unwrap(), STATUS_200);
        assert_eq!(fixture.calls(), 2);
        // A CONTINUATION arriving after the block deadline is rejected before dispatch.
        socket
            .write_all(&frame(0x1, 0x1, 5, &GET_HEALTHZ[..10]))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(450)).await;
        let _ = socket
            .write_all(&frame(0x9, 0x4, 5, &GET_HEALTHZ[10..]))
            .await;
        assert_closed(&mut socket, Duration::from_secs(1)).await;
        assert_eq!(fixture.calls(), 2);
        fixture.finish().await;
    }

    #[tokio::test]
    async fn slow_first_h2_request_is_still_closed() {
        let fixture = Fixture::start().await;
        let mut socket = fixture.connect().await;
        let request = frame(0x1, 0x5, 1, &GET_HEALTHZ);
        socket.write_all(&request[..11]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(450)).await;
        let _ = socket.write_all(&request[11..]).await;
        assert_closed(&mut socket, Duration::from_secs(1)).await;
        assert_eq!(fixture.calls(), 0);
        fixture.finish().await;
    }

    #[tokio::test]
    async fn delayed_data_body_is_not_limited_by_header_deadline() {
        let fixture = Fixture::start().await;
        let mut socket = fixture.connect().await;
        socket
            .write_all(&frame(0x1, 0x4, 1, &POST_HEALTHZ))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(450)).await;
        socket
            .write_all(&frame(0x0, 0x1, 1, b"hello"))
            .await
            .unwrap();
        assert_eq!(await_response(&mut socket, 1).await.unwrap(), STATUS_200);
        assert_eq!(fixture.calls(), 1);
        drop(socket);
        fixture.finish().await;
    }

    #[tokio::test]
    async fn expired_trailers_close_connection_without_extra_dispatch() {
        let fixture = Fixture::start().await;
        let mut socket = fixture.connect().await;
        socket
            .write_all(&frame(0x1, 0x4, 1, &POST_HEALTHZ))
            .await
            .unwrap();
        socket
            .write_all(&frame(0x0, 0x0, 1, b"hello"))
            .await
            .unwrap();
        // Partial trailer block: declared 5 bytes, only 2 sent.
        let trailers = frame(0x1, 0x5, 1, &[0u8; 5]);
        socket.write_all(&trailers[..11]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(450)).await;
        let _ = socket.write_all(&trailers[11..]).await;
        assert_closed(&mut socket, Duration::from_secs(1)).await;
        assert_eq!(fixture.calls(), 1);
        fixture.finish().await;
    }

    #[tokio::test]
    async fn graceful_draining_keeps_the_header_deadline() {
        let mut fixture = Fixture::start().await;
        let mut socket = fixture.connect().await;
        first_request(&fixture, &mut socket).await;
        let request = frame(0x1, 0x5, 3, &GET_HEALTHZ);
        socket.write_all(&request[..11]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let _ = fixture.stop.take().unwrap().send(());
        // Shutdown deadline is 3 s; the header deadline must close the connection first.
        assert_closed(&mut socket, Duration::from_secs(1)).await;
        assert_eq!(fixture.calls(), 1);
        tokio::time::timeout(Duration::from_secs(5), fixture.server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn later_http1_header_protection_is_preserved() {
        let fixture = Fixture::start().await;
        let mut socket = tokio::net::TcpStream::connect(fixture.address)
            .await
            .unwrap();
        socket
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut buf = [0u8; 512];
        let read = socket.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..read]).contains("200 OK"));
        socket
            .write_all(b"GET /healthz HTTP/1.1\r\nX-Slow: ")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(450)).await;
        let _ = socket.write_all(b"1\r\nHost: localhost\r\n\r\n").await;
        let mut raw = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(1), socket.read_to_end(&mut raw)).await;
        assert!(!String::from_utf8_lossy(&raw).contains("200 OK"));
        assert_eq!(fixture.calls(), 1);
        fixture.finish().await;
    }

    // Phase-aware idle reads. The fixture mirrors the original reproduction: idle 50ms,
    // handler 150ms, and a client that keeps its socket open after sending the whole request.
    // `Instant` is tokio's, imported through `super::*`; `AtomicUsize` is imported by the
    // HTTP/2 tests above.

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

    struct CancelGuard(Arc<Probe>, bool);
    impl Drop for CancelGuard {
        fn drop(&mut self) {
            if !self.1 {
                self.0.cancelled.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    struct IdleFixture {
        address: std::net::SocketAddr,
        probe: Arc<Probe>,
        stop: Option<tokio::sync::oneshot::Sender<()>>,
        server: Option<tokio::task::JoinHandle<io::Result<()>>>,
    }

    fn idle_fixture_limits(idle_ms: u64) -> Limits {
        Limits {
            header: Duration::from_millis(500),
            idle: Duration::from_millis(idle_ms),
            write: Duration::from_millis(350),
            shutdown: Duration::from_millis(400),
        }
    }

    async fn idle_fixture(idle_ms: u64, handler_ms: u64) -> IdleFixture {
        let probe = Arc::new(Probe::default());
        let shared = probe.clone();
        let handler = move |body: axum::body::Bytes| {
            let probe = shared.clone();
            async move {
                probe.entered.fetch_add(1, Ordering::SeqCst);
                let mut guard = CancelGuard(probe.clone(), false);
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
        let server = tokio::spawn(serve(listener, app, idle_fixture_limits(idle_ms), async {
            let _ = stopped.await;
        }));
        IdleFixture {
            address,
            probe,
            stop: Some(stop),
            server: Some(server),
        }
    }

    impl IdleFixture {
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
            let fixture = idle_fixture(50, 150).await;
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
            let fixture = idle_fixture(idle, handler).await;
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
        let fixture = idle_fixture(50, 150).await;
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
        let fixture = idle_fixture(100, 0).await;
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
        let fixture = idle_fixture(5_000, 3_000).await;
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
