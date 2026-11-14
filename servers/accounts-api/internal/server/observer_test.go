// SPDX-License-Identifier: FSL-1.1-ALv2

package server

import (
	"bytes"
	"io"
	"log/slog"
	"math/rand"
	"net"
	"net/http"
	"reflect"
	"runtime"
	"strings"
	"testing"
	"time"
)

type pendingSummary struct {
	method       string
	path         string
	expectFailed bool
}

func newTestObserver() (*connObserver, *bytes.Buffer) {
	var logs bytes.Buffer
	return newConnObserver(slog.New(slog.NewJSONHandler(&logs, nil))), &logs
}

func summarize(o *connObserver) []pendingSummary {
	o.mu.Lock()
	defer o.mu.Unlock()
	var out []pendingSummary
	for _, p := range o.pending {
		out = append(out, pendingSummary{p.method, p.path, p.expectFailed})
	}
	return out
}

func TestObserverParsesRequestSequences(t *testing.T) {
	get := func(path string) string { return "GET " + path + " HTTP/1.1\r\nHost: t\r\n\r\n" }
	for _, tc := range []struct {
		name   string
		stream string
		want   []pendingSummary
	}{
		{"two gets", get("/a") + get("/b"), []pendingSummary{{"GET", "/a", false}, {"GET", "/b", false}}},
		{"bare LF line ends", "GET /a HTTP/1.1\nHost: t\n\n" + get("/b"), []pendingSummary{{"GET", "/a", false}, {"GET", "/b", false}}},
		{"options star", "OPTIONS * HTTP/1.1\r\nHost: t\r\n\r\n", []pendingSummary{{"OPTIONS", "*", false}}},
		{"absolute form", "GET http://t/abs?q=1 HTTP/1.1\r\nHost: t\r\n\r\n", []pendingSummary{{"GET", "/abs", false}}},
		{
			"content-length body is skipped",
			"POST /a HTTP/1.1\r\nHost: t\r\nContent-Length: 11\r\n\r\nGET /x HTTP" + get("/b"),
			[]pendingSummary{{"POST", "/a", false}, {"GET", "/b", false}},
		},
		{
			"chunked body with extension and trailer is skipped",
			"POST /a HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n3;ext=1\r\nabc\r\n0A\r\n0123456789\r\n0\r\nX-Trailer: v\r\n\r\n" + get("/b"),
			[]pendingSummary{{"POST", "/a", false}, {"GET", "/b", false}},
		},
		{
			"chunked body without trailer is skipped",
			"POST /a HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n1\r\nz\r\n0\r\n\r\n" + get("/b"),
			[]pendingSummary{{"POST", "/a", false}, {"GET", "/b", false}},
		},
		{
			"unsupported expectation stops parsing",
			get("/a") + "GET /b HTTP/1.1\r\nHost: t\r\nExpect: a-pony\r\n\r\n" + get("/c"),
			[]pendingSummary{{"GET", "/a", false}, {"GET", "/b", true}},
		},
		{
			"100-continue is supported",
			"POST /a HTTP/1.1\r\nHost: t\r\nExpect: 100-continue\r\nContent-Length: 2\r\n\r\nhi" + get("/b"),
			[]pendingSummary{{"POST", "/a", false}, {"GET", "/b", false}},
		},
		{
			"connection close ends the stream",
			"GET /a HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n" + get("/b"),
			[]pendingSummary{{"GET", "/a", false}},
		},
		{"malformed request line stops parsing", get("/a") + "NOT A REQUEST\r\n\r\n" + get("/b"), []pendingSummary{{"GET", "/a", false}}},
		{"leading empty line stops parsing", "\r\n" + get("/a"), nil},
		{
			"invalid chunk size stops parsing",
			"POST /a HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\nabc\r\n0\r\n\r\n" + get("/b"),
			[]pendingSummary{{"POST", "/a", false}},
		},
		{
			"missing CRLF after chunk data stops parsing",
			"POST /a HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n1\r\nzXX0\r\n\r\n" + get("/b"),
			[]pendingSummary{{"POST", "/a", false}},
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			whole, _ := newTestObserver()
			whole.feed([]byte(tc.stream))
			if got := summarize(whole); !reflect.DeepEqual(got, tc.want) {
				t.Errorf("whole stream: pending = %v, want %v", got, tc.want)
			}

			// The result must not depend on how the stream is split into reads.
			bytewise, _ := newTestObserver()
			for i := 0; i < len(tc.stream); i++ {
				bytewise.feed([]byte{tc.stream[i]})
			}
			if got := summarize(bytewise); !reflect.DeepEqual(got, tc.want) {
				t.Errorf("byte by byte: pending = %v, want %v", got, tc.want)
			}

			rng := rand.New(rand.NewSource(1))
			for range 50 {
				split, _ := newTestObserver()
				rest := []byte(tc.stream)
				for len(rest) > 0 {
					n := 1 + rng.Intn(len(rest))
					split.feed(rest[:n])
					rest = rest[n:]
				}
				if got := summarize(split); !reflect.DeepEqual(got, tc.want) {
					t.Fatalf("random split: pending = %v, want %v", got, tc.want)
				}
			}
		})
	}
}

func TestObserverBoundsHeadSize(t *testing.T) {
	o, _ := newTestObserver()
	o.feed([]byte("GET /a HTTP/1.1\r\nHost: t\r\n"))
	// A header section larger than net/http accepts, fed in small reads.
	chunk := []byte("X-Fill: " + strings.Repeat("a", 1000) + "\r\n")
	for i := 0; i < (maxObservedHeadBytes/len(chunk))+5; i++ {
		o.feed(chunk)
		o.mu.Lock()
		size := cap(o.head)
		o.mu.Unlock()
		// append may over-allocate, but never past twice the limit.
		if size > 2*maxObservedHeadBytes {
			t.Fatalf("head buffer capacity %d exceeds twice the limit %d", size, maxObservedHeadBytes)
		}
	}
	o.mu.Lock()
	defer o.mu.Unlock()
	if o.phase != phaseStopped || o.head != nil {
		t.Errorf("phase = %v, head retained = %v, want a stopped observer holding no head", o.phase, o.head != nil)
	}
}

func TestObserverAcceptsHeadAtTheNetHTTPLimit(t *testing.T) {
	o, _ := newTestObserver()
	prefix := "GET /big HTTP/1.1\r\nHost: t\r\nX-Fill: "
	suffix := "\r\n\r\n"
	fill := strings.Repeat("a", maxObservedHeadBytes-len(prefix)-len(suffix))
	o.feed([]byte(prefix + fill + suffix))
	if got := summarize(o); len(got) != 1 || got[0].path != "/big" {
		t.Errorf("pending = %v, want the head of exactly the limit to be observed", got)
	}
	o.mu.Lock()
	defer o.mu.Unlock()
	if cap(o.head) > retainedHeadCap {
		t.Errorf("head capacity %d retained after the request, want at most %d", cap(o.head), retainedHeadCap)
	}
}

func TestObserverBoundsPendingRequests(t *testing.T) {
	o, _ := newTestObserver()
	request := []byte("GET / HTTP/1.1\r\nHost: t\r\n\r\n")
	for range maxPendingRequests + 10 {
		o.feed(request)
	}
	if got := len(summarize(o)); got != maxPendingRequests {
		t.Errorf("pending = %d, want the cap %d", got, maxPendingRequests)
	}
}

func TestObserverBoundsChunkedScratch(t *testing.T) {
	for _, tc := range []struct {
		name string
		body string
	}{
		{"chunk size line", strings.Repeat("1", maxChunkLineBytes+10)},
		{"trailer", "0\r\n" + strings.Repeat("X: y\r\n", maxTrailerBytes)},
	} {
		t.Run(tc.name, func(t *testing.T) {
			o, _ := newTestObserver()
			o.feed([]byte("POST /a HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n"))
			for i := 0; i < len(tc.body); i += 100 {
				o.feed([]byte(tc.body[i:min(i+100, len(tc.body))]))
			}
			o.mu.Lock()
			defer o.mu.Unlock()
			if o.phase != phaseStopped || o.line != nil {
				t.Errorf("phase = %v, scratch retained = %v, want a stopped observer", o.phase, o.line != nil)
			}
		})
	}
}

func TestHandlerDequeuesPendingRequests(t *testing.T) {
	o, _ := newTestObserver()
	o.feed([]byte("GET /a HTTP/1.1\r\nHost: t\r\n\r\nGET /b HTTP/1.1\r\nHost: t\r\n\r\n"))
	o.handlerStarted()
	if got := summarize(o); len(got) != 1 || got[0].path != "/b" {
		t.Errorf("pending after one handler = %v, want only /b", got)
	}
	o.handlerFinished()
	o.handlerStarted()
	o.handlerFinished()
	// An extra call, such as after the observer gave up, must not break.
	o.handlerStarted()
	o.handlerFinished()
	if got := summarize(o); len(got) != 0 {
		t.Errorf("pending = %v, want none", got)
	}
}

const expectationFailedResponse = "HTTP/1.1 417 Expectation Failed\r\nConnection: close\r\nContent-Length: 0\r\nDate: Thu, 05 Nov 2026 00:00:00 GMT\r\n\r\n"

func TestClaimRejection(t *testing.T) {
	const badRequest = "GET /bad HTTP/1.1\r\nHost: t\r\nExpect: a-pony\r\n\r\n"
	for _, tc := range []struct {
		name   string
		setup  func(o *connObserver)
		write  string
		wantOK bool
	}{
		{"417 for the oldest pending rejected request", func(*connObserver) {}, expectationFailedResponse, true},
		{"417 answering an HTTP/1.0 request", func(*connObserver) {}, strings.Replace(expectationFailedResponse, "HTTP/1.1", "HTTP/1.0", 1), true},
		{"417 of another protocol version", func(*connObserver) {}, strings.Replace(expectationFailedResponse, "HTTP/1.1", "HTTP/2.0", 1), false},
		{"short write", func(*connObserver) {}, "HTTP/1.", false},
		{"handler still running", func(o *connObserver) { o.inFlight = 1 }, expectationFailedResponse, false},
		{"other status", func(*connObserver) {}, "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n", false},
		{"body continuation", func(*connObserver) {}, "xxHTTP/1.1 417 Expectation Failed\r\nConnection: close\r\n\r\n", false},
		{"417 without Connection close", func(*connObserver) {}, "HTTP/1.1 417 Expectation Failed\r\nContent-Length: 0\r\n\r\n", false},
		{"Connection close only in the body", func(*connObserver) {}, "HTTP/1.1 417 Expectation Failed\r\nContent-Length: 20\r\n\r\n\r\nConnection: close\r\n", false},
		{"Connection close is the first header", func(*connObserver) {}, "HTTP/1.1 417 Expectation Failed\r\nConnection: close\r\n\r\n", true},
		{"already claimed", func(o *connObserver) { o.rejected = true }, expectationFailedResponse, false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			o, _ := newTestObserver()
			o.feed([]byte(badRequest))
			tc.setup(o)
			_, _, ok := o.claimRejection([]byte(tc.write))
			if ok != tc.wantOK {
				t.Errorf("claimRejection = %v, want %v", ok, tc.wantOK)
			}
		})
	}

	t.Run("a request without a rejectable Expect is never claimed", func(t *testing.T) {
		o, _ := newTestObserver()
		o.feed([]byte("GET /a HTTP/1.1\r\nHost: t\r\n\r\n"))
		if _, _, ok := o.claimRejection([]byte(expectationFailedResponse)); ok {
			t.Error("claimed a 417 written for a request without an unsupported Expect header")
		}
	})

	t.Run("claimed once", func(t *testing.T) {
		o, _ := newTestObserver()
		o.feed([]byte(badRequest))
		if _, _, ok := o.claimRejection([]byte(expectationFailedResponse)); !ok {
			t.Fatal("first claim failed")
		}
		if _, _, ok := o.claimRejection([]byte(expectationFailedResponse)); ok {
			t.Error("second claim succeeded, want exactly one record per rejection")
		}
	})
}

func TestUnsupportedExpect(t *testing.T) {
	for _, tc := range []struct {
		values []string
		want   bool
	}{
		{nil, false},
		{[]string{""}, false},
		{[]string{"100-continue"}, false},
		{[]string{"100-Continue"}, false},
		{[]string{"100-continue, a-pony"}, false},
		{[]string{"a-pony, 100-continue"}, false},
		{[]string{"a-pony,100-continue"}, false},
		{[]string{"a-pony\t100-continue"}, false},
		{[]string{"a-pony"}, true},
		{[]string{"x100-continue"}, true},
		{[]string{"100-continued"}, true},
		{[]string{"100-continue-a-pony"}, true},
		{[]string{"a-pony", "100-continue"}, true},
		{[]string{"100-continue", "a-pony"}, false},
	} {
		h := http.Header{}
		for _, v := range tc.values {
			h.Add("Expect", v)
		}
		if got := unsupportedExpect(h); got != tc.want {
			t.Errorf("unsupportedExpect(%q) = %v, want %v", tc.values, got, tc.want)
		}
	}
}

// TestObservedConnForwardsTrafficUnchanged checks that the wrapper passes bytes
// through unmodified in both directions and keeps CloseWrite, which net/http
// uses to half-close, and that observing a connection starts no goroutine.
func TestObservedConnForwardsTrafficUnchanged(t *testing.T) {
	before := runtime.NumGoroutine()
	var logs bytes.Buffer
	logger := slog.New(slog.NewJSONHandler(&logs, nil))
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	defer listener.Close()
	observed := ObserveListener(listener, logger)

	accepted := make(chan net.Conn, 1)
	go func() {
		conn, err := observed.Accept()
		if err != nil {
			close(accepted)
			return
		}
		accepted <- conn
	}()
	client, err := net.Dial("tcp", listener.Addr().String())
	if err != nil {
		t.Fatalf("dial: %v", err)
	}
	defer client.Close()
	conn := <-accepted
	if conn == nil {
		t.Fatal("accept failed")
	}
	if _, ok := conn.(interface{ CloseWrite() error }); !ok {
		t.Error("observed connection hides CloseWrite")
	}

	request := "GET /a HTTP/1.1\r\nHost: t\r\nExpect: a-pony\r\n\r\n"
	if _, err := io.WriteString(client, request); err != nil {
		t.Fatalf("write request: %v", err)
	}
	received := make([]byte, len(request))
	if _, err := io.ReadFull(conn, received); err != nil || string(received) != request {
		t.Fatalf("server read %q, %v; want the request unchanged", received, err)
	}
	if _, err := io.WriteString(conn, expectationFailedResponse); err != nil {
		t.Fatalf("write response: %v", err)
	}
	response := make([]byte, len(expectationFailedResponse))
	if _, err := io.ReadFull(client, response); err != nil || string(response) != expectationFailedResponse {
		t.Fatalf("client read %q, %v; want the response unchanged", response, err)
	}
	if err := conn.(interface{ CloseWrite() error }).CloseWrite(); err != nil {
		t.Errorf("CloseWrite: %v", err)
	}
	if n, err := client.Read(make([]byte, 1)); n != 0 || err != io.EOF {
		t.Errorf("client read after CloseWrite = %d, %v; want EOF", n, err)
	}
	if got := strings.Count(logs.String(), `"msg":"http_request"`); got != 1 {
		t.Errorf("http_request records = %d, want exactly 1: %s", got, logs.String())
	}
	conn.Close()
	client.Close()

	// The observer is plain per-connection state: no goroutine outlives the
	// connection. Allow the accept goroutine and the runtime a moment to settle.
	deadline := time.Now().Add(2 * time.Second)
	for runtime.NumGoroutine() > before && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	if after := runtime.NumGoroutine(); after > before {
		t.Errorf("goroutines = %d after closing the connection, want at most %d", after, before)
	}
}

func TestConnContextPairsOnlyObservedConnections(t *testing.T) {
	var logs bytes.Buffer
	logger := slog.New(slog.NewJSONHandler(&logs, nil))
	plain, other := net.Pipe()
	defer plain.Close()
	defer other.Close()
	if obs := observerFrom(ConnContext(t.Context(), plain)); obs != nil {
		t.Error("an unobserved connection was given an observer")
	}
	observed := &observedConn{Conn: plain, obs: newConnObserver(logger)}
	if obs := observerFrom(ConnContext(t.Context(), observed)); obs != observed.obs {
		t.Error("an observed connection was not paired with its observer")
	}
}
