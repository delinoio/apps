// SPDX-License-Identifier: FSL-1.1-ALv2

package server

import (
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"log"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"testing"
)

func newTestHandler(t *testing.T) (http.Handler, *bytes.Buffer) {
	t.Helper()
	var logs bytes.Buffer
	logger := slog.New(slog.NewJSONHandler(&logs, nil))
	return NewHandler(logger), &logs
}

// decodeRecords parses every JSON log line written to logs.
func decodeRecords(t *testing.T, logs *bytes.Buffer) []map[string]any {
	t.Helper()
	var records []map[string]any
	for _, line := range bytes.Split(bytes.TrimSpace(logs.Bytes()), []byte("\n")) {
		if len(line) == 0 {
			continue
		}
		var record map[string]any
		if err := json.Unmarshal(line, &record); err != nil {
			t.Fatalf("decode log record %q: %v", line, err)
		}
		records = append(records, record)
	}
	return records
}

func TestHealthz(t *testing.T) {
	handler, logs := newTestHandler(t)
	recorder := httptest.NewRecorder()
	handler.ServeHTTP(recorder, httptest.NewRequest(http.MethodGet, "/healthz", nil))

	if recorder.Code != http.StatusOK {
		t.Fatalf("status = %d, want %d", recorder.Code, http.StatusOK)
	}
	if got := recorder.Header().Get("Content-Type"); got != "application/json" {
		t.Errorf("Content-Type = %q, want application/json", got)
	}
	var body healthResponse
	if err := json.Unmarshal(recorder.Body.Bytes(), &body); err != nil {
		t.Fatalf("decode body: %v", err)
	}
	if body.Status != "ok" {
		t.Errorf("status field = %q, want ok", body.Status)
	}

	records := decodeRecords(t, logs)
	if len(records) != 1 {
		t.Fatalf("got %d log records, want exactly 1: %v", len(records), records)
	}
	record := records[0]
	if record["msg"] != "http_request" || record["path"] != "/healthz" || record["status"] != float64(http.StatusOK) {
		t.Errorf("unexpected request log: %v", record)
	}
}

func TestUnknownRouteAndMethod(t *testing.T) {
	handler, _ := newTestHandler(t)
	for _, tc := range []struct {
		method string
		path   string
		want   int
	}{
		{http.MethodGet, "/missing", http.StatusNotFound},
		{http.MethodPost, "/healthz", http.StatusMethodNotAllowed},
	} {
		recorder := httptest.NewRecorder()
		handler.ServeHTTP(recorder, httptest.NewRequest(tc.method, tc.path, nil))
		if recorder.Code != tc.want {
			t.Errorf("%s %s status = %d, want %d", tc.method, tc.path, recorder.Code, tc.want)
		}
	}
}

// serveWithHandler runs handler behind the request logger on a real HTTP
// server and returns the client-observed status and the logged status.
func serveWithHandler(t *testing.T, handler http.HandlerFunc) (clientStatus int, loggedStatus float64, record map[string]any) {
	t.Helper()
	var logs bytes.Buffer
	logger := slog.New(slog.NewJSONHandler(&logs, nil))
	ts := httptest.NewServer(logRequests(logger, handler))
	defer ts.Close()

	resp, err := http.Get(ts.URL + "/probe")
	if err != nil {
		t.Fatalf("request: %v", err)
	}
	resp.Body.Close()
	// Close waits for in-flight handlers, so the log record is complete.
	ts.Close()

	if err := json.Unmarshal(logs.Bytes(), &record); err != nil {
		t.Fatalf("decode log record: %v (logs %q)", err, logs.String())
	}
	status, _ := record["status"].(float64)
	return resp.StatusCode, status, record
}

func TestRequestLogRecordsFirstCommittedStatus(t *testing.T) {
	for _, tc := range []struct {
		name    string
		handler http.HandlerFunc
		want    int
	}{
		{"double WriteHeader", func(w http.ResponseWriter, _ *http.Request) {
			w.WriteHeader(http.StatusInternalServerError)
			w.WriteHeader(http.StatusOK)
		}, http.StatusInternalServerError},
		{"body only", func(w http.ResponseWriter, _ *http.Request) {
			_, _ = w.Write([]byte("hello"))
		}, http.StatusOK},
		{"body then error header", func(w http.ResponseWriter, _ *http.Request) {
			_, _ = w.Write([]byte("hello"))
			w.WriteHeader(http.StatusInternalServerError)
		}, http.StatusOK},
		{"informational then final", func(w http.ResponseWriter, _ *http.Request) {
			w.WriteHeader(http.StatusEarlyHints)
			w.WriteHeader(http.StatusOK)
		}, http.StatusOK},
		{"no writes", func(http.ResponseWriter, *http.Request) {}, http.StatusOK},
	} {
		t.Run(tc.name, func(t *testing.T) {
			client, logged, record := serveWithHandler(t, tc.handler)
			if client != tc.want {
				t.Errorf("client status = %d, want %d", client, tc.want)
			}
			if logged != float64(tc.want) {
				t.Errorf("logged status = %v, want %d (record %v)", logged, tc.want, record)
			}
		})
	}
}

func TestStatusRecorderWroteHeader(t *testing.T) {
	for _, tc := range []struct {
		name string
		do   func(*statusRecorder)
		want bool
	}{
		{"untouched", func(*statusRecorder) {}, false},
		{"informational", func(r *statusRecorder) { r.WriteHeader(http.StatusEarlyHints) }, false},
		{"switching protocols", func(r *statusRecorder) { r.WriteHeader(http.StatusSwitchingProtocols) }, true},
		{"final", func(r *statusRecorder) { r.WriteHeader(http.StatusNoContent) }, true},
		{"implicit write", func(r *statusRecorder) { _, _ = r.Write([]byte("x")) }, true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			r := &statusRecorder{ResponseWriter: httptest.NewRecorder(), status: http.StatusOK}
			tc.do(r)
			if r.wroteHeader != tc.want {
				t.Errorf("wroteHeader = %v, want %v", r.wroteHeader, tc.want)
			}
		})
	}
}

// serveRequest sends one GET to handler behind the request logger on a real
// HTTP server and returns the client error (nil when a response arrived), the
// client-observed status (0 on error) and the decoded log records. net/http's
// own panic trace is discarded to keep test output quiet.
func serveRequest(t *testing.T, handler http.HandlerFunc) (clientErr error, clientStatus int, records []map[string]any) {
	t.Helper()
	var logs bytes.Buffer
	logger := slog.New(slog.NewJSONHandler(&logs, nil))
	ts := httptest.NewUnstartedServer(logRequests(logger, handler))
	ts.Config.ErrorLog = log.New(io.Discard, "", 0)
	ts.Start()
	defer ts.Close()

	resp, err := http.Get(ts.URL + "/probe")
	if err == nil {
		clientStatus = resp.StatusCode
		resp.Body.Close()
	}
	// Close waits for in-flight handlers, so every log record is complete.
	ts.Close()
	return err, clientStatus, decodeRecords(t, &logs)
}

func TestRequestLogOnPanic(t *testing.T) {
	for _, tc := range []struct {
		name       string
		handler    http.HandlerFunc
		wantStatus int
		wantBytes  int
	}{
		{"before any write", func(http.ResponseWriter, *http.Request) {
			panic("boom")
		}, http.StatusInternalServerError, 0},
		{"after committed status", func(w http.ResponseWriter, _ *http.Request) {
			w.WriteHeader(http.StatusAccepted)
			panic("boom")
		}, http.StatusAccepted, 0},
		{"after body write", func(w http.ResponseWriter, _ *http.Request) {
			_, _ = w.Write([]byte("partial"))
			panic("boom")
		}, http.StatusOK, len("partial")},
		{"abort handler", func(http.ResponseWriter, *http.Request) {
			panic(http.ErrAbortHandler)
		}, http.StatusInternalServerError, 0},
	} {
		t.Run(tc.name, func(t *testing.T) {
			clientErr, clientStatus, records := serveRequest(t, tc.handler)
			// net/http recovers the re-raised panic by closing the connection
			// without sending a response, so the client sees an error rather
			// than a 500 status.
			if clientErr == nil {
				t.Errorf("client status = %d, want a transport error from the aborted connection", clientStatus)
			}
			if len(records) != 1 {
				t.Fatalf("got %d log records, want exactly 1: %v", len(records), records)
			}
			record := records[0]
			if record["msg"] != "http_request" || record["method"] != http.MethodGet || record["path"] != "/probe" {
				t.Errorf("unexpected request log: %v", record)
			}
			if record["status"] != float64(tc.wantStatus) {
				t.Errorf("logged status = %v, want %d", record["status"], tc.wantStatus)
			}
			if record["bytes"] != float64(tc.wantBytes) {
				t.Errorf("logged bytes = %v, want %d", record["bytes"], tc.wantBytes)
			}
			if duration, ok := record["duration_ms"].(float64); !ok || duration < 0 {
				t.Errorf("duration_ms = %v, want a non-negative number", record["duration_ms"])
			}
		})
	}
}

func TestRequestLogRepanicsSamePanicValue(t *testing.T) {
	sentinel := errors.New("sentinel")
	for _, value := range []any{sentinel, http.ErrAbortHandler} {
		var logs bytes.Buffer
		logger := slog.New(slog.NewJSONHandler(&logs, nil))
		handler := logRequests(logger, http.HandlerFunc(func(http.ResponseWriter, *http.Request) {
			panic(value)
		}))

		var recovered any
		func() {
			defer func() { recovered = recover() }()
			handler.ServeHTTP(httptest.NewRecorder(), httptest.NewRequest(http.MethodGet, "/probe", nil))
		}()
		if recovered != value {
			t.Errorf("recovered %v, want the original panic value %v", recovered, value)
		}
		if got := len(decodeRecords(t, &logs)); got != 1 {
			t.Errorf("panic %v: got %d log records, want exactly 1", value, got)
		}
	}
}

// TestRequestLogBytesForHead checks that the logged bytes are the body bytes
// transmitted to the client: a HEAD response carries no body, so it logs 0 with
// the same status as the equivalent GET, while GET logs the transmitted length.
func TestRequestLogBytesForHead(t *testing.T) {
	for _, tc := range []struct {
		method         string
		path           string
		wantStatus     int
		wantBytes      float64
		wantBodyLength int
	}{
		{http.MethodGet, "/healthz", http.StatusOK, 16, 16},
		{http.MethodHead, "/healthz", http.StatusOK, 0, 0},
		{http.MethodGet, "/unknown", http.StatusNotFound, 19, 19},
		{http.MethodHead, "/unknown", http.StatusNotFound, 0, 0},
	} {
		t.Run(tc.method+" "+tc.path, func(t *testing.T) {
			handler, logs := newTestHandler(t)
			ts := httptest.NewServer(handler)
			defer ts.Close()

			req, err := http.NewRequest(tc.method, ts.URL+tc.path, nil)
			if err != nil {
				t.Fatalf("new request: %v", err)
			}
			resp, err := http.DefaultClient.Do(req)
			if err != nil {
				t.Fatalf("do request: %v", err)
			}
			body, err := io.ReadAll(resp.Body)
			resp.Body.Close()
			if err != nil {
				t.Fatalf("read body: %v", err)
			}
			if resp.StatusCode != tc.wantStatus {
				t.Errorf("client status = %d, want %d", resp.StatusCode, tc.wantStatus)
			}
			if len(body) != tc.wantBodyLength {
				t.Errorf("client body length = %d, want %d", len(body), tc.wantBodyLength)
			}
			// The response headers must be unchanged for HEAD: the same
			// Content-Length as the GET, without a body.
			if tc.method == http.MethodHead && resp.ContentLength <= 0 {
				t.Errorf("HEAD Content-Length = %d, want the GET body length", resp.ContentLength)
			}
			// Close waits for in-flight handlers, so the record is complete.
			ts.Close()

			records := decodeRecords(t, logs)
			if len(records) != 1 {
				t.Fatalf("got %d log records, want exactly 1: %v", len(records), records)
			}
			record := records[0]
			if record["method"] != tc.method || record["path"] != tc.path {
				t.Errorf("unexpected request log: %v", record)
			}
			if record["status"] != float64(tc.wantStatus) {
				t.Errorf("logged status = %v, want %d", record["status"], tc.wantStatus)
			}
			if record["bytes"] != tc.wantBytes {
				t.Errorf("logged bytes = %v, want %v", record["bytes"], tc.wantBytes)
			}
		})
	}
}
