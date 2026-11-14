// SPDX-License-Identifier: FSL-1.1-ALv2

package main

import (
	"bufio"
	"io"
	"net"
	"net/http"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/delinoio/mono/servers/accounts-api/internal/server"
)

// These tests drive the production construction (newServer and serve) with raw
// HTTP/1.1 bytes, because a Go client cannot send most of these requests.

const healthzBody = `{"status":"ok"}` + "\n"

func dialRaw(t *testing.T, addr string) net.Conn {
	t.Helper()
	conn, err := net.Dial("tcp", addr)
	if err != nil {
		t.Fatalf("dial: %v", err)
	}
	t.Cleanup(func() { _ = conn.Close() })
	return conn
}

// readUntilClosed returns everything the server sends until it closes the
// connection, failing if it stays open.
func readUntilClosed(t *testing.T, conn net.Conn) string {
	t.Helper()
	_, received := expectClosedWithin(t, conn, 5*time.Second)
	return received
}

// exchange sends request on a new connection and returns the full response
// stream up to the server closing the connection. The request must make the
// server close the connection.
func exchange(t *testing.T, addr, request string) string {
	t.Helper()
	conn := dialRaw(t, addr)
	if _, err := io.WriteString(conn, request); err != nil {
		t.Fatalf("write: %v", err)
	}
	return readUntilClosed(t, conn)
}

// maskDate replaces the value of every Date header so responses compare equal.
func maskDate(response string) string {
	var out []string
	for _, line := range strings.Split(response, "\r\n") {
		if strings.HasPrefix(line, "Date: ") {
			line = "Date: <masked>"
		}
		out = append(out, line)
	}
	return strings.Join(out, "\r\n")
}

// wantExpectationFailed is the byte-exact response of net/http at the pinned Go
// toolchain, which the observer must leave unchanged. Go writes the headers in
// this order.
const wantExpectationFailed = "HTTP/1.1 417 Expectation Failed\r\nConnection: close\r\nDate: <masked>\r\nContent-Length: 0\r\n\r\n"

func assertRequestRecord(t *testing.T, record map[string]any, method, path string, status int, bytes int) {
	t.Helper()
	if record["msg"] != "http_request" || record["level"] != "INFO" {
		t.Errorf("record = %v, want an INFO http_request", record)
	}
	if record["method"] != method || record["path"] != path || record["status"] != float64(status) || record["bytes"] != float64(bytes) {
		t.Errorf("record = %v, want %s %s status %d bytes %d", record, method, path, status, bytes)
	}
	if duration, ok := record["duration_ms"].(float64); !ok || duration < 0 {
		t.Errorf("duration_ms = %v, want a non-negative number", record["duration_ms"])
	}
}

func TestNewServerLogsRequestRejectedForUnsupportedExpectation(t *testing.T) {
	for _, tc := range []struct {
		name    string
		request string
		method  string
		path    string
	}{
		{"healthz", "GET /healthz HTTP/1.1\r\nHost: test\r\nExpect: a-pony\r\nContent-Length: 0\r\n\r\n", "GET", "/healthz"},
		{"unrouted path", "GET /missing HTTP/1.1\r\nHost: test\r\nExpect: a-pony\r\nContent-Length: 0\r\n\r\n", "GET", "/missing"},
		{"HEAD", "HEAD /healthz HTTP/1.1\r\nHost: test\r\nExpect: a-pony\r\n\r\n", "HEAD", "/healthz"},
		{"POST with a body", "POST /healthz HTTP/1.1\r\nHost: test\r\nExpect: a-pony\r\nContent-Length: 5\r\n\r\nhello", "POST", "/healthz"},
		{"HTTP/1.0", "GET /healthz HTTP/1.0\r\nExpect: a-pony\r\n\r\n", "GET", "/healthz"},
		{"query string is not logged", "GET /healthz?token=secret HTTP/1.1\r\nHost: test\r\nExpect: a-pony\r\n\r\n", "GET", "/healthz"},
		{"secrets in headers and body are not logged", "POST /healthz HTTP/1.1\r\nHost: test\r\nAuthorization: Bearer super-secret\r\nExpect: a-pony\r\nContent-Length: 11\r\n\r\nbody-secret", "POST", "/healthz"},
		{"bare LF line ends", "GET /healthz HTTP/1.1\nHost: test\nExpect: a-pony\n\n", "GET", "/healthz"},
		{"header names are case insensitive", "GET /healthz HTTP/1.1\r\nhost: test\r\nexPECT: a-pony\r\n\r\n", "GET", "/healthz"},
		{"unsupported token before a supported one", "GET /healthz HTTP/1.1\r\nHost: test\r\nExpect: a-pony\r\nExpect: 100-continue\r\n\r\n", "GET", "/healthz"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			addr, logs := startRequestLogServer(t)
			response := exchange(t, addr, tc.request)
			// net/http answers with the protocol of the request and omits the
			// Content-Length of a HEAD response.
			want := wantExpectationFailed
			if strings.Contains(tc.request, "HTTP/1.0") {
				want = strings.Replace(want, "HTTP/1.1", "HTTP/1.0", 1)
			}
			if tc.method == http.MethodHead {
				want = strings.Replace(want, "Content-Length: 0\r\n", "", 1)
			}
			if got := maskDate(response); got != want {
				t.Errorf("response = %q, want the unchanged net/http 417 %q", got, want)
			}

			records := requestRecords(t, logs)
			if len(records) != 1 {
				t.Fatalf("got %d http_request records, want exactly 1: %s", len(records), logs.String())
			}
			assertRequestRecord(t, records[0], tc.method, tc.path, http.StatusExpectationFailed, 0)
			for _, secret := range []string{"super-secret", "body-secret", "a-pony", "token=secret", "Authorization", "Expect"} {
				if strings.Contains(logs.String(), secret) {
					t.Errorf("logs contain %q: %s", secret, logs.String())
				}
			}
		})
	}
}

// TestNewServerExpectationVariantsMatchNetHTTP compares the logged outcome with
// the answer of the real net/http server for each Expect value, which guards
// the observer's copy of the net/http rejection rule.
func TestNewServerExpectationVariantsMatchNetHTTP(t *testing.T) {
	for _, tc := range []struct {
		name       string
		expect     string // header lines, each ending in CRLF
		wantStatus int
	}{
		{"no Expect", "", http.StatusOK},
		{"empty Expect", "Expect:\r\n", http.StatusOK},
		{"100-continue", "Expect: 100-continue\r\n", http.StatusOK},
		{"mixed case 100-continue", "Expect: 100-Continue\r\n", http.StatusOK},
		{"100-continue then a-pony", "Expect: 100-continue, a-pony\r\n", http.StatusOK},
		{"a-pony then 100-continue", "Expect: a-pony, 100-continue\r\n", http.StatusOK},
		{"tab separated", "Expect: a-pony\t100-continue\r\n", http.StatusOK},
		{"a-pony", "Expect: a-pony\r\n", http.StatusExpectationFailed},
		{"prefixed token", "Expect: x100-continue\r\n", http.StatusExpectationFailed},
		{"suffixed token", "Expect: 100-continued\r\n", http.StatusExpectationFailed},
		{"first of two headers is supported", "Expect: 100-continue\r\nExpect: a-pony\r\n", http.StatusOK},
		{"first of two headers is unsupported", "Expect: a-pony\r\nExpect: 100-continue\r\n", http.StatusExpectationFailed},
	} {
		t.Run(tc.name, func(t *testing.T) {
			addr, logs := startRequestLogServer(t)
			response := exchange(t, addr, "GET /healthz HTTP/1.1\r\nHost: test\r\nConnection: close\r\n"+tc.expect+"\r\n")
			status := statusOf(t, response)
			if status != tc.wantStatus {
				t.Fatalf("net/http answered %d, want %d; response %q", status, tc.wantStatus, response)
			}
			wantBytes := len(healthzBody)
			if tc.wantStatus == http.StatusExpectationFailed {
				wantBytes = 0
			}
			records := requestRecords(t, logs)
			if len(records) != 1 {
				t.Fatalf("got %d http_request records, want exactly 1: %s", len(records), logs.String())
			}
			assertRequestRecord(t, records[0], "GET", "/healthz", tc.wantStatus, wantBytes)
		})
	}
}

func statusOf(t *testing.T, response string) int {
	t.Helper()
	resp, err := http.ReadResponse(bufio.NewReader(strings.NewReader(response)), nil)
	if err != nil {
		t.Fatalf("parse response %q: %v", response, err)
	}
	resp.Body.Close()
	return resp.StatusCode
}

// readResponse reads one response off a kept-alive connection.
func readResponse(t *testing.T, conn net.Conn, reader *bufio.Reader) (int, string) {
	t.Helper()
	if err := conn.SetReadDeadline(time.Now().Add(5 * time.Second)); err != nil {
		t.Fatalf("set read deadline: %v", err)
	}
	resp, err := http.ReadResponse(reader, nil)
	if err != nil {
		t.Fatalf("read response: %v", err)
	}
	body, err := io.ReadAll(resp.Body)
	resp.Body.Close()
	if err != nil {
		t.Fatalf("read body: %v", err)
	}
	return resp.StatusCode, string(body)
}

func TestNewServerKeepAliveSequenceEndingInRejectionLogsEveryRequestOnce(t *testing.T) {
	const get = "GET /healthz HTTP/1.1\r\nHost: test\r\n\r\n"
	const reject = "GET /missing HTTP/1.1\r\nHost: test\r\nExpect: a-pony\r\n\r\n"

	check := func(t *testing.T, logs *syncBuffer) {
		t.Helper()
		records := requestRecords(t, logs)
		if len(records) != 3 {
			t.Fatalf("got %d http_request records, want exactly 3: %s", len(records), logs.String())
		}
		assertRequestRecord(t, records[0], "GET", "/healthz", http.StatusOK, len(healthzBody))
		assertRequestRecord(t, records[1], "GET", "/healthz", http.StatusOK, len(healthzBody))
		assertRequestRecord(t, records[2], "GET", "/missing", http.StatusExpectationFailed, 0)
	}

	t.Run("one request at a time", func(t *testing.T) {
		addr, logs := startRequestLogServer(t)
		conn := dialRaw(t, addr)
		reader := bufio.NewReader(conn)
		for range 2 {
			if _, err := io.WriteString(conn, get); err != nil {
				t.Fatalf("write: %v", err)
			}
			if status, body := readResponse(t, conn, reader); status != http.StatusOK || body != healthzBody {
				t.Fatalf("healthz = %d %q", status, body)
			}
		}
		if _, err := io.WriteString(conn, reject); err != nil {
			t.Fatalf("write: %v", err)
		}
		rest, err := io.ReadAll(reader)
		if err != nil {
			t.Fatalf("read: %v", err)
		}
		if got := maskDate(string(rest)); got != wantExpectationFailed {
			t.Errorf("response = %q, want the 417 and then the closed connection", got)
		}
		check(t, logs)
	})

	t.Run("pipelined in one write", func(t *testing.T) {
		addr, logs := startRequestLogServer(t)
		response := exchange(t, addr, get+get+reject)
		if got := strings.Count(response, "HTTP/1.1 200 OK"); got != 2 {
			t.Errorf("got %d 200 responses, want 2: %q", got, response)
		}
		if !strings.HasSuffix(maskDate(response), wantExpectationFailed) {
			t.Errorf("response %q does not end with the 417", response)
		}
		check(t, logs)
	})

	t.Run("request split across writes", func(t *testing.T) {
		addr, logs := startRequestLogServer(t)
		conn := dialRaw(t, addr)
		stream := get + get + reject
		for i := 0; i < len(stream); i += 7 {
			if _, err := io.WriteString(conn, stream[i:min(i+7, len(stream))]); err != nil {
				t.Fatalf("write: %v", err)
			}
			time.Sleep(time.Millisecond)
		}
		response := readUntilClosed(t, conn)
		if !strings.HasSuffix(maskDate(response), wantExpectationFailed) {
			t.Errorf("response %q does not end with the 417", response)
		}
		check(t, logs)
	})
}

func postWithBody(body string) string {
	return "POST /healthz HTTP/1.1\r\nHost: test\r\nContent-Length: " + strconv.Itoa(len(body)) + "\r\n\r\n" + body
}

func TestNewServerRejectionAfterRequestBodiesLogsEveryRequestOnce(t *testing.T) {
	const reject = "GET /missing HTTP/1.1\r\nHost: test\r\nExpect: a-pony\r\n\r\n"
	for _, tc := range []struct {
		name  string
		first string
	}{
		{"content-length body", "POST /healthz HTTP/1.1\r\nHost: test\r\nContent-Length: 10\r\n\r\n0123456789"},
		{"body that looks like a request", postWithBody("GET /evil HTTP/1.1\r\nHost: test\r\nExpect: a-pony\r\n\r\n")},
		{"chunked body", "POST /healthz HTTP/1.1\r\nHost: test\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nabcd\r\n0\r\n\r\n"},
		{"chunked body with extension and trailer", "POST /healthz HTTP/1.1\r\nHost: test\r\nTransfer-Encoding: chunked\r\nTrailer: X-T\r\n\r\n4;a=b\r\nabcd\r\n0\r\nX-T: v\r\n\r\n"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			addr, logs := startRequestLogServer(t)
			response := exchange(t, addr, tc.first+reject)
			if !strings.HasPrefix(response, "HTTP/1.1 405 Method Not Allowed\r\n") || !strings.HasSuffix(maskDate(response), wantExpectationFailed) {
				t.Errorf("response = %q, want a 405 and then the 417", response)
			}
			records := requestRecords(t, logs)
			if len(records) != 2 {
				t.Fatalf("got %d http_request records, want exactly 2: %s", len(records), logs.String())
			}
			assertRequestRecord(t, records[0], "POST", "/healthz", http.StatusMethodNotAllowed, len("Method Not Allowed\n"))
			assertRequestRecord(t, records[1], "GET", "/missing", http.StatusExpectationFailed, 0)
		})
	}
}

// TestNewServerDoesNotLogRequestsThatNetHTTPNeverReaches checks that the
// observer logs a rejection only when net/http sends the 417: a request parsed
// from the pipeline that the server never reads must not be logged.
func TestNewServerDoesNotLogRequestsThatNetHTTPNeverReaches(t *testing.T) {
	const reject = "GET /missing HTTP/1.1\r\nHost: test\r\nExpect: a-pony\r\n\r\n"
	t.Run("after a request that closes the connection", func(t *testing.T) {
		addr, logs := startRequestLogServer(t)
		response := exchange(t, addr, "GET /healthz HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n"+reject)
		if strings.Contains(response, "417") {
			t.Fatalf("response = %q, want no 417 after Connection: close", response)
		}
		records := requestRecords(t, logs)
		if len(records) != 1 {
			t.Fatalf("got %d http_request records, want exactly 1: %s", len(records), logs.String())
		}
		assertRequestRecord(t, records[0], "GET", "/healthz", http.StatusOK, len(healthzBody))
	})

	t.Run("after a malformed request", func(t *testing.T) {
		addr, logs := startRequestLogServer(t)
		response := exchange(t, addr, "NOT A REQUEST\r\n\r\n"+reject)
		if strings.Contains(response, "417") || !strings.HasPrefix(response, "HTTP/1.1 400 Bad Request\r\n") {
			t.Fatalf("response = %q, want a 400 and no 417", response)
		}
		if records := requestRecords(t, logs); len(records) != 0 {
			t.Errorf("records = %v, want none for a request net/http rejected as malformed", records)
		}
	})

	t.Run("request net/http rejects as malformed before the Expect check", func(t *testing.T) {
		addr, logs := startRequestLogServer(t)
		// HTTP/1.1 without Host is a 400 in net/http, although the head parses.
		response := exchange(t, addr, "GET /healthz HTTP/1.1\r\nExpect: a-pony\r\n\r\n")
		if strings.Contains(response, "417") || !strings.HasPrefix(response, "HTTP/1.1 400 Bad Request") {
			t.Fatalf("response = %q, want a 400 and no 417", response)
		}
		if records := requestRecords(t, logs); len(records) != 0 {
			t.Errorf("records = %v, want none: net/http answered 400, not 417", records)
		}
	})

	t.Run("client leaves before the response", func(t *testing.T) {
		addr, logs := startRequestLogServer(t)
		conn := dialRaw(t, addr)
		// Half of a head never completes: no request, no record.
		if _, err := io.WriteString(conn, "GET /healthz HTTP/1.1\r\nHost: test\r\nExpect: a-pony\r\n"); err != nil {
			t.Fatalf("write: %v", err)
		}
		_ = conn.Close()
		time.Sleep(50 * time.Millisecond)
		if records := requestRecords(t, logs); len(records) != 0 {
			t.Errorf("records = %v, want none for an incomplete request", records)
		}
	})
}

func TestNewServerOversizedHeadIsRejectedWithoutARecord(t *testing.T) {
	addr, logs := startRequestLogServer(t)
	head := "GET /healthz HTTP/1.1\r\nHost: test\r\nExpect: a-pony\r\nX-Fill: " + strings.Repeat("a", server.MaxHeaderBytes+8192) + "\r\n\r\n"
	conn := dialRaw(t, addr)
	// The server may close while the client is still writing.
	go func() { _, _ = io.WriteString(conn, head) }()
	response := readUntilClosed(t, conn)
	if !strings.HasPrefix(response, "HTTP/1.1 431 ") {
		t.Fatalf("response = %.80q, want the net/http 431", response)
	}
	if records := requestRecords(t, logs); len(records) != 0 {
		t.Errorf("records = %v, want none for an oversized head", records)
	}
}

func TestNewServerSetsObserverContract(t *testing.T) {
	srv := newServer(http.NotFoundHandler(), newTestLogger())
	if srv.ConnContext == nil {
		t.Error("ConnContext is nil; the request logger cannot tell dispatched requests from rejected ones")
	}
	if srv.MaxHeaderBytes != server.MaxHeaderBytes {
		t.Errorf("MaxHeaderBytes = %d, want %d so the observer limit matches", srv.MaxHeaderBytes, server.MaxHeaderBytes)
	}
}

// TestNewServerSupportedExpectationWithBodyIsHandledNormally checks that a
// request with Expect: 100-continue and a body that the handler never reads is
// answered by the handler and logged once. net/http then closes the connection
// (it never sent 100 Continue, so it does not read the body), so a rejection
// pipelined behind it is never reached and must not be logged.
func TestNewServerSupportedExpectationWithBodyIsHandledNormally(t *testing.T) {
	addr, logs := startRequestLogServer(t)
	request := "POST /healthz HTTP/1.1\r\nHost: test\r\nExpect: 100-continue\r\nContent-Length: 4\r\n\r\nbody"
	reject := "GET /missing HTTP/1.1\r\nHost: test\r\nExpect: a-pony\r\n\r\n"
	response := exchange(t, addr, request+reject)
	if !strings.HasPrefix(response, "HTTP/1.1 405 Method Not Allowed\r\n") || strings.Contains(response, "417") {
		t.Errorf("response = %q, want only the 405", response)
	}
	records := requestRecords(t, logs)
	if len(records) != 1 {
		t.Fatalf("got %d http_request records, want exactly 1: %s", len(records), logs.String())
	}
	assertRequestRecord(t, records[0], "POST", "/healthz", http.StatusMethodNotAllowed, len("Method Not Allowed\n"))
}
