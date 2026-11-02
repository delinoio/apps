// SPDX-License-Identifier: FSL-1.1-ALv2

package server_test

import (
	"context"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/prometheus/client_golang/prometheus/testutil"

	"github.com/delinoio/apps/protos/mcpa/v1/mcpav1connect"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
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
