// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package web_test

import (
	"context"
	"crypto/rand"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"net/url"
	"testing"
	"time"

	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth/authtest"
	"github.com/delinoio/apps/servers/mcpa-api/internal/crypto"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store/storetest"
	"github.com/delinoio/apps/servers/mcpa-api/internal/web"
)

const webOrigin = "https://console.example.test"

func TestLogoutDoesNotRefreshTokens(t *testing.T) {
	db := storetest.Open(t)
	key := make([]byte, 32)
	_, _ = rand.Read(key)
	keys, err := crypto.NewKeyring(map[string][]byte{"k1": key}, "k1")
	if err != nil {
		t.Fatal(err)
	}
	log := slog.New(slog.NewTextHandler(io.Discard, nil))
	// Logto is unavailable: every token request fails.
	provider := authtest.New(t)
	provider.TokenHandler = func(url.Values) (int, any) {
		return http.StatusServiceUnavailable, map[string]string{"error": "temporarily_unavailable"}
	}
	logto := auth.NewLogto(provider.Issuer(), "console-client", "secret", provider.Server.Client())
	sessions := auth.NewSessions(auth.SessionsConfig{
		Store: db, Keys: keys, Logto: logto, APIResource: "https://api.example.test", DelcoreResource: "https://delcore.example.test", MFAClaim: "mcpa_mfa", Log: log,
	})
	authenticator := auth.NewAuthenticator(auth.AuthenticatorConfig{Sessions: sessions, Logto: logto, WebOrigin: webOrigin, MFAClaim: "mcpa_mfa"})
	mux := http.NewServeMux()
	web.New(web.Config{Store: db, Logto: logto, Authenticator: authenticator, Log: log}).Register(mux, func(h http.Handler) http.Handler { return h })

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
	logout := func(csrf string) *httptest.ResponseRecorder {
		req := httptest.NewRequest(http.MethodPost, "/auth/logout", nil)
		req.Header.Set("Origin", webOrigin)
		req.Header.Set(auth.CSRFHeader, csrf)
		req.AddCookie(&http.Cookie{Name: auth.SessionCookie, Value: cookie})
		rec := httptest.NewRecorder()
		mux.ServeHTTP(rec, req)
		return rec
	}

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
	cleared := false
	for _, c := range rec.Result().Cookies() {
		if c.Name == auth.SessionCookie && c.MaxAge < 0 {
			cleared = true
		}
	}
	if !cleared {
		t.Fatal("logout did not clear the session cookie")
	}
	if _, err := store.SessionByTokenHash(ctx, db.Pool, auth.Digest(cookie)); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("session lookup after logout returned %v, want not found", err)
	}
	if n := len(provider.TokenRequests); n != 0 {
		t.Fatalf("logout sent %d token requests to Logto", n)
	}
}
