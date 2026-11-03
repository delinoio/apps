// SPDX-License-Identifier: FSL-1.1-ALv2

package server_test

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/prometheus/client_golang/prometheus/testutil"
	"google.golang.org/protobuf/proto"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/protos/mcpa/v1/mcpav1connect"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/config"
	"github.com/delinoio/apps/servers/mcpa-api/internal/obs"
	"github.com/delinoio/apps/servers/mcpa-api/internal/server"
	"github.com/delinoio/apps/servers/mcpa-api/internal/web"
)

const webOrigin = "https://mcpa.example"

func newHandler(t *testing.T, ready server.Readiness, mounts ...server.Mount) http.Handler {
	t.Helper()
	log := slog.New(slog.NewTextHandler(io.Discard, nil))
	authn := auth.NewAuthenticator(auth.AuthenticatorConfig{WebOrigin: webOrigin})
	return server.New(server.Config{
		Service:       mcpav1connect.UnimplementedMcpaServiceHandler{},
		Authenticator: authn,
		Web:           web.New(web.Config{Authenticator: authn, Log: log}),
		Metrics:       obs.NewMetrics(),
		Log:           log,
		WebOrigin:     webOrigin,
		MetricsToken:  "metrics-token",
		Ready:         ready,
		Mounts:        mounts,
	})
}

func do(h http.Handler, method, path string, header map[string]string) *httptest.ResponseRecorder {
	req := httptest.NewRequest(method, path, strings.NewReader("{}"))
	for k, v := range header {
		req.Header.Set(k, v)
	}
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	return rec
}

const getViewer = "/" + mcpav1connect.McpaServiceName + "/GetViewer"

func TestEveryResponseCarriesRequestID(t *testing.T) {
	h := newHandler(t, nil)
	for _, path := range []string{"/healthz", "/readyz", "/auth/session", getViewer} {
		method := http.MethodGet
		if path == getViewer {
			method = http.MethodPost
		}
		rec := do(h, method, path, map[string]string{"Content-Type": "application/json"})
		if rec.Header().Get(server.RequestIDHeader) == "" {
			t.Errorf("%s: missing %s", path, server.RequestIDHeader)
		}
	}
}

// TestMuxGeneratedResponsesCarryRequestID covers responses that the ServeMux
// produces before any route is instrumented: unknown paths and wrong methods.
func TestMuxGeneratedResponsesCarryRequestID(t *testing.T) {
	h := newHandler(t, nil)
	cases := []struct {
		method, path string
		status       int
	}{
		{http.MethodGet, "/", http.StatusNotFound},
		{http.MethodGet, "/unknown", http.StatusNotFound},
		{http.MethodPost, "/unknown", http.StatusNotFound},
		// "OPTIONS /auth/" makes the mux answer other methods under /auth/ with 405.
		{http.MethodGet, "/auth/unknown", http.StatusMethodNotAllowed},
		{http.MethodPost, "/healthz", http.StatusMethodNotAllowed},
		{http.MethodPost, "/readyz", http.StatusMethodNotAllowed},
		{http.MethodGet, "/auth/logout", http.StatusMethodNotAllowed},
		{http.MethodGet, "/auth/refresh", http.StatusMethodNotAllowed},
	}
	seen := map[string]bool{}
	for _, tc := range cases {
		rec := do(h, tc.method, tc.path, nil)
		if rec.Code != tc.status {
			t.Errorf("%s %s: status = %d, want %d", tc.method, tc.path, rec.Code, tc.status)
		}
		values := rec.Header().Values(server.RequestIDHeader)
		if len(values) != 1 || values[0] == "" {
			t.Errorf("%s %s: %s = %q, want exactly one non-empty value", tc.method, tc.path, server.RequestIDHeader, values)
			continue
		}
		if seen[values[0]] {
			t.Errorf("%s %s: request ID %q repeated across requests", tc.method, tc.path, values[0])
		}
		seen[values[0]] = true
	}
}

// TestRoutedRequestIDMatchesConnectTraceID checks that routed responses keep
// one ID, shared by the header and the typed Connect error detail.
func TestRoutedRequestIDMatchesConnectTraceID(t *testing.T) {
	rec := do(newHandler(t, nil), http.MethodPost, getViewer, map[string]string{"Content-Type": "application/json"})
	values := rec.Header().Values(server.RequestIDHeader)
	if len(values) != 1 || values[0] == "" {
		t.Fatalf("%s = %q, want exactly one non-empty value", server.RequestIDHeader, values)
	}
	var body struct {
		Details []struct {
			Value string `json:"value"`
		} `json:"details"`
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &body); err != nil || len(body.Details) == 0 {
		t.Fatalf("Connect error body without details: %v %s", err, rec.Body.String())
	}
	raw, err := base64.RawStdEncoding.DecodeString(strings.TrimRight(body.Details[0].Value, "="))
	if err != nil {
		t.Fatalf("decode detail: %v", err)
	}
	var detail mcpav1.ErrorDetail
	if err := proto.Unmarshal(raw, &detail); err != nil {
		t.Fatalf("unmarshal detail: %v", err)
	}
	if detail.GetTraceId() != values[0] {
		t.Fatalf("trace_id %q != %s %q", detail.GetTraceId(), server.RequestIDHeader, values[0])
	}
}

func TestConnectRequiresAuthentication(t *testing.T) {
	rec := do(newHandler(t, nil), http.MethodPost, getViewer, map[string]string{"Content-Type": "application/json"})
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("status = %d, want 401", rec.Code)
	}
	if !strings.Contains(rec.Body.String(), "ERROR_REASON_UNAUTHENTICATED") {
		t.Fatalf("missing typed reason: %s", rec.Body.String())
	}
}

func TestBrowserPolicy(t *testing.T) {
	h := newHandler(t, nil)

	rec := do(h, http.MethodOptions, getViewer, map[string]string{"Origin": webOrigin, "Access-Control-Request-Method": "POST"})
	if rec.Code != http.StatusNoContent || rec.Header().Get("Access-Control-Allow-Origin") != webOrigin ||
		rec.Header().Get("Access-Control-Allow-Credentials") != "true" {
		t.Fatalf("preflight: status %d headers %v", rec.Code, rec.Header())
	}
	if !strings.Contains(rec.Header().Get("Access-Control-Expose-Headers"), server.RequestIDHeader) {
		t.Fatalf("CORS does not expose %s", server.RequestIDHeader)
	}

	rec = do(h, http.MethodPost, getViewer, map[string]string{"Origin": "https://evil.example", "Content-Type": "application/json"})
	if rec.Code != http.StatusForbidden || rec.Header().Get("Access-Control-Allow-Origin") != "" {
		t.Fatalf("foreign origin: status %d headers %v", rec.Code, rec.Header())
	}

	// A top-level navigation to /auth/login from another origin is admitted
	// and fails on its own return_to validation instead.
	rec = do(h, http.MethodGet, "/auth/login?return_to=https://evil.example/", map[string]string{"Origin": "https://evil.example"})
	if rec.Code == http.StatusForbidden {
		t.Fatalf("navigation was rejected by the origin policy: %s", rec.Body.String())
	}
}

func TestMountsGetNoCredentialedCORS(t *testing.T) {
	h := newHandler(t, nil, server.Mount{Pattern: "/r/{resource_id}/", Surface: obs.SurfaceProxy, Handler: http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set(server.RequestIDHeader, "call-record-id")
		w.WriteHeader(http.StatusTeapot)
	})})
	rec := do(h, http.MethodPost, "/r/abc/v1/chat/completions", map[string]string{"Origin": webOrigin})
	if rec.Code != http.StatusTeapot {
		t.Fatalf("status = %d", rec.Code)
	}
	if rec.Header().Get("Access-Control-Allow-Credentials") != "" {
		t.Fatal("mounted surface received credentialed CORS")
	}
	if got := rec.Header().Get(server.RequestIDHeader); got != "call-record-id" {
		t.Fatalf("mount could not replace the request ID: %q", got)
	}
}

func TestMetricsToken(t *testing.T) {
	h := newHandler(t, nil)
	if rec := do(h, http.MethodGet, "/metrics", nil); rec.Code != http.StatusUnauthorized {
		t.Fatalf("without token: %d", rec.Code)
	}
	if rec := do(h, http.MethodGet, "/metrics", map[string]string{"Authorization": "Bearer wrong"}); rec.Code != http.StatusUnauthorized {
		t.Fatalf("wrong token: %d", rec.Code)
	}
	rec := do(h, http.MethodGet, "/metrics", map[string]string{"Authorization": "Bearer metrics-token"})
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "go_goroutines") {
		t.Fatalf("with token: %d", rec.Code)
	}
}

func TestReadiness(t *testing.T) {
	failing := newHandler(t, func(context.Context) error { return errors.New("schema mismatch: secret detail") })
	rec := do(failing, http.MethodGet, "/readyz", nil)
	if rec.Code != http.StatusServiceUnavailable || strings.Contains(rec.Body.String(), "secret") {
		t.Fatalf("failing readiness: %d %s", rec.Code, rec.Body.String())
	}
	if rec := do(newHandler(t, nil), http.MethodGet, "/readyz", nil); rec.Code != http.StatusOK {
		t.Fatalf("ready: %d", rec.Code)
	}
}

func TestSelfMeteredMountIsCountedOnce(t *testing.T) {
	log := slog.New(slog.NewTextHandler(io.Discard, nil))
	authn := auth.NewAuthenticator(auth.AuthenticatorConfig{WebOrigin: webOrigin})
	metrics := obs.NewMetrics()
	h := server.New(server.Config{
		Service: mcpav1connect.UnimplementedMcpaServiceHandler{}, Authenticator: authn,
		Web: web.New(web.Config{Authenticator: authn, Log: log}), Metrics: metrics, Log: log, WebOrigin: webOrigin,
		Mounts: []server.Mount{
			{Pattern: "/r/", Surface: obs.SurfaceProxy, SelfMetered: true, Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				metrics.HTTPRequests.WithLabelValues(string(obs.SurfaceProxy), "2xx").Inc()
			})},
			{Pattern: "/mcp", Surface: obs.SurfaceMCP, Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {})},
		},
	})
	do(h, http.MethodPost, "/r/x/v1/chat/completions", nil)
	do(h, http.MethodPost, "/mcp", nil)
	if got := testutil.ToFloat64(metrics.HTTPRequests.WithLabelValues("proxy", "2xx")); got != 1 {
		t.Fatalf("proxy requests counted %v times", got)
	}
	if got := testutil.ToFloat64(metrics.HTTPRequests.WithLabelValues("mcp", "2xx")); got != 1 {
		t.Fatalf("mcp requests counted %v times", got)
	}
}

// TestRequestLogsCarryTraceIDAndSurface captures real JSON log output and
// checks that *Context lines written inside each surface carry the response's
// request ID and the surface, exactly once.
func TestRequestLogsCarryTraceIDAndSurface(t *testing.T) {
	var buf bytes.Buffer
	log := obs.NewLogger(&buf, config.Logging{Format: config.LogFormatJSON})
	authn := auth.NewAuthenticator(auth.AuthenticatorConfig{WebOrigin: webOrigin})
	logging := func(msg string) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			log.WarnContext(r.Context(), msg)
			w.WriteHeader(http.StatusNoContent)
		})
	}
	h := server.New(server.Config{
		Service: mcpav1connect.UnimplementedMcpaServiceHandler{}, Authenticator: authn,
		Web: web.New(web.Config{Authenticator: authn, Log: log}), Metrics: obs.NewMetrics(), Log: log, WebOrigin: webOrigin,
		Mounts: []server.Mount{
			{Pattern: "/oauth/", Surface: obs.SurfaceOAuth, Handler: logging("oauth line")},
			{Pattern: "/mcp", Surface: obs.SurfaceMCP, Handler: logging("mcp line")},
			{Pattern: "/r/", Surface: obs.SurfaceProxy, SelfMetered: true, Handler: logging("proxy line")},
		},
	})
	log.Info("startup line")

	cases := []struct {
		method, path, msg, surface string
	}{
		{http.MethodPost, getViewer, "management call rejected", "management"},
		{http.MethodPost, "/oauth/token", "oauth line", "oauth"},
		{http.MethodPost, "/mcp", "mcp line", "mcp"},
		{http.MethodPost, "/r/x/v1/chat/completions", "proxy line", "proxy"},
	}
	ids := map[string]string{}
	for _, tc := range cases {
		rec := do(h, tc.method, tc.path, map[string]string{"Content-Type": "application/json"})
		ids[tc.msg] = rec.Header().Get(server.RequestIDHeader)
	}

	seen := map[string]bool{}
	for _, line := range strings.Split(strings.TrimSpace(buf.String()), "\n") {
		if n := strings.Count(line, `"trace_id"`); n > 1 {
			t.Errorf("duplicate trace_id in %s", line)
		}
		var m map[string]any
		if err := json.Unmarshal([]byte(line), &m); err != nil {
			t.Fatalf("not JSON: %q", line)
		}
		msg, _ := m["msg"].(string)
		if msg == "startup line" {
			if _, ok := m["trace_id"]; ok {
				t.Errorf("startup line changed: %s", line)
			}
			if _, ok := m["surface"]; ok {
				t.Errorf("startup line changed: %s", line)
			}
			continue
		}
		for _, tc := range cases {
			if tc.msg != msg {
				continue
			}
			seen[msg] = true
			if m["surface"] != tc.surface {
				t.Errorf("%s: surface = %v, want %s", msg, m["surface"], tc.surface)
			}
			if tc.surface == "proxy" {
				// The call surface replaces the request ID with the call ID.
				if _, ok := m["trace_id"]; ok {
					t.Errorf("%s: unexpected trace_id", msg)
				}
			} else if m["trace_id"] != ids[msg] || ids[msg] == "" {
				t.Errorf("%s: trace_id = %v, want response header %q", msg, m["trace_id"], ids[msg])
			}
		}
	}
	for _, tc := range cases {
		if !seen[tc.msg] {
			t.Errorf("no log line %q in %s", tc.msg, buf.String())
		}
	}
}
