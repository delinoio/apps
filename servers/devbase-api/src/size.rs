// SPDX-License-Identifier: FSL-1.1-ALv2
//! Received-message size capture for the DevBase request-size limit.
//!
//! The shared handler only sees the decoded request, so JSON whitespace, escape sequences and
//! non-canonical Protobuf encodings are invisible to it. A first-position unary interceptor
//! therefore records the byte length of the message exactly as the transport delivered it
//! (already decompressed and already bounded by the transport message cap) before any
//! JSON-to-Protobuf normalization. The value is derived from the consumed body, never from a
//! caller header such as `Content-Length`, and is stored in a private request extension.
use connectrpc::{
    ConnectError, Interceptor, Next, async_trait,
    interceptor::{UnaryRequest, UnaryResponse},
};

/// Maximum size of one received DevBase request message, inclusive.
pub(crate) const MAX_MESSAGE_BYTES: usize = 64 << 10;

/// Byte length of the received request message. Internal metadata; never read from headers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ReceivedMessageSize(pub(crate) usize);

/// Stores the received message length in the request extensions for the shared handler.
pub(crate) struct CaptureReceivedSize;

#[async_trait]
impl Interceptor for CaptureReceivedSize {
    async fn intercept_unary(
        &self,
        mut req: UnaryRequest,
        next: Next<'_>,
    ) -> Result<UnaryResponse, ConnectError> {
        // `insert` replaces any value a caller-influenced layer could have placed earlier.
        let size = ReceivedMessageSize(req.payload.bytes().len());
        req.ctx.extensions_mut().insert(size);
        next.run(req).await
    }
}
