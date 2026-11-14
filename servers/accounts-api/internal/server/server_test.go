// SPDX-License-Identifier: FSL-1.1-ALv2

package server

import (
	"bytes"
	"encoding/json"
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

	var record map[string]any
	if err := json.Unmarshal(logs.Bytes(), &record); err != nil {
		t.Fatalf("decode log record: %v", err)
	}
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
