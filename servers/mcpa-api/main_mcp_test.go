// SPDX-License-Identifier: FSL-1.1-ALv2

package main

import (
	"encoding/json"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	dto "github.com/prometheus/client_model/go"
	"golang.org/x/oauth2"

	"github.com/delinoio/apps/servers/mcpa-api/internal/app"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth/authtest"
	"github.com/delinoio/apps/servers/mcpa-api/internal/authz"
	"github.com/delinoio/apps/servers/mcpa-api/internal/config"
	"github.com/delinoio/apps/servers/mcpa-api/internal/crypto"
	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
	"github.com/delinoio/apps/servers/mcpa-api/internal/obs"
	"github.com/delinoio/apps/servers/mcpa-api/internal/service"
	"github.com/delinoio/apps/servers/mcpa-api/internal/web"
)

// TestInstallMCPMountsBrokerAndMCP assembles the root handler without a
// database and checks the MCP and OAuth routes and their metric surfaces.
func TestInstallMCPMountsBrokerAndMCP(t *testing.T) {
	logto := authtest.New(t)
	log := slog.New(slog.NewTextHandler(io.Discard, nil))
	cfg := &config.Serve{
		PublicURL: "https://api.example.test", WebOrigin: "https://console.example.test",
		APIResource: "https://api.example.test", MCPResource: "https://api.example.test/mcp", DelcoreResource: "https://delcore.example.test",
		MFAClaim: config.DefaultMFAClaim, MFAReauthURL: "https://login.example.test/mfa",
		OAuthClients: map[string]config.OAuthClient{"claude": {Name: "Claude", RedirectURIs: []string{"http://127.0.0.1:1/cb"}}},
	}
	keys, err := crypto.NewKeyring(map[string][]byte{"k1": make([]byte, 32)}, "k1")
	if err != nil {
		t.Fatal(err)
	}
	dc, err := delcore.New(delcore.Config{BaseURL: "http://127.0.0.1:1", CacheTTL: time.Minute, TokenSource: oauth2.StaticTokenSource(&oauth2.Token{AccessToken: "m2m"})})
	if err != nil {
		t.Fatal(err)
	}
	lt := auth.NewLogto(logto.Issuer(), "logto-app", "secret", nil)
	authn := auth.NewAuthenticator(auth.AuthenticatorConfig{Logto: lt, Keys: dc, WebOrigin: cfg.WebOrigin, MCPResource: cfg.MCPResource, MFAClaim: cfg.MFAClaim, ReauthURL: cfg.MFAReauthURL})
	a := &app.App{
		Config: cfg, Log: log, Metrics: obs.NewMetrics(), Keys: keys, Delcore: dc, Logto: lt, Authenticator: authn,
		Authorizer: authz.New(nil, dc, time.Minute, log),
		Service:    service.New(service.Config{PublicURL: cfg.PublicURL, Log: log}),
		Web:        web.New(web.Config{Authenticator: authn, Log: log}),
	}
	if err := installMCP(a); err != nil {
		t.Fatal(err)
	}
	h := a.Handler()
	do := func(method, path string) *httptest.ResponseRecorder {
		req := httptest.NewRequest(method, path, strings.NewReader(`{"jsonrpc":"2.0","id":1,"method":"tools/list"}`))
		req.Header.Set("Content-Type", "application/json")
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, req)
		return rec
	}

	rec := do(http.MethodGet, "/.well-known/oauth-protected-resource")
	var prm map[string]any
	if rec.Code != http.StatusOK || json.Unmarshal(rec.Body.Bytes(), &prm) != nil || prm["resource"] != cfg.MCPResource {
		t.Fatalf("protected resource metadata: %d %s", rec.Code, rec.Body.String())
	}
	if rec := do(http.MethodGet, "/.well-known/oauth-authorization-server"); rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), `"issuer":"https://api.example.test"`) {
		t.Fatalf("authorization server metadata: %d %s", rec.Code, rec.Body.String())
	}
	rec = do(http.MethodPost, "/mcp")
	if rec.Code != http.StatusUnauthorized ||
		rec.Header().Get("WWW-Authenticate") != `Bearer resource_metadata="https://api.example.test/.well-known/oauth-protected-resource"` {
		t.Fatalf("/mcp: %d %q", rec.Code, rec.Header().Get("WWW-Authenticate"))
	}
	if rec := do(http.MethodGet, "/oauth/authorize?client_id=unknown"); rec.Code != http.StatusBadRequest {
		t.Fatalf("/oauth/authorize: %d", rec.Code)
	}

	count := func(surface obs.Surface, class string) float64 {
		var m dto.Metric
		if err := a.Metrics.HTTPRequests.WithLabelValues(string(surface), class).Write(&m); err != nil {
			t.Fatal(err)
		}
		return m.GetCounter().GetValue()
	}
	if got := count(obs.SurfaceOAuth, "2xx"); got != 2 {
		t.Fatalf("oauth 2xx = %v", got)
	}
	if got := count(obs.SurfaceOAuth, "4xx"); got != 1 {
		t.Fatalf("oauth 4xx = %v", got)
	}
	if got := count(obs.SurfaceMCP, "4xx"); got != 1 {
		t.Fatalf("mcp 4xx = %v", got)
	}
}
