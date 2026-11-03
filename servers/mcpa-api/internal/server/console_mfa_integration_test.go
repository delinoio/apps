// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package server_test

import (
	"context"
	"crypto/rand"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
	"time"

	"connectrpc.com/connect"

	delcorev1 "github.com/delinoio/apps/protos/delcore/v1"
	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/protos/mcpa/v1/mcpav1connect"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth/authtest"
	"github.com/delinoio/apps/servers/mcpa-api/internal/authz"
	"github.com/delinoio/apps/servers/mcpa-api/internal/crypto"
	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
	"github.com/delinoio/apps/servers/mcpa-api/internal/obs"
	"github.com/delinoio/apps/servers/mcpa-api/internal/server"
	"github.com/delinoio/apps/servers/mcpa-api/internal/service"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store/storetest"
	"github.com/delinoio/apps/servers/mcpa-api/internal/web"
)

const (
	mfaReauthURL    = "https://login.example.test/mfa"
	mfaAPIAudience  = "https://api.example.test"
	mfaDelcoreAud   = "https://delcore.example.test"
	mfaTestTenant   = "tenant-mfa"
	mfaTestUser     = "user-1"
	mfaTestSubject  = "sub-1"
	mfaWebOriginURL = "https://console.example.test"
)

// mfaDelcore is a Delcore double that allows every capability and serves one
// user in one tenant. It counts the user-token calls so that tests can prove
// that no data was read for a request that lost MFA evidence.
type mfaDelcore struct {
	userCalls   int
	groupsCalls int
}

func (d *mfaDelcore) Authorize(context.Context, string, string, delcore.Capability, bool) (delcore.Decision, error) {
	return delcore.Decision{Allowed: true}, nil
}

func (d *mfaDelcore) AuthorizeAPIKey(context.Context, string, delcore.Capability, bool) (delcore.KeyDecision, error) {
	return delcore.KeyDecision{}, nil
}

func (d *mfaDelcore) ListGroups(context.Context, string, string) ([]*delcorev1.Group, error) {
	d.groupsCalls++
	return []*delcorev1.Group{{Id: "group-1", Name: "Engineering", UserIds: []string{mfaTestUser}}}, nil
}

func (d *mfaDelcore) GetCurrentUser(context.Context, string) (delcore.CurrentUser, error) {
	d.userCalls++
	return delcore.CurrentUser{UserID: mfaTestUser, Tenants: []*delcorev1.Tenant{{Id: mfaTestTenant, Name: "Tenant"}}}, nil
}

func (d *mfaDelcore) BillingPeriod(context.Context, string) (delcore.Period, error) {
	return delcore.Period{}, nil
}

type mfaFixture struct {
	t        *testing.T
	provider *authtest.Provider
	db       *store.Store
	sessions *auth.Sessions
	dc       *mfaDelcore
	server   *httptest.Server
	client   mcpav1connect.McpaServiceClient
}

// newMFAFixture serves the management API and /auth/* over the real handler
// stack with a fake Logto and a fake Delcore. The Logto token endpoint
// answers the refresh of every console session with grant, which is the
// grant a test wants Logto to give: an API token with or without MFA
// evidence, or an invalid_grant failure.
func newMFAFixture(t *testing.T, apiTokenHasMFA bool, invalidGrant bool) *mfaFixture {
	t.Helper()
	provider := authtest.New(t)
	db := storetest.Open(t)
	key := make([]byte, 32)
	_, _ = rand.Read(key)
	keys, err := crypto.NewKeyring(map[string][]byte{"k1": key}, "k1")
	if err != nil {
		t.Fatal(err)
	}
	log := slog.New(slog.NewTextHandler(io.Discard, nil))
	provider.SetTokenHandler(func(form url.Values) (int, any) {
		if invalidGrant {
			return http.StatusBadRequest, map[string]string{"error": "invalid_grant"}
		}
		switch form.Get("resource") {
		case mfaAPIAudience:
			claims := map[string]any{"sub": mfaTestSubject, "aud": mfaAPIAudience}
			if apiTokenHasMFA {
				claims["mcpa_mfa"] = true
			}
			return http.StatusOK, map[string]any{
				"access_token": provider.Sign(t, claims), "token_type": "Bearer", "expires_in": 3600,
			}
		case mfaDelcoreAud:
			return http.StatusOK, map[string]any{"access_token": "dc-new", "token_type": "Bearer", "expires_in": 3600}
		}
		return http.StatusBadRequest, map[string]string{"error": "invalid_target"}
	})
	logto := auth.NewLogto(provider.Issuer(), "console-client", "secret", provider.Server.Client())
	sessions := auth.NewSessions(auth.SessionsConfig{
		Store: db, Keys: keys, Logto: logto, APIResource: mfaAPIAudience, DelcoreResource: mfaDelcoreAud,
		MFAClaim: "mcpa_mfa", ReauthURL: mfaReauthURL, Log: log,
	})
	authenticator := auth.NewAuthenticator(auth.AuthenticatorConfig{
		Sessions: sessions, Logto: logto, WebOrigin: mfaWebOriginURL, MFAClaim: "mcpa_mfa", ReauthURL: mfaReauthURL,
	})
	dc := &mfaDelcore{}
	svc := service.New(service.Config{
		Store: db, Delcore: dc, PublicURL: "https://mcpa.example.test", Log: log,
		Authorizer: authz.New(db, dc, time.Minute, log),
	})
	handler := server.New(server.Config{
		Service: svc, Authenticator: authenticator, Metrics: obs.NewMetrics(), Log: log, WebOrigin: mfaWebOriginURL,
		Web: web.New(web.Config{Store: db, Logto: logto, Authenticator: authenticator, Delcore: dc, Log: log}),
	})
	srv := httptest.NewServer(handler)
	t.Cleanup(srv.Close)
	return &mfaFixture{
		t: t, provider: provider, db: db, sessions: sessions, dc: dc, server: srv,
		client: mcpav1connect.NewMcpaServiceClient(srv.Client(), srv.URL),
	}
}

// consoleSession stores an MFA-verified console session whose Delcore token
// is inside the refresh margin, so the first Delcore call of a request
// refreshes it, while the API access token is still valid, so cookie
// authentication admits the request without refreshing.
func (f *mfaFixture) consoleSession() (cookie, csrf string, id string) {
	f.t.Helper()
	cookie, sess, err := f.sessions.Create(context.Background(), auth.NewSessionInput{
		UserID: mfaTestUser, Subject: mfaTestSubject, MFA: true, AccessExpiry: time.Now().Add(time.Hour),
		Tokens: auth.SessionTokens{
			APIAccessToken: "api-0", DelcoreAccessToken: "dc-0", DelcoreExpiry: time.Now().Add(10 * time.Second), RefreshToken: "rt-0",
		},
	})
	if err != nil {
		f.t.Fatal(err)
	}
	return cookie, sess.CSRFToken, sess.ID.String()
}

// call sends one management request authenticated with the console cookie.
func call[Req, Resp any](f *mfaFixture, cookie, csrf string, fn func(context.Context, *connect.Request[Req]) (*connect.Response[Resp], error), msg *Req) (*Resp, error) {
	req := connect.NewRequest(msg)
	req.Header().Set("Origin", mfaWebOriginURL)
	req.Header().Set(auth.CSRFHeader, csrf)
	req.Header().Set("Cookie", (&http.Cookie{Name: auth.SessionCookie, Value: cookie}).String())
	resp, err := fn(context.Background(), req)
	if err != nil {
		return nil, err
	}
	return resp.Msg, nil
}

func wantMFARequired(t *testing.T, what string, err error) {
	t.Helper()
	if err == nil {
		t.Fatalf("%s succeeded, want ERROR_REASON_MFA_REQUIRED", what)
	}
	e := apperr.As(err)
	if e.Reason != mcpav1.ErrorReason_ERROR_REASON_MFA_REQUIRED || e.ReauthURL != mfaReauthURL {
		t.Fatalf("%s returned reason %s reauth %q (%v), want MFA_REQUIRED with %q", what, e.Reason, e.ReauthURL, err, mfaReauthURL)
	}
}

// requireDowngradedSession checks that /auth/session now reports 403
// mfa_required with a CSRF token and that logout with it succeeds.
func (f *mfaFixture) requireDowngradedSession(cookie, csrf string) {
	t := f.t
	t.Helper()
	req, _ := http.NewRequest(http.MethodGet, f.server.URL+"/auth/session", nil)
	req.AddCookie(&http.Cookie{Name: auth.SessionCookie, Value: cookie})
	resp, err := f.server.Client().Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	var body map[string]string
	if err := json.NewDecoder(resp.Body).Decode(&body); err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != http.StatusForbidden || body["error"] != "mfa_required" || body["reauthentication_url"] != mfaReauthURL {
		t.Fatalf("GET /auth/session returned %d %v, want 403 mfa_required with the reauthentication URL", resp.StatusCode, body)
	}
	if body["csrf_token"] == "" || body["csrf_token"] != csrf {
		t.Fatalf("GET /auth/session csrf_token = %q, want the session CSRF token", body["csrf_token"])
	}
	if _, ok := body["user_id"]; ok {
		t.Fatalf("restricted session body exposes the user: %v", body)
	}
	logout, _ := http.NewRequest(http.MethodPost, f.server.URL+"/auth/logout", strings.NewReader(""))
	logout.Header.Set("Origin", mfaWebOriginURL)
	logout.Header.Set(auth.CSRFHeader, body["csrf_token"])
	logout.AddCookie(&http.Cookie{Name: auth.SessionCookie, Value: cookie})
	lr, err := f.server.Client().Do(logout)
	if err != nil {
		t.Fatal(err)
	}
	lr.Body.Close()
	if lr.StatusCode != http.StatusNoContent {
		t.Fatalf("POST /auth/logout returned %d, want 204", lr.StatusCode)
	}
	if _, err := store.SessionByTokenHash(context.Background(), f.db.Pool, auth.Digest(cookie)); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("session lookup after logout returned %v, want not found", err)
	}
}

// TestConsoleRefreshWithoutMFAReportsMFARequired covers a console request
// admitted with a session that still carried MFA evidence, whose in-request
// Delcore token refresh returns an API token without it. Every management
// read of that request must fail with MFA_REQUIRED and the reauthentication
// URL, serve no data, and leave a session that can only log out.
func TestConsoleRefreshWithoutMFAReportsMFARequired(t *testing.T) {
	cases := []struct {
		name string
		run  func(f *mfaFixture, cookie, csrf, resourceID string) error
	}{
		{"GetViewer", func(f *mfaFixture, cookie, csrf, _ string) error {
			_, err := call(f, cookie, csrf, f.client.GetViewer, &mcpav1.GetViewerRequest{})
			return err
		}},
		{"ListTenantGroups", func(f *mfaFixture, cookie, csrf, _ string) error {
			_, err := call(f, cookie, csrf, f.client.ListTenantGroups, &mcpav1.ListTenantGroupsRequest{TenantId: mfaTestTenant})
			return err
		}},
		{"ListResources", func(f *mfaFixture, cookie, csrf, _ string) error {
			resp, err := call(f, cookie, csrf, f.client.ListResources, &mcpav1.ListResourcesRequest{TenantId: mfaTestTenant})
			if err == nil && len(resp.GetResources()) > 0 {
				t.Errorf("ListResources served %d resources on a request that lost MFA evidence", len(resp.GetResources()))
			}
			return err
		}},
		{"GetResource", func(f *mfaFixture, cookie, csrf, resourceID string) error {
			resp, err := call(f, cookie, csrf, f.client.GetResource, &mcpav1.GetResourceRequest{ResourceId: resourceID})
			if err == nil {
				t.Errorf("GetResource served %v on a request that lost MFA evidence", resp.GetResource())
			}
			return err
		}},
		{"GetUsage", func(f *mfaFixture, cookie, csrf, _ string) error {
			_, err := call(f, cookie, csrf, f.client.GetUsage, &mcpav1.GetUsageRequest{TenantId: mfaTestTenant})
			return err
		}},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			f := newMFAFixture(t, false, false)
			// A resource the console user manages, so that a leak would be visible.
			owner := auth.WithPrincipal(context.Background(), auth.NewUserPrincipal(mcpav1.ActorKind_ACTOR_KIND_CONSOLE, mfaTestUser, mfaTestSubject, nil))
			created, err := service.New(service.Config{
				Store: f.db, PublicURL: "https://mcpa.example.test", Log: slog.New(slog.NewTextHandler(io.Discard, nil)),
				Authorizer: authz.New(f.db, f.dc, time.Minute, slog.New(slog.NewTextHandler(io.Discard, nil))),
			}).CreateResource(owner, connect.NewRequest(&mcpav1.CreateResourceRequest{RequestKey: store.NewID().String(), TenantId: mfaTestTenant, Name: "secret-resource"}))
			if err != nil {
				t.Fatal(err)
			}
			cookie, csrf, _ := f.consoleSession()

			wantMFARequired(t, c.name, c.run(f, cookie, csrf, created.Msg.GetResource().GetId()))
			if f.dc.userCalls != 0 {
				t.Fatalf("%s read the Delcore user %d times after the MFA loss", c.name, f.dc.userCalls)
			}
			f.requireDowngradedSession(cookie, csrf)
		})
	}
}

// TestConsoleRefreshInvalidGrantEndsSession keeps the existing contract: a
// refresh token Logto rejects deletes the session and reports
// UNAUTHENTICATED, not MFA_REQUIRED.
func TestConsoleRefreshInvalidGrantEndsSession(t *testing.T) {
	f := newMFAFixture(t, true, true)
	cookie, csrf, _ := f.consoleSession()
	_, err := call(f, cookie, csrf, f.client.GetViewer, &mcpav1.GetViewerRequest{})
	if e := apperr.As(err); e.Reason != mcpav1.ErrorReason_ERROR_REASON_UNAUTHENTICATED {
		t.Fatalf("GetViewer with a rejected refresh token returned %v, want UNAUTHENTICATED", err)
	}
	if _, err := store.SessionByTokenHash(context.Background(), f.db.Pool, auth.Digest(cookie)); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("session lookup after invalid_grant returned %v, want not found", err)
	}
}

// TestConsoleRefreshWithMFAServesData is the control case: the same request
// with a refresh that keeps the MFA evidence serves the viewer.
func TestConsoleRefreshWithMFAServesData(t *testing.T) {
	f := newMFAFixture(t, true, false)
	cookie, csrf, _ := f.consoleSession()
	resp, err := call(f, cookie, csrf, f.client.GetViewer, &mcpav1.GetViewerRequest{})
	if err != nil {
		t.Fatalf("GetViewer with a refresh that keeps MFA evidence returned %v", err)
	}
	if resp.GetViewer().GetUserId() != mfaTestUser || len(resp.GetViewer().GetTenants()) != 1 {
		t.Fatalf("viewer = %v", resp.GetViewer())
	}
}
