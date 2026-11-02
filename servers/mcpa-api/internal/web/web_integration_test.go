// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package web_test

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
	"testing"
	"time"

	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth/authtest"
	"github.com/delinoio/apps/servers/mcpa-api/internal/crypto"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store/storetest"
	"github.com/delinoio/apps/servers/mcpa-api/internal/web"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
)

const (
	webOrigin = "https://console.example.test"
	reauthURL = "https://login.example.test/mfa"
)

type fixture struct {
	db            *store.Store
	sessions      *auth.Sessions
	authenticator *auth.Authenticator
	mux           *http.ServeMux
}

// newFixture serves /auth/* with a Logto provider that is unavailable:
// every token request fails.
func newFixture(t *testing.T, provider *authtest.Provider) fixture {
	t.Helper()
	db := storetest.Open(t)
	key := make([]byte, 32)
	_, _ = rand.Read(key)
	keys, err := crypto.NewKeyring(map[string][]byte{"k1": key}, "k1")
	if err != nil {
		t.Fatal(err)
	}
	log := slog.New(slog.NewTextHandler(io.Discard, nil))
	provider.TokenHandler = func(url.Values) (int, any) {
		return http.StatusServiceUnavailable, map[string]string{"error": "temporarily_unavailable"}
	}
	logto := auth.NewLogto(provider.Issuer(), "console-client", "secret", provider.Server.Client())
	sessions := auth.NewSessions(auth.SessionsConfig{
		Store: db, Keys: keys, Logto: logto, APIResource: "https://api.example.test", DelcoreResource: "https://delcore.example.test", MFAClaim: "mcpa_mfa", Log: log,
	})
	authenticator := auth.NewAuthenticator(auth.AuthenticatorConfig{Sessions: sessions, Logto: logto, WebOrigin: webOrigin, MFAClaim: "mcpa_mfa", ReauthURL: reauthURL})
	mux := http.NewServeMux()
	web.New(web.Config{Store: db, Logto: logto, Authenticator: authenticator, Log: log}).Register(mux, func(h http.Handler) http.Handler { return h })
	return fixture{db: db, sessions: sessions, authenticator: authenticator, mux: mux}
}

func (f fixture) logout(cookie, csrf string) *httptest.ResponseRecorder {
	req := httptest.NewRequest(http.MethodPost, "/auth/logout", nil)
	req.Header.Set("Origin", webOrigin)
	req.Header.Set(auth.CSRFHeader, csrf)
	req.AddCookie(&http.Cookie{Name: auth.SessionCookie, Value: cookie})
	rec := httptest.NewRecorder()
	f.mux.ServeHTTP(rec, req)
	return rec
}

func clearsSessionCookie(rec *httptest.ResponseRecorder) bool {
	for _, c := range rec.Result().Cookies() {
		if c.Name == auth.SessionCookie && c.MaxAge < 0 {
			return true
		}
	}
	return false
}

func TestLogoutDoesNotRefreshTokens(t *testing.T) {
	provider := authtest.New(t)
	f := newFixture(t, provider)
	db, sessions := f.db, f.sessions
	ctx := context.Background()
	// The access token expires now, inside the refresh margin, so a session
	// lookup would refresh it through Logto.
	cookie, sess, err := sessions.Create(ctx, auth.NewSessionInput{
		UserID: "user-1", Subject: "sub-1", MFA: true, AccessExpiry: time.Now(),
		Tokens: auth.SessionTokens{APIAccessToken: "api", DelcoreAccessToken: "dc", DelcoreExpiry: time.Now(), RefreshToken: "rt"},
	})
	if err != nil {
		t.Fatal(err)
	}
	logout := func(csrf string) *httptest.ResponseRecorder { return f.logout(cookie, csrf) }

	// A wrong CSRF token is still rejected and keeps the session.
	if rec := logout("wrong"); rec.Code != http.StatusForbidden {
		t.Fatalf("logout with a wrong CSRF token returned %d, want 403", rec.Code)
	}
	if _, err := store.SessionByTokenHash(ctx, db.Pool, auth.Digest(cookie)); err != nil {
		t.Fatalf("the session was removed by a rejected logout: %v", err)
	}

	rec := logout(sess.CSRFToken)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("logout returned %d %s, want 204", rec.Code, rec.Body.String())
	}
	if !clearsSessionCookie(rec) {
		t.Fatal("logout did not clear the session cookie")
	}
	if _, err := store.SessionByTokenHash(ctx, db.Pool, auth.Digest(cookie)); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("session lookup after logout returned %v, want not found", err)
	}
	if n := len(provider.TokenRequests); n != 0 {
		t.Fatalf("logout sent %d token requests to Logto", n)
	}
}

// TestRestrictedSessionCanLogOut covers a session created without MFA
// evidence: /auth/session must give the browser the CSRF token that
// /auth/logout requires, while the session still has no management access.
func TestRestrictedSessionCanLogOut(t *testing.T) {
	f := newFixture(t, authtest.New(t))
	ctx := context.Background()
	cookie, sess, err := f.sessions.Create(ctx, auth.NewSessionInput{
		UserID: "user-1", Subject: "sub-1", MFA: false, AccessExpiry: time.Now().Add(time.Hour),
	})
	if err != nil {
		t.Fatal(err)
	}

	req := httptest.NewRequest(http.MethodGet, "/auth/session", nil)
	req.AddCookie(&http.Cookie{Name: auth.SessionCookie, Value: cookie})
	rec := httptest.NewRecorder()
	f.mux.ServeHTTP(rec, req)
	if rec.Code != http.StatusForbidden {
		t.Fatalf("session returned %d %s, want 403", rec.Code, rec.Body.String())
	}
	var body map[string]string
	if err := json.Unmarshal(rec.Body.Bytes(), &body); err != nil {
		t.Fatal(err)
	}
	if body["error"] != "mfa_required" || body["reauthentication_url"] != reauthURL {
		t.Fatalf("session body = %v, want mfa_required with the reauthentication URL", body)
	}
	if _, ok := body["user_id"]; ok {
		t.Fatalf("restricted session body exposes the user: %v", body)
	}
	csrf := body["csrf_token"]
	if csrf == "" || csrf != sess.CSRFToken {
		t.Fatalf("session body csrf_token = %q, want the session CSRF token", csrf)
	}

	// The CSRF token does not unlock management calls.
	h := http.Header{}
	h.Set("Origin", webOrigin)
	h.Set(auth.CSRFHeader, csrf)
	h.Add("Cookie", (&http.Cookie{Name: auth.SessionCookie, Value: cookie}).String())
	if _, err := f.authenticator.Request(ctx, h); apperr.As(err).Reason != mcpav1.ErrorReason_ERROR_REASON_MFA_REQUIRED {
		t.Fatalf("management request with a restricted session returned %v, want MFA required", err)
	}

	rec = f.logout(cookie, csrf)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("logout returned %d %s, want 204", rec.Code, rec.Body.String())
	}
	if !clearsSessionCookie(rec) {
		t.Fatal("logout did not clear the session cookie")
	}
	if _, err := store.SessionByTokenHash(ctx, f.db.Pool, auth.Digest(cookie)); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("session lookup after logout returned %v, want not found", err)
	}
}
