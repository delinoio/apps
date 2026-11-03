// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"context"
	"errors"
	"io"
	"net/http"
	"net/http/httptrace"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/gin-gonic/gin"
)

// Byte metering
//
// Billable bytes are counted in HTTP/1.1 wire form before TLS:
//
//   - provider segment: "<METHOD> <request-uri> HTTP/1.1\r\n", one
//     "Name: value\r\n" line per header value the HTTP client wrote, the blank
//     line, and the body bytes the client read from the request body;
//   - customer segment: "HTTP/1.1 <code> <reason>\r\n", one "Name: value\r\n"
//     line per value in the handler's header map when the header is sent, the
//     blank line, and every body byte the handler wrote.
//
// HTTP/2 upstream requests are normalized to the same form: pseudo-headers
// become the request line and the Host line, and the lowercase field names
// have the same length as their HTTP/1.1 spelling. Chunked transfer framing,
// TLS and TCP overhead, and headers that net/http's server adds after the
// handler (Date, Content-Length, Transfer-Encoding) are not counted.

const http11Suffix = " HTTP/1.1\r\n"

// callState is the per-call metering and outcome evidence. It is created by
// the call middleware and reached from SDK code through the request context
// (see callFromContext).
type callState struct {
	id         string
	resourceID string
	principal  CallPrincipal
	format     mcpav1.EndpointFormat
	model      string
	streaming  bool
	start      time.Time

	requestBytes  atomic.Int64
	responseBytes atomic.Int64

	mu              sync.Mutex
	executorInvoked bool
	credentialID    string
	provider        mcpav1.Provider
	httpStatus      int
	executorStatus  int
	executorFailed  bool
	streamFailed    bool
	dataBeforeError bool
	rejection       *Rejection

	checkpoint checkpointState
}

func (c *callState) addRequestBytes(n int64) {
	if c != nil && n > 0 {
		c.requestBytes.Add(n)
	}
}

func (c *callState) addResponseBytes(n int64) {
	if c != nil && n > 0 {
		c.responseBytes.Add(n)
	}
}

// selected records the credential the SDK selected for this call.
func (c *callState) selected(credentialID string, provider mcpav1.Provider) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.executorInvoked = true
	c.credentialID = credentialID
	c.provider = provider
}

// observeHTTPStatus records the status of a provider response seen by the
// metering transport. The last response of the call wins.
func (c *callState) observeHTTPStatus(status int) {
	c.mu.Lock()
	c.httpStatus = status
	c.mu.Unlock()
}

// observeExecutorResult records the error returned by an SDK executor.
func (c *callState) observeExecutorResult(err error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if err == nil {
		if c.executorStatus == 0 {
			c.executorStatus = http.StatusOK
		}
		return
	}
	c.executorFailed = true
	var statusErr interface{ StatusCode() int }
	if errors.As(err, &statusErr) && statusErr.StatusCode() > 0 {
		c.executorStatus = statusErr.StatusCode()
	}
}

// observeStreamChunk records stream progress and terminal stream errors.
func (c *callState) observeStreamChunk(payload []byte, err error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if err != nil {
		if !c.streamFailed {
			c.streamFailed = true
			var statusErr interface{ StatusCode() int }
			if errors.As(err, &statusErr) && statusErr.StatusCode() > 0 {
				c.executorStatus = statusErr.StatusCode()
			}
		}
		return
	}
	if len(payload) > 0 && !c.streamFailed {
		c.dataBeforeError = true
	}
}

func (c *callState) reject(rejection *Rejection) {
	c.mu.Lock()
	c.rejection = rejection
	c.mu.Unlock()
}

// invalidCallKeyRejected reports whether the call was rejected because its
// call key is no longer valid. Such a call is not attributable to a valid key,
// so it is neither recorded nor billed.
func (c *callState) invalidCallKeyRejected() bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.rejection != nil && c.rejection.Code == RejectionInvalidCallKey
}

// outcomeInput snapshots the evidence for classification.
func (c *callState) outcomeInput(clientCancelled bool, customerStatus int) outcomeInput {
	c.mu.Lock()
	defer c.mu.Unlock()
	status := c.executorStatus
	if status == 0 {
		status = c.httpStatus
	}
	// An executor error without an HTTP error status means the provider
	// exchange failed at the transport level (no response, or a body that
	// could not be read).
	networkError := c.executorFailed && c.executorStatus == 0 && c.httpStatus < 400
	return outcomeInput{
		rejection:       c.rejection,
		executorInvoked: c.executorInvoked,
		providerStatus:  status,
		networkError:    networkError,
		streamFailed:    c.streamFailed,
		sentStreamData:  c.dataBeforeError,
		clientCancelled: clientCancelled,
		customerStatus:  customerStatus,
	}
}

type callContextKey struct{}

// traceMeteredContextKey marks contexts whose outbound requests are already
// metered by an httptrace hook, so the metering transport does not count them
// a second time.
type traceMeteredContextKey struct{}

func withCall(ctx context.Context, call *callState) context.Context {
	return context.WithValue(ctx, callContextKey{}, call)
}

// callFromContext finds the call of an SDK execution context.
//
// The SDK handlers do not derive the execution context from the inbound
// request context: GetContextWithCancel (sdk/api/handlers/handlers.go:364)
// is called with context.Background() by every handler, copies only the
// request ID, links cancellation with a goroutine, and stores the
// *gin.Context under the string key "gin" (handlers.go:414). The auth
// manager passes that context to executors (conductor.go:1316, 1395, 1474),
// which build every upstream request with http.NewRequestWithContext(ctx, ...).
// mcpa therefore resolves the call through ctx.Value("gin") and the gin
// request's own context, which carries the value set by the call middleware.
// Each inbound request has its own *gin.Context, so the lookup cannot
// attribute bytes to another call.
func callFromContext(ctx context.Context) *callState {
	if ctx == nil {
		return nil
	}
	if call, ok := ctx.Value(callContextKey{}).(*callState); ok && call != nil {
		return call
	}
	if ginCtx, ok := ctx.Value("gin").(*gin.Context); ok && ginCtx != nil && ginCtx.Request != nil {
		if call, ok := ginCtx.Request.Context().Value(callContextKey{}).(*callState); ok {
			return call
		}
	}
	return nil
}

// headerFieldBytes returns the HTTP/1.1 size of header lines for one field.
func headerFieldBytes(name string, values []string) int64 {
	var n int64
	for _, value := range values {
		n += int64(len(name) + len(": ") + len(value) + len("\r\n"))
	}
	return n
}

// requestLineBytes returns the size of "<METHOD> <uri> HTTP/1.1\r\n".
func requestLineBytes(method, requestURI string) int64 {
	if method == "" {
		method = http.MethodGet
	}
	return int64(len(method) + 1 + len(requestURI) + len(http11Suffix))
}

// statusLineBytes returns the size of the HTTP/1.1 status line net/http
// writes for code.
func statusLineBytes(code int) int64 {
	text := http.StatusText(code)
	if text == "" {
		text = "status code " + strconv.Itoa(code)
	}
	return int64(len("HTTP/1.1 ") + 3 + 1 + len(text) + len("\r\n"))
}

// meteringTransport counts provider-bound bytes of calls. It is installed as
// the SDK's per-auth RoundTripper (coreauth.Manager.SetRoundTripperProvider);
// the manager stores it in the execution context under
// "cliproxy.roundtripper" (conductor.go:1311-1314) and every executor that
// builds its client with helps.NewProxyAwareHTTPClient uses it as the client
// transport (internal/runtime/executor/helps/proxy_helpers.go:56).
type meteringTransport struct {
	base http.RoundTripper
}

func (t *meteringTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	call := callFromContext(req.Context())
	if call == nil || req.Context().Value(traceMeteredContextKey{}) != nil {
		return t.base.RoundTrip(req)
	}
	call.addRequestBytes(requestLineBytes(req.Method, req.URL.RequestURI()))
	trace := &httptrace.ClientTrace{
		WroteHeaderField: func(name string, values []string) {
			if strings.HasPrefix(name, ":") {
				// HTTP/2: the request line is already counted; :authority is the
				// Host header line.
				if name == ":authority" {
					call.addRequestBytes(headerFieldBytes("Host", values))
				}
				return
			}
			call.addRequestBytes(headerFieldBytes(name, values))
		},
		WroteHeaders: func() { call.addRequestBytes(int64(len("\r\n"))) },
	}
	out := req.Clone(httptrace.WithClientTrace(req.Context(), trace))
	if req.Body != nil && req.Body != http.NoBody {
		out.Body = &countingReadCloser{ReadCloser: req.Body, call: call}
	}
	if req.GetBody != nil {
		getBody := req.GetBody
		out.GetBody = func() (io.ReadCloser, error) {
			body, err := getBody()
			if err != nil || body == nil || body == http.NoBody {
				return body, err
			}
			return &countingReadCloser{ReadCloser: body, call: call}, nil
		}
	}
	resp, err := t.base.RoundTrip(out)
	if err == nil && resp != nil {
		call.observeHTTPStatus(resp.StatusCode)
	}
	return resp, err
}

type countingReadCloser struct {
	io.ReadCloser
	call *callState
}

func (r *countingReadCloser) Read(p []byte) (int, error) {
	n, err := r.ReadCloser.Read(p)
	r.call.addRequestBytes(int64(n))
	return n, err
}

// withTraceMetering meters outbound requests of executors that do not use
// the context RoundTripper.
//
// Workaround: the SDK Claude executor builds its client with
// helps.NewUtlsHTTPClient (internal/runtime/executor/claude_executor.go:228,
// 402, 637), which creates its own uTLS/HTTP2 and net/http transports and
// ignores the context RoundTripper. The Kimi executor delegates Claude-format
// requests to the same code. Both still build requests with the execution
// context, and net/http and x/net/http2 report every written header field
// through httptrace (net/http/header.go:219, request.go:694 and 711,
// transfer.go:307, internal/httpcommon/httpcommon.go:408). The hook counts
// those fields and takes the body size from Content-Length, which the Claude
// executor always sets because it sends bytes.Reader bodies. For HTTP/1.1
// requests the transport does not trace the request line, so fallbackURI
// supplies the request URI the executor builds. Remove this hook when the SDK
// Claude executor honours the context RoundTripper.
//
// The executor's body cannot be wrapped, and httptrace reports no body
// progress. WroteHeaders runs before the body is sent, so the body is billed
// only when WroteRequest reports the whole request written without an error
// (net/http request.go Request.write; x/net http2 transport.go
// traceWroteRequest, which also reports a body that the transport abandoned
// after an early response). A cancelled, rejected or disconnected upload
// therefore bills its request line and headers but none of its body, because
// the number of body bytes that reached the provider is unknown.
func withTraceMetering(ctx context.Context, call *callState, fallbackURI string) context.Context {
	var (
		mu            sync.Mutex
		method        string
		path          string
		contentLength int64
		// pendingBody is the declared body size of the request whose
		// headers were written last, billed when its write completes.
		pendingBody int64
	)
	trace := &httptrace.ClientTrace{
		WroteHeaderField: func(name string, values []string) {
			mu.Lock()
			defer mu.Unlock()
			switch {
			case name == ":method" && len(values) > 0:
				method = values[0]
			case name == ":path" && len(values) > 0:
				path = values[0]
			case name == ":authority":
				call.addRequestBytes(headerFieldBytes("Host", values))
			case strings.HasPrefix(name, ":"):
			default:
				if strings.EqualFold(name, "Content-Length") && len(values) > 0 {
					if n, err := strconv.ParseInt(values[0], 10, 64); err == nil && n > 0 {
						contentLength = n
					}
				}
				call.addRequestBytes(headerFieldBytes(name, values))
			}
		},
		WroteHeaders: func() {
			mu.Lock()
			defer mu.Unlock()
			if method != "" && path != "" {
				call.addRequestBytes(requestLineBytes(method, path))
			} else {
				call.addRequestBytes(requestLineBytes(http.MethodPost, fallbackURI))
			}
			call.addRequestBytes(int64(len("\r\n")))
			pendingBody = contentLength
			method, path, contentLength = "", "", 0
		},
		WroteRequest: func(info httptrace.WroteRequestInfo) {
			mu.Lock()
			defer mu.Unlock()
			if info.Err == nil {
				call.addRequestBytes(pendingBody)
			}
			pendingBody = 0
		},
	}
	ctx = context.WithValue(ctx, traceMeteredContextKey{}, true)
	return httptrace.WithClientTrace(ctx, trace)
}

// requestURIFor joins a base URL path with an executor path such as
// "/v1/messages?beta=true".
func requestURIFor(baseURL, suffix string) string {
	parsed, err := url.Parse(strings.TrimRight(baseURL, "/"))
	if err != nil || parsed.Path == "" {
		return suffix
	}
	return strings.TrimRight(parsed.EscapedPath(), "/") + suffix
}

// countingResponseWriter counts customer-bound bytes of a call.
type countingResponseWriter struct {
	http.ResponseWriter
	call        *callState
	wroteHeader bool
	status      int
}

func newCountingResponseWriter(w http.ResponseWriter, call *callState) *countingResponseWriter {
	return &countingResponseWriter{ResponseWriter: w, call: call}
}

func (w *countingResponseWriter) WriteHeader(code int) {
	if w.wroteHeader {
		return
	}
	if code >= 100 && code < 200 && code != http.StatusSwitchingProtocols {
		w.ResponseWriter.WriteHeader(code)
		return
	}
	w.wroteHeader = true
	w.status = code
	size := statusLineBytes(code)
	for name, values := range w.ResponseWriter.Header() {
		size += headerFieldBytes(name, values)
	}
	w.call.addResponseBytes(size + int64(len("\r\n")))
	w.ResponseWriter.WriteHeader(code)
}

func (w *countingResponseWriter) Write(p []byte) (int, error) {
	if !w.wroteHeader {
		w.WriteHeader(http.StatusOK)
	}
	n, err := w.ResponseWriter.Write(p)
	w.call.addResponseBytes(int64(n))
	return n, err
}

func (w *countingResponseWriter) Flush() {
	if !w.wroteHeader {
		w.WriteHeader(http.StatusOK)
	}
	_ = http.NewResponseController(w.ResponseWriter).Flush()
}

// Unwrap lets http.ResponseController reach the server's writer.
func (w *countingResponseWriter) Unwrap() http.ResponseWriter { return w.ResponseWriter }

func (w *countingResponseWriter) statusCode() int {
	if !w.wroteHeader {
		return 0
	}
	return w.status
}
