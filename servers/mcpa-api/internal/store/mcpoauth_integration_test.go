// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package store_test

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"errors"
	"io"
	"log/slog"
	"os"
	"testing"
	"time"

	"github.com/delinoio/apps/servers/mcpa-api/internal/crypto"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

// openTestStore connects to the disposable TEST_DATABASE_URL database,
// resets its schema and applies the migrations.
func openTestStore(t *testing.T) *store.Store {
	t.Helper()
	url := os.Getenv("TEST_DATABASE_URL")
	if url == "" {
		t.Skip("TEST_DATABASE_URL is not set; skipping PostgreSQL integration tests")
	}
	ctx := context.Background()
	s, err := store.Open(ctx, url, 10*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(s.Close)
	if _, err := s.Pool.Exec(ctx, `DROP SCHEMA public CASCADE; CREATE SCHEMA public`); err != nil {
		t.Fatal(err)
	}
	if _, err := s.Migrate(ctx); err != nil {
		t.Fatal(err)
	}
	if err := s.CheckSchema(ctx); err != nil {
		t.Fatal(err)
	}
	return s
}

func digest(v string) []byte {
	sum := sha256.Sum256([]byte(v))
	return sum[:]
}

func testKeys(t *testing.T, ids ...string) *crypto.Keyring {
	t.Helper()
	keys := map[string][]byte{}
	for _, id := range ids {
		k := make([]byte, 32)
		_, _ = rand.Read(k)
		keys[id] = k
	}
	kr, err := crypto.NewKeyring(keys, ids[len(ids)-1])
	if err != nil {
		t.Fatal(err)
	}
	return kr
}

func newGrant(t *testing.T, kr *crypto.Keyring, expire time.Time) store.MCPGrant {
	t.Helper()
	id := store.NewID()
	sealed, err := kr.Seal([]byte(`{"refresh_token":"rt"}`), crypto.MCPGrantAAD(id.String()))
	if err != nil {
		t.Fatal(err)
	}
	return store.MCPGrant{ID: id, ClientID: "client", UserID: "user", Subject: "sub", ExpireTime: expire, Tokens: sealed}
}

func TestMCPOAuthAuthorizationRequests(t *testing.T) {
	s := openTestStore(t)
	m := store.NewMCPOAuth(s)
	ctx := context.Background()
	req := store.MCPAuthRequest{
		StateHash: digest("state"), BrowserHash: digest("browser"), ClientID: "client", RedirectURI: "http://127.0.0.1/cb",
		ClientState: "xyz", CodeChallenge: "challenge", CodeVerifier: "verifier", Nonce: "nonce", ExpireTime: time.Now().Add(time.Minute),
	}
	if err := m.InsertAuthRequest(ctx, req); err != nil {
		t.Fatal(err)
	}
	got, err := m.TakeAuthRequest(ctx, req.StateHash)
	if err != nil || got.ClientState != "xyz" || got.CodeVerifier != "verifier" || got.RedirectURI != req.RedirectURI {
		t.Fatalf("take = %+v, %v", got, err)
	}
	if _, err := m.TakeAuthRequest(ctx, req.StateHash); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("second take: %v", err)
	}
	expired := req
	expired.StateHash = digest("expired")
	expired.ExpireTime = time.Now().Add(-time.Second)
	if err := m.InsertAuthRequest(ctx, expired); err != nil {
		t.Fatal(err)
	}
	if _, err := m.TakeAuthRequest(ctx, expired.StateHash); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("expired take: %v", err)
	}
}

func TestMCPOAuthCodesHandlesAndTokens(t *testing.T) {
	s := openTestStore(t)
	m := store.NewMCPOAuth(s)
	ctx := context.Background()
	kr := testKeys(t, "k1")
	g := newGrant(t, kr, time.Now().Add(time.Hour))
	code := store.MCPCode{CodeHash: digest("code"), ClientID: "client", RedirectURI: "http://127.0.0.1/cb", CodeChallenge: "c", ExpireTime: time.Now().Add(time.Minute)}
	if err := m.InsertGrant(ctx, g, code); err != nil {
		t.Fatal(err)
	}

	// One-time use: the second redemption reports reuse with the grant ID.
	c, err := m.RedeemCode(ctx, code.CodeHash)
	if err != nil || c.GrantID != g.ID || c.CodeChallenge != "c" {
		t.Fatalf("redeem = %+v, %v", c, err)
	}
	c, err = m.RedeemCode(ctx, code.CodeHash)
	if !errors.Is(err, store.ErrCodeReused) || c.GrantID != g.ID {
		t.Fatalf("reuse = %+v, %v", c, err)
	}
	if _, err := m.RedeemCode(ctx, digest("unknown")); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("unknown code: %v", err)
	}

	// Issue an access token and the first handle.
	accessExpire := time.Now().Add(time.Hour)
	if err := m.UpdateGrant(ctx, g.ID, nil, func(got store.MCPGrant) (store.MCPGrantChange, error) {
		if got.UserID != "user" {
			t.Errorf("locked grant = %+v", got)
		}
		return store.MCPGrantChange{NewHandle: digest("h1"), AccessToken: digest("at1"), AccessExpire: accessExpire, ExpireTime: time.Now().Add(2 * time.Hour)}, nil
	}); err != nil {
		t.Fatal(err)
	}
	if got, err := m.GrantByAccessToken(ctx, digest("at1")); err != nil || got.ID != g.ID {
		t.Fatalf("grant by token = %+v, %v", got, err)
	}
	if id, retired, err := m.RefreshHandleGrant(ctx, digest("h1")); err != nil || retired || id != g.ID {
		t.Fatalf("handle = %v %v %v", id, retired, err)
	}

	// Rotation retires h1 and adds h2; reusing h1 fails without calling fn.
	if err := m.UpdateGrant(ctx, g.ID, digest("h1"), func(store.MCPGrant) (store.MCPGrantChange, error) {
		return store.MCPGrantChange{NewHandle: digest("h2")}, nil
	}); err != nil {
		t.Fatal(err)
	}
	if _, retired, _ := m.RefreshHandleGrant(ctx, digest("h1")); !retired {
		t.Fatal("h1 not retired")
	}
	called := false
	err = m.UpdateGrant(ctx, g.ID, digest("h1"), func(store.MCPGrant) (store.MCPGrantChange, error) {
		called = true
		return store.MCPGrantChange{}, nil
	})
	if !errors.Is(err, store.ErrHandleRetired) || called {
		t.Fatalf("reuse = %v, called %v", err, called)
	}

	// A failing fn rolls back unless it deletes.
	boom := errors.New("boom")
	if err := m.UpdateGrant(ctx, g.ID, digest("h2"), func(store.MCPGrant) (store.MCPGrantChange, error) {
		return store.MCPGrantChange{NewHandle: digest("h3")}, boom
	}); !errors.Is(err, boom) {
		t.Fatalf("fn error = %v", err)
	}
	if _, retired, err := m.RefreshHandleGrant(ctx, digest("h2")); err != nil || retired {
		t.Fatalf("rolled back handle = %v %v", retired, err)
	}
	if _, _, err := m.RefreshHandleGrant(ctx, digest("h3")); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("h3 = %v", err)
	}
	if err := m.UpdateGrant(ctx, g.ID, nil, func(store.MCPGrant) (store.MCPGrantChange, error) {
		return store.MCPGrantChange{Delete: true}, boom
	}); !errors.Is(err, boom) {
		t.Fatalf("delete error = %v", err)
	}
	// Deletion cascades to codes, handles and tokens.
	if _, err := m.GetGrant(ctx, g.ID); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("grant after delete: %v", err)
	}
	if _, err := m.GrantByAccessToken(ctx, digest("at1")); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("token after delete: %v", err)
	}
	if _, _, err := m.RefreshHandleGrant(ctx, digest("h2")); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("handle after delete: %v", err)
	}
	if _, err := m.RedeemCode(ctx, code.CodeHash); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("code after delete: %v", err)
	}
}

func TestMCPOAuthExpiry(t *testing.T) {
	s := openTestStore(t)
	m := store.NewMCPOAuth(s)
	ctx := context.Background()
	kr := testKeys(t, "k1")

	live := newGrant(t, kr, time.Now().Add(time.Hour))
	if err := m.InsertGrant(ctx, live, store.MCPCode{CodeHash: digest("expired-code"), ClientID: "client", RedirectURI: "r", CodeChallenge: "c",
		ExpireTime: time.Now().Add(-time.Second)}); err != nil {
		t.Fatal(err)
	}
	if _, err := m.RedeemCode(ctx, digest("expired-code")); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("expired code: %v", err)
	}
	if err := m.UpdateGrant(ctx, live.ID, nil, func(store.MCPGrant) (store.MCPGrantChange, error) {
		return store.MCPGrantChange{AccessToken: digest("old-token"), AccessExpire: time.Now().Add(-time.Second)}, nil
	}); err != nil {
		t.Fatal(err)
	}
	if _, err := m.GrantByAccessToken(ctx, digest("old-token")); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("expired token resolved: %v", err)
	}

	dead := newGrant(t, kr, time.Now().Add(-time.Second))
	if err := m.InsertGrant(ctx, dead, store.MCPCode{CodeHash: digest("dead-code"), ClientID: "client", RedirectURI: "r", CodeChallenge: "c",
		ExpireTime: time.Now().Add(time.Minute)}); err != nil {
		t.Fatal(err)
	}
	if _, err := m.GetGrant(ctx, dead.ID); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("expired grant visible: %v", err)
	}
	if err := m.UpdateGrant(ctx, dead.ID, nil, func(store.MCPGrant) (store.MCPGrantChange, error) {
		return store.MCPGrantChange{}, nil
	}); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("expired grant locked: %v", err)
	}

	n, err := m.DeleteExpired(ctx)
	if err != nil {
		t.Fatal(err)
	}
	// The expired code and token of the live grant, and the dead grant.
	if n != 3 {
		t.Fatalf("deleted %d rows", n)
	}
	if _, err := m.GetGrant(ctx, live.ID); err != nil {
		t.Fatalf("live grant deleted: %v", err)
	}
	var left int
	if err := s.Pool.QueryRow(ctx, `SELECT count(*) FROM mcp_grants`).Scan(&left); err != nil || left != 1 {
		t.Fatalf("grants left = %d, %v", left, err)
	}
}

func TestMasterKeyRotationCoversMCPGrants(t *testing.T) {
	s := openTestStore(t)
	m := store.NewMCPOAuth(s)
	ctx := context.Background()
	oldKey, newKey := make([]byte, 32), make([]byte, 32)
	_, _ = rand.Read(oldKey)
	_, _ = rand.Read(newKey)
	old, err := crypto.NewKeyring(map[string][]byte{"old": oldKey}, "old")
	if err != nil {
		t.Fatal(err)
	}
	g := newGrant(t, old, time.Now().Add(time.Hour))
	if err := m.InsertGrant(ctx, g, store.MCPCode{CodeHash: digest("c"), ClientID: "client", RedirectURI: "r", CodeChallenge: "c",
		ExpireTime: time.Now().Add(time.Minute)}); err != nil {
		t.Fatal(err)
	}
	both, err := crypto.NewKeyring(map[string][]byte{"old": oldKey, "new": newKey}, "new")
	if err != nil {
		t.Fatal(err)
	}
	report, err := store.RotateMasterKey(ctx, s, both, 10, slog.New(slog.NewTextHandler(io.Discard, nil)))
	if err != nil {
		t.Fatal(err)
	}
	if report.Rewrapped["mcp_grants"] != 1 || len(report.Remaining) != 0 {
		t.Fatalf("report = %+v", report)
	}
	got, err := m.GetGrant(ctx, g.ID)
	if err != nil || got.Tokens.KeyID != "new" {
		t.Fatalf("grant = %+v, %v", got, err)
	}
	plain, err := both.Open(got.Tokens, crypto.MCPGrantAAD(g.ID.String()))
	if err != nil || string(plain) != `{"refresh_token":"rt"}` {
		t.Fatalf("open = %q, %v", plain, err)
	}
}
