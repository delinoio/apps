// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package auth_test

import (
	"context"
	"crypto/rand"
	"io"
	"log/slog"
	"net/http"
	"net/url"
	"sync"
	"testing"
	"time"

	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth/authtest"
	"github.com/delinoio/apps/servers/mcpa-api/internal/crypto"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store/storetest"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
)

const delcoreAudience = "https://delcore.example.test"

// TestRefreshKeepsRotatedRefreshToken covers a refresh in which Logto
// rotates the refresh token on the API-resource exchange and the following
// Delcore-resource exchange fails. Like Logto, the fake provider rejects a
// rotated-out refresh token, so the retry after recovery succeeds only when
// the failed refresh stored the replacement token.
func TestRefreshKeepsRotatedRefreshToken(t *testing.T) {
	provider := authtest.New(t)
	db := storetest.Open(t)
	key := make([]byte, 32)
	_, _ = rand.Read(key)
	keys, err := crypto.NewKeyring(map[string][]byte{"k1": key}, "k1")
	if err != nil {
		t.Fatal(err)
	}
	logto := auth.NewLogto(provider.Issuer(), "console-client", "secret", provider.Server.Client())
	sessions := auth.NewSessions(auth.SessionsConfig{
		Store: db, Keys: keys, Logto: logto, APIResource: apiAudience, DelcoreResource: delcoreAudience,
		MFAClaim: "mcpa_mfa", Log: slog.New(slog.NewTextHandler(io.Discard, nil)),
	})
	ctx := context.Background()

	// The access token expires now, inside the refresh margin, so Lookup
	// refreshes the session.
	cookie, _, err := sessions.Create(ctx, auth.NewSessionInput{
		UserID: "user-1", Subject: "sub-1", MFA: true, AccessExpiry: time.Now(),
		Tokens: auth.SessionTokens{APIAccessToken: "api-0", DelcoreAccessToken: "dc-0", DelcoreExpiry: time.Now(), RefreshToken: "rt-0"},
	})
	if err != nil {
		t.Fatal(err)
	}

	var mu sync.Mutex
	current := "rt-0"
	delcoreUp := false
	provider.SetTokenHandler(func(form url.Values) (int, any) {
		mu.Lock()
		defer mu.Unlock()
		if form.Get("grant_type") != "refresh_token" || form.Get("refresh_token") != current {
			return http.StatusBadRequest, map[string]string{"error": "invalid_grant"}
		}
		switch form.Get("resource") {
		case apiAudience:
			current += "'"
			return http.StatusOK, map[string]any{
				"access_token":  provider.Sign(t, map[string]any{"sub": "sub-1", "aud": apiAudience, "mcpa_mfa": true}),
				"token_type":    "Bearer",
				"expires_in":    3600,
				"refresh_token": current,
			}
		case delcoreAudience:
			if !delcoreUp {
				return http.StatusServiceUnavailable, map[string]string{"error": "temporarily_unavailable"}
			}
			return http.StatusOK, map[string]any{"access_token": "dc-1", "token_type": "Bearer", "expires_in": 3600}
		}
		return http.StatusBadRequest, map[string]string{"error": "invalid_target"}
	})

	if _, err := sessions.Lookup(ctx, cookie); apperr.As(err).Reason != mcpav1.ErrorReason_ERROR_REASON_DEPENDENCY_UNAVAILABLE {
		t.Fatalf("lookup while the Delcore exchange fails returned %v, want dependency unavailable", err)
	}

	mu.Lock()
	delcoreUp = true
	mu.Unlock()
	st, err := sessions.Lookup(ctx, cookie)
	if err != nil {
		t.Fatalf("lookup after the Delcore exchange recovered returned %v, want a refreshed session", err)
	}
	mu.Lock()
	want := current
	mu.Unlock()
	if !st.Session.MFA || st.Tokens.DelcoreAccessToken != "dc-1" || st.Tokens.RefreshToken != want {
		t.Fatalf("refreshed session MFA=%v delcore=%q refresh=%q, want MFA, dc-1 and %q", st.Session.MFA, st.Tokens.DelcoreAccessToken, st.Tokens.RefreshToken, want)
	}
}
