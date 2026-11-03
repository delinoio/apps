// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package auth_test

import (
	"bytes"
	"context"
	"crypto/rand"
	"log/slog"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth/authtest"
	"github.com/delinoio/apps/servers/mcpa-api/internal/crypto"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store/storetest"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
)

// cancelHarness is a session manager against a fake Logto that, like Logto,
// rejects a rotated-out refresh token.
type cancelHarness struct {
	sessions *auth.Sessions
	provider *authtest.Provider
	db       *store.Store
	logs     *bytes.Buffer

	mu         sync.Mutex
	current    string
	delcoreUp  bool
	apiCalls   atomic.Int32
	afterRotor func()
}

func newCancelHarness(t *testing.T) (*cancelHarness, auth.SessionState, context.Context) {
	t.Helper()
	h := &cancelHarness{provider: authtest.New(t), db: storetest.Open(t), logs: &bytes.Buffer{}, current: "rt-0", delcoreUp: true}
	key := make([]byte, 32)
	_, _ = rand.Read(key)
	keys, err := crypto.NewKeyring(map[string][]byte{"k1": key}, "k1")
	if err != nil {
		t.Fatal(err)
	}
	logto := auth.NewLogto(h.provider.Issuer(), "console-client", "secret", h.provider.Server.Client())
	h.sessions = auth.NewSessions(auth.SessionsConfig{
		Store: h.db, Keys: keys, Logto: logto, APIResource: apiAudience, DelcoreResource: delcoreAudience,
		MFAClaim: "mcpa_mfa", Log: slog.New(slog.NewTextHandler(h.logs, nil)),
	})
	h.provider.SetTokenHandler(func(form url.Values) (int, any) {
		h.mu.Lock()
		defer h.mu.Unlock()
		if form.Get("grant_type") != "refresh_token" || form.Get("refresh_token") != h.current {
			return http.StatusBadRequest, map[string]string{"error": "invalid_grant"}
		}
		switch form.Get("resource") {
		case apiAudience:
			h.apiCalls.Add(1)
			h.current += "'"
			resp := map[string]any{
				"access_token":  h.provider.Sign(t, map[string]any{"sub": "sub-1", "aud": apiAudience, "mcpa_mfa": true}),
				"token_type":    "Bearer",
				"expires_in":    3600,
				"refresh_token": h.current,
			}
			if h.afterRotor != nil {
				// Logto rotated the token; the client request ends before the
				// response reaches the server.
				h.afterRotor()
			}
			return http.StatusOK, resp
		case delcoreAudience:
			if !h.delcoreUp {
				return http.StatusServiceUnavailable, map[string]string{"error": "temporarily_unavailable"}
			}
			return http.StatusOK, map[string]any{"access_token": "dc-1", "token_type": "Bearer", "expires_in": 3600}
		}
		return http.StatusBadRequest, map[string]string{"error": "invalid_target"}
	})
	expiry := time.Now().Add(time.Hour)
	_, sess, err := h.sessions.Create(context.Background(), auth.NewSessionInput{
		UserID: "user-1", Subject: "sub-1", MFA: true, AccessExpiry: expiry,
		Tokens: auth.SessionTokens{APIAccessToken: "api-0", DelcoreAccessToken: "dc-0", DelcoreExpiry: expiry, RefreshToken: "rt-0"},
	})
	if err != nil {
		t.Fatal(err)
	}
	return h, auth.SessionState{Session: sess}, context.Background()
}

// A request canceled after Logto rotated the refresh token must still store
// the replacement, so the next refresh succeeds and the session survives.
func TestRefreshStoresRotatedTokenAfterRequestCancellation(t *testing.T) {
	for _, tc := range []struct {
		name      string
		delcoreUp bool
	}{
		{"refresh completes", true},
		{"later step fails", false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			h, st, ctx := newCancelHarness(t)
			h.delcoreUp = tc.delcoreUp
			reqCtx, cancel := context.WithCancel(ctx)
			defer cancel()
			h.afterRotor = func() {
				cancel()
				time.Sleep(100 * time.Millisecond) // let the cancellation reach the client first
			}

			_, err := h.sessions.Refresh(auth.ForceRefresh(reqCtx), st.Session.ID)
			if tc.delcoreUp && err != nil {
				t.Fatalf("canceled refresh returned %v, want the detached refresh to complete", err)
			}
			if !tc.delcoreUp && apperr.As(err).Reason != mcpav1.ErrorReason_ERROR_REASON_DEPENDENCY_UNAVAILABLE {
				t.Fatalf("canceled refresh with a failing Delcore exchange returned %v, want dependency unavailable", err)
			}
			if h.apiCalls.Load() != 1 {
				t.Fatalf("API exchanges %d, want 1", h.apiCalls.Load())
			}
			logs := h.logs.String()
			if !strings.Contains(logs, "request_cancelled=true") || strings.Contains(logs, "rt-0") {
				t.Fatalf("log lacks request_cancelled=true or leaks token material:\n%s", logs)
			}

			h.mu.Lock()
			h.delcoreUp = true
			h.afterRotor = nil
			h.mu.Unlock()
			got, err := h.sessions.Refresh(auth.ForceRefresh(ctx), st.Session.ID)
			if err != nil {
				t.Fatalf("refresh after the cancellation returned %v, want success with the stored rotated token", err)
			}
			h.mu.Lock()
			want := h.current
			h.mu.Unlock()
			if got.Tokens.RefreshToken != want {
				t.Fatalf("stored refresh token %q, want %q", got.Tokens.RefreshToken, want)
			}
		})
	}
}

// A request that is already canceled while it waits for the session lock
// must not call Logto: the lock wait stays under the request context.
func TestRefreshWaitingForLockStopsWithRequest(t *testing.T) {
	h, st, ctx := newCancelHarness(t)
	tx, err := h.db.Pool.Begin(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = tx.Rollback(ctx) }()
	if _, err := tx.Exec(ctx, `SELECT 1 FROM console_sessions WHERE id = $1 FOR UPDATE`, st.Session.ID); err != nil {
		t.Fatal(err)
	}
	reqCtx, cancel := context.WithTimeout(ctx, 200*time.Millisecond)
	defer cancel()
	if _, err := h.sessions.Refresh(auth.ForceRefresh(reqCtx), st.Session.ID); err == nil {
		t.Fatal("refresh succeeded while another transaction held the session lock")
	}
	if h.apiCalls.Load() != 0 {
		t.Fatalf("Logto was called %d times by a request that never got the lock", h.apiCalls.Load())
	}
}
