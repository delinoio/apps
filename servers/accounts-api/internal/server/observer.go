// SPDX-License-Identifier: FSL-1.1-ALv2

package server

import (
	"bufio"
	"bytes"
	"context"
	"log/slog"
	"net"
	"net/http"
	"strconv"
	"sync"
	"time"
)

// MaxHeaderBytes is the request header limit of the production http.Server.
// The request head observer derives its own limit from it, so the observer
// never accepts a head that net/http would reject and never gives up on a head
// that net/http accepts.
const MaxHeaderBytes = http.DefaultMaxHeaderBytes

const (
	// maxObservedHeadBytes matches the limit net/http applies to one request
	// head: MaxHeaderBytes plus 4096 bytes for the request line and slack.
	maxObservedHeadBytes = MaxHeaderBytes + 4096
	// maxPendingRequests bounds the parsed requests that net/http has read from
	// the connection but not yet dispatched to the handler. net/http reads
	// through a 4 KiB buffer, so a connection cannot lead the handler by more
	// than a few hundred minimal requests; the cap only guards that assumption.
	maxPendingRequests = 1024
	// maxChunkLineBytes and maxTrailerBytes bound the scratch space used to
	// skip a chunked request body. They match the limits net/http applies to a
	// chunk-size line and to the trailer section.
	maxChunkLineBytes = 4096
	maxTrailerBytes   = 4096
	// retainedHeadCap is the largest head buffer kept between requests, so an
	// idle keep-alive connection does not hold a large buffer.
	retainedHeadCap = 4096
)

// Request head observer
//
// net/http answers a request whose Expect header names an unsupported
// expectation with "417 Expectation Failed" and closes the connection before it
// calls Server.Handler, and it offers no hook for that rejection. The request
// logger inside the handler therefore never sees such a request.
//
// The observer closes that gap without changing how net/http answers. The
// listener returned by ObserveListener wraps every accepted connection:
//
//   - Inbound bytes are fed, synchronously and before net/http sees them, to a
//     bounded incremental parser that splits the stream into request heads,
//     parses each head with http.ReadRequest (the parser net/http uses) and
//     skips the request body by its declared framing. The result is an ordered
//     queue of pending requests (method, path, whether the Expect header is
//     unsupported). The parser keeps at most one request head in memory.
//   - The request logger reports each handler call through the connection
//     state that ConnContext stores in the request context. A handler call
//     dequeues the oldest pending request. Every request that net/http does not
//     answer itself reaches the handler, and a rejection closes the connection,
//     so the oldest pending request is the one net/http handles next.
//   - When net/http writes the 417 response while no handler is running and the
//     oldest pending request has an unsupported Expect header, that request is
//     the rejected one and the observer logs its http_request record exactly
//     once, before the response is written, as the request logger does.
//
// The observer never answers, delays or alters traffic and starts no goroutine,
// so a connection that net/http closes releases everything with it. Whenever the
// stream does not parse as net/http would parse it (a malformed or oversized
// head, an invalid chunked body, a full queue) the observer stops parsing and
// the connection continues unobserved: it may then miss the record of a later
// rejected request, but it never logs a request that was not rejected.

// observerPhase is the position of the incremental parser in the byte stream.
type observerPhase int

const (
	// phaseHead accumulates a request head up to its terminating empty line.
	phaseHead observerPhase = iota
	// phaseBody skips the remaining bytes of a Content-Length body.
	phaseBody
	// phaseChunkSize accumulates a chunk-size line of a chunked body.
	phaseChunkSize
	// phaseChunkData skips the remaining data bytes of a chunk.
	phaseChunkData
	// phaseChunkDataEnd expects the CRLF that follows chunk data.
	phaseChunkDataEnd
	// phaseTrailer skips the trailer section after the last chunk.
	phaseTrailer
	// phaseStopped means the parser gave up or the connection ends after the
	// queued requests. Pending requests stay valid for pairing.
	phaseStopped
)

// observedRequest is the data of a parsed request head that the 417 record
// needs. It never holds header values or bodies.
type observedRequest struct {
	method string
	path   string
	// expectFailed reports that net/http rejects the request with 417.
	expectFailed bool
	// parsedAt is when the head was complete.
	parsedAt time.Time
}

// connObserver is the per-connection parser and pairing state. All methods are
// safe for concurrent use: net/http may read the connection from its background
// reader goroutine, and Server.Shutdown closes connections from another one.
type connObserver struct {
	logger *slog.Logger
	now    func() time.Time

	mu sync.Mutex

	phase     observerPhase
	head      []byte
	lineStart int
	lines     int

	// remaining counts the body or chunk data bytes still to skip.
	remaining uint64
	// line accumulates a chunk-size line or the current trailer line.
	line         []byte
	trailerBytes int

	pending  []observedRequest
	inFlight int
	lastDone time.Time
	rejected bool
}

func newConnObserver(logger *slog.Logger) *connObserver {
	return &connObserver{logger: logger, now: time.Now}
}

// feed consumes inbound bytes in stream order.
func (o *connObserver) feed(p []byte) {
	o.mu.Lock()
	defer o.mu.Unlock()
	for len(p) > 0 && o.phase != phaseStopped {
		switch o.phase {
		case phaseHead:
			p = o.feedHead(p)
		case phaseBody, phaseChunkData:
			n := uint64(len(p))
			if n > o.remaining {
				n = o.remaining
			}
			p = p[n:]
			o.remaining -= n
			if o.remaining == 0 {
				if o.phase == phaseBody {
					o.phase = phaseHead
				} else {
					o.phase = phaseChunkDataEnd
					o.line = o.line[:0]
				}
			}
		case phaseChunkDataEnd:
			p = o.feedChunkDataEnd(p)
		case phaseChunkSize:
			p = o.feedChunkSize(p)
		case phaseTrailer:
			p = o.feedTrailer(p)
		}
	}
}

func (o *connObserver) stop() {
	o.phase = phaseStopped
	o.head = nil
	o.line = nil
}

// feedHead appends bytes to the current head until its empty line and returns
// the unconsumed rest.
func (o *connObserver) feedHead(p []byte) []byte {
	for i, b := range p {
		o.head = append(o.head, b)
		if len(o.head) > maxObservedHeadBytes {
			o.stop()
			return nil
		}
		if b != '\n' {
			continue
		}
		// textproto accepts a bare LF as a line end and ends the head at the
		// first empty line after the request line.
		line := bytes.TrimSuffix(o.head[o.lineStart:len(o.head)-1], []byte("\r"))
		if len(line) == 0 {
			if o.lines == 0 {
				o.stop()
				return nil
			}
			o.finishHead()
			return p[i+1:]
		}
		o.lineStart = len(o.head)
		o.lines++
	}
	return nil
}

// finishHead parses the completed head and chooses how to skip its body.
func (o *connObserver) finishHead() {
	head := o.head
	o.lineStart, o.lines = 0, 0
	req, ok := parseObservedHead(head)
	if cap(o.head) > retainedHeadCap {
		o.head = nil
	} else {
		o.head = o.head[:0]
	}
	if !ok || len(o.pending) >= maxPendingRequests {
		o.stop()
		return
	}
	observed := observedRequest{
		method:       req.Method,
		path:         req.URL.Path,
		expectFailed: unsupportedExpect(req.Header),
		parsedAt:     o.now(),
	}
	o.pending = append(o.pending, observed)

	switch {
	case req.Close || observed.expectFailed:
		// net/http closes the connection after this request, or after the 417.
		o.stop()
	case len(req.TransferEncoding) == 1 && req.TransferEncoding[0] == "chunked":
		o.phase = phaseChunkSize
		o.line = o.line[:0]
	case req.ContentLength > 0:
		o.phase = phaseBody
		o.remaining = uint64(req.ContentLength)
	case req.ContentLength == 0:
		o.phase = phaseHead
	default:
		o.stop()
	}
}

// parseObservedHead parses a complete request head with the parser net/http
// uses. It reports false unless the parser accepts the head and consumes
// exactly all of its bytes, which is how a disagreement about where the head
// ends shows up.
func parseObservedHead(head []byte) (*http.Request, bool) {
	rd := bytes.NewReader(head)
	br := bufio.NewReaderSize(rd, len(head))
	req, err := http.ReadRequest(br)
	if err != nil || rd.Len()+br.Buffered() != 0 {
		return nil, false
	}
	return req, true
}

// feedChunkSize accumulates and interprets a chunk-size line.
func (o *connObserver) feedChunkSize(p []byte) []byte {
	for i, b := range p {
		o.line = append(o.line, b)
		if len(o.line) > maxChunkLineBytes {
			o.stop()
			return nil
		}
		if b != '\n' {
			continue
		}
		size, ok := parseChunkSize(o.line)
		o.line = o.line[:0]
		if !ok {
			o.stop()
			return nil
		}
		if size == 0 {
			o.phase = phaseTrailer
			o.trailerBytes = 0
		} else {
			o.phase = phaseChunkData
			o.remaining = size
		}
		return p[i+1:]
	}
	return nil
}

// feedChunkDataEnd expects the CRLF after chunk data, byte by byte.
func (o *connObserver) feedChunkDataEnd(p []byte) []byte {
	want := "\r\n"
	for i, b := range p {
		if b != want[len(o.line)] {
			o.stop()
			return nil
		}
		o.line = append(o.line, b)
		if len(o.line) == len(want) {
			o.line = o.line[:0]
			o.phase = phaseChunkSize
			return p[i+1:]
		}
	}
	return nil
}

// feedTrailer skips the trailer section: lines up to the first empty line.
func (o *connObserver) feedTrailer(p []byte) []byte {
	for i, b := range p {
		o.trailerBytes++
		if o.trailerBytes > maxTrailerBytes {
			o.stop()
			return nil
		}
		if b != '\n' {
			o.line = append(o.line, b)
			continue
		}
		empty := len(bytes.TrimSuffix(o.line, []byte("\r"))) == 0
		o.line = o.line[:0]
		if empty {
			o.phase = phaseHead
			return p[i+1:]
		}
	}
	return nil
}

// parseChunkSize parses a chunk-size line the way net/http does: trailing
// whitespace and a chunk extension are dropped and the rest must be a hex
// number of at most 16 digits.
func parseChunkSize(line []byte) (uint64, bool) {
	line = bytes.TrimRight(line, " \t\r\n")
	if i := bytes.IndexByte(line, ';'); i >= 0 {
		line = line[:i]
	}
	if len(line) == 0 || len(line) > 16 {
		return 0, false
	}
	// With base 16 ParseUint accepts only hex digits: no sign, prefix or
	// underscore.
	size, err := strconv.ParseUint(string(line), 16, 64)
	return size, err == nil
}

// unsupportedExpect reports whether net/http rejects a request with this header
// set with 417: a non-empty Expect header that does not contain the
// 100-continue token. It mirrors the check in net/http's connection loop
// (request.expectsContinue); a test compares it with the real server.
func unsupportedExpect(h http.Header) bool {
	var value string
	if values := h["Expect"]; len(values) > 0 {
		value = values[0]
	}
	return value != "" && !hasToken(value, "100-continue")
}

// hasToken reports whether v contains token as a comma, space or tab delimited,
// case-insensitive element, like the unexported helper of net/http.
func hasToken(v, token string) bool {
	if len(token) > len(v) || token == "" {
		return false
	}
	isBoundary := func(b byte) bool { return b == ' ' || b == ',' || b == '\t' }
	for start := 0; start <= len(v)-len(token); start++ {
		if start > 0 && !isBoundary(v[start-1]) {
			continue
		}
		end := start + len(token)
		if end != len(v) && !isBoundary(v[end]) {
			continue
		}
		if equalFoldASCII(v[start:end], token) {
			return true
		}
	}
	return false
}

func equalFoldASCII(a, b string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := 0; i < len(a); i++ {
		x, y := a[i], b[i]
		if 'A' <= x && x <= 'Z' {
			x += 'a' - 'A'
		}
		if 'A' <= y && y <= 'Z' {
			y += 'a' - 'A'
		}
		if x != y {
			return false
		}
	}
	return true
}

// handlerStarted records that the request logger runs for the oldest pending
// request of the connection.
func (o *connObserver) handlerStarted() {
	o.mu.Lock()
	defer o.mu.Unlock()
	o.inFlight++
	if len(o.pending) > 0 {
		o.pending[0] = observedRequest{}
		o.pending = o.pending[1:]
		if len(o.pending) == 0 {
			o.pending = nil
		}
	}
}

// handlerFinished records that a handler call returned or panicked. The next
// pending request cannot start before that, which bounds its logged duration.
func (o *connObserver) handlerFinished() {
	o.mu.Lock()
	defer o.mu.Unlock()
	o.inFlight--
	o.lastDone = o.now()
}

// expectationFailedStatus is the status line of every 417 response of net/http
// after the protocol version, which net/http copies from the request (HTTP/1.0
// or HTTP/1.1). net/http adds "Connection: close" to the response, which a
// handler answering 417 itself does not do unless it closes the connection too.
const expectationFailedStatus = " 417 Expectation Failed\r\n"

// isExpectationFailedResponse reports whether p starts the response that
// net/http writes for an unsupported expectation.
func isExpectationFailedResponse(p []byte) bool {
	const versionLen = len("HTTP/1.1")
	if len(p) < versionLen || !bytes.HasPrefix(p, []byte("HTTP/1.")) || (p[versionLen-1] != '0' && p[versionLen-1] != '1') {
		return false
	}
	if !bytes.HasPrefix(p[versionLen:], []byte(expectationFailedStatus)) {
		return false
	}
	// Start at the CRLF that ends the status line so that the first header line
	// is matched like every other one.
	headers := p[versionLen+len(expectationFailedStatus)-2:]
	if end := bytes.Index(headers, []byte("\r\n\r\n")); end >= 0 {
		headers = headers[:end+2]
	}
	return bytes.Contains(headers, []byte("\r\nConnection: close\r\n"))
}

// claimRejection reports the request that this outbound write rejects, if any,
// and claims it so it is logged exactly once.
func (o *connObserver) claimRejection(p []byte) (observedRequest, time.Time, bool) {
	o.mu.Lock()
	defer o.mu.Unlock()
	if o.rejected || o.inFlight > 0 || len(o.pending) == 0 || !o.pending[0].expectFailed || !isExpectationFailedResponse(p) {
		return observedRequest{}, time.Time{}, false
	}
	o.rejected = true
	request := o.pending[0]
	o.pending = nil
	// The request waited behind earlier pipelined requests on the connection, so
	// it started when the last handler finished if that was later.
	start := request.parsedAt
	if o.lastDone.After(start) {
		start = o.lastDone
	}
	return request, start, true
}

// logRejection writes the http_request record of a request that net/http
// rejected before the handler ran. The response carries no body.
func (o *connObserver) logRejection(request observedRequest, start time.Time) {
	logRequest(context.Background(), o.logger, request.method, request.path, http.StatusExpectationFailed, o.now().Sub(start), 0)
}

// observedConn tees a connection into its connObserver. It keeps the
// net.Conn behavior net/http relies on, including CloseWrite.
type observedConn struct {
	net.Conn
	obs *connObserver
}

func (c *observedConn) Read(p []byte) (int, error) {
	n, err := c.Conn.Read(p)
	if n > 0 {
		c.obs.feed(p[:n])
	}
	return n, err
}

func (c *observedConn) Write(p []byte) (int, error) {
	// Log before writing, like the request logger, which records the request
	// before net/http flushes the response.
	if request, start, ok := c.obs.claimRejection(p); ok {
		c.obs.logRejection(request, start)
	}
	return c.Conn.Write(p)
}

// CloseWrite is used by net/http to half-close a connection before it closes
// it; embedding net.Conn alone would hide it.
func (c *observedConn) CloseWrite() error {
	if cw, ok := c.Conn.(interface{ CloseWrite() error }); ok {
		return cw.CloseWrite()
	}
	return nil
}

type observedListener struct {
	net.Listener
	logger *slog.Logger
}

// ObserveListener wraps listener so that requests that net/http rejects with
// 417 before the handler runs are logged as http_request records. The server
// that serves the listener must set ConnContext to this package's ConnContext
// and MaxHeaderBytes to MaxHeaderBytes. It does not change any response.
func ObserveListener(listener net.Listener, logger *slog.Logger) net.Listener {
	return observedListener{Listener: listener, logger: logger}
}

func (l observedListener) Accept() (net.Conn, error) {
	conn, err := l.Listener.Accept()
	if err != nil {
		return nil, err
	}
	return &observedConn{Conn: conn, obs: newConnObserver(l.logger)}, nil
}

type observerKey struct{}

// ConnContext is the http.Server.ConnContext hook that hands the connection's
// observer to the request logger. Connections that ObserveListener did not wrap
// are left alone.
func ConnContext(ctx context.Context, conn net.Conn) context.Context {
	if observed, ok := conn.(*observedConn); ok {
		return context.WithValue(ctx, observerKey{}, observed.obs)
	}
	return ctx
}

func observerFrom(ctx context.Context) *connObserver {
	obs, _ := ctx.Value(observerKey{}).(*connObserver)
	return obs
}
