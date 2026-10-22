//! Received-message size limit for DevBase requests.
//!
//! The shared handler only sees the decoded request, so JSON whitespace, escape sequences and
//! non-canonical Protobuf encodings are invisible to it. A first-position unary interceptor
//! therefore measures the byte length of the message exactly as the transport delivered it
//! (already decompressed and already bounded by the transport message cap). The value is
//! derived from the consumed body, never from a caller header such as `Content-Length`.
//!
//! The interceptor is the earliest hook of the generated Connect stack that sees the received
//! bytes: it runs after the bounded body read and before the lazy message decode. It rejects an
//! over-limit message there, so a malformed over-limit ProtoJSON or Protobuf body receives the
//! same typed error as a well-formed one instead of the dispatcher's bare decode error. The
//! rejection therefore happens before decoding, authentication and rate limiting, which need
//! the decoded principal; it consumes no limiter capacity.
use connectrpc::{
    ConnectError, ErrorCode, Interceptor, Next, async_trait,
    interceptor::{UnaryRequest, UnaryResponse},
};
use server_common::{Fault, http::Accounting};
use std::time::Instant;

/// Maximum size of one received DevBase request message, inclusive.
pub(crate) const MAX_MESSAGE_BYTES: usize = 64 << 10;

/// Byte length of the received request message. Internal metadata; never read from headers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ReceivedMessageSize(pub(crate) usize);

/// Measures the received message, rejects an over-limit one before decoding and otherwise
/// stores the length in the request extensions for the shared handler.
pub(crate) struct LimitReceivedSize;

#[async_trait]
impl Interceptor for LimitReceivedSize {
    async fn intercept_unary(
        &self,
        mut req: UnaryRequest,
        next: Next<'_>,
    ) -> Result<UnaryResponse, ConnectError> {
        let started = Instant::now();
        let received = req.payload.bytes().len();
        // `insert` replaces any value a caller-influenced layer could have placed earlier.
        req.ctx
            .extensions_mut()
            .insert(ReceivedMessageSize(received));
        if received > MAX_MESSAGE_BYTES {
            let trace = server_common::id();
            let procedure = req.ctx.path().unwrap_or_default();
            tracing::warn!(trace_id=%trace,procedure,received_bytes=received,limit_bytes=MAX_MESSAGE_BYTES,"request message exceeds size limit");
            // The handler never runs, so publish its outcome for the request metrics here; the
            // HTTP middleware records it exactly once with the response that is returned.
            if let Some(accounting) = req.ctx.extensions().get::<Accounting>() {
                accounting.report(ErrorCode::InvalidArgument.as_str(), started.elapsed());
            }
            return Err(crate::typed_error(
                Fault::invalid("request exceeds 64 KiB"),
                &trace,
            ));
        }
        next.run(req).await
    }
}
