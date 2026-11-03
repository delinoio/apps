// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package store_test

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"os"
	"sync"
	"sync/atomic"
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
	s, err := store.Open(ctx, url, 10*time.Second, store.ModeServe)
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

	// Lookup does not consume the code.
	for range 2 {
		c, err := m.LookupCode(ctx, code.CodeHash)
		if err != nil || c.GrantID != g.ID || c.CodeChallenge != "c" || c.ClientID != "client" {
			t.Fatalf("lookup = %+v, %v", c, err)
		}
	}
	if _, err := m.LookupCode(ctx, digest("unknown")); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("unknown code: %v", err)
	}
	codeUse := store.MCPGrantUse{Code: code.CodeHash}
	// A failed issuance leaves the code unused, even when it keeps tokens.
	boom := errors.New("boom")
	kept, err := kr.Seal([]byte(`{"refresh_token":"rt-kept"}`), crypto.MCPGrantAAD(g.ID.String()))
	if err != nil {
		t.Fatal(err)
	}
	if err := m.UpdateGrant(ctx, g.ID, codeUse, func(_ context.Context, _ store.MCPGrant) (store.MCPGrantChange, error) {
		return store.MCPGrantChange{Tokens: &kept, NewHandle: digest("h0"), AccessToken: digest("at0"), AccessExpire: time.Now().Add(time.Hour)}, boom
	}); !errors.Is(err, boom) {
		t.Fatalf("failed issuance = %v", err)
	}
	if _, err := m.LookupCode(ctx, code.CodeHash); err != nil {
		t.Fatalf("code after failed issuance: %v", err)
	}
	if _, _, err := m.RefreshHandleGrant(ctx, digest("h0")); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("h0 after failed issuance = %v", err)
	}
	if _, err := m.GrantByAccessToken(ctx, digest("at0")); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("at0 after failed issuance = %v", err)
	}

	// Issue an access token and the first handle; this consumes the code.
	accessExpire := time.Now().Add(time.Hour)
	if err := m.UpdateGrant(ctx, g.ID, codeUse, func(_ context.Context, got store.MCPGrant) (store.MCPGrantChange, error) {
		if got.UserID != "user" {
			t.Errorf("locked grant = %+v", got)
		}
		return store.MCPGrantChange{NewHandle: digest("h1"), AccessToken: digest("at1"), AccessExpire: accessExpire, ExpireTime: time.Now().Add(2 * time.Hour)}, nil
	}); err != nil {
		t.Fatal(err)
	}
	// One-time use: a used code reports reuse with the grant ID and cannot
	// issue again.
	c, err := m.LookupCode(ctx, code.CodeHash)
	if !errors.Is(err, store.ErrCodeReused) || c.GrantID != g.ID {
		t.Fatalf("reuse = %+v, %v", c, err)
	}
	called := false
	if err := m.UpdateGrant(ctx, g.ID, codeUse, func(_ context.Context, _ store.MCPGrant) (store.MCPGrantChange, error) {
		called = true
		return store.MCPGrantChange{}, nil
	}); !errors.Is(err, store.ErrCodeReused) || called {
		t.Fatalf("issue with used code = %v, called %v", err, called)
	}
	if got, err := m.GrantByAccessToken(ctx, digest("at1")); err != nil || got.ID != g.ID {
		t.Fatalf("grant by token = %+v, %v", got, err)
	}
	if id, retired, err := m.RefreshHandleGrant(ctx, digest("h1")); err != nil || retired || id != g.ID {
		t.Fatalf("handle = %v %v %v", id, retired, err)
	}

	// Rotation retires h1 and adds h2; reusing h1 fails without calling fn.
	if err := m.UpdateGrant(ctx, g.ID, store.MCPGrantUse{Handle: digest("h1")}, func(_ context.Context, _ store.MCPGrant) (store.MCPGrantChange, error) {
		return store.MCPGrantChange{NewHandle: digest("h2")}, nil
	}); err != nil {
		t.Fatal(err)
	}
	if _, retired, _ := m.RefreshHandleGrant(ctx, digest("h1")); !retired {
		t.Fatal("h1 not retired")
	}
	called = false
	err = m.UpdateGrant(ctx, g.ID, store.MCPGrantUse{Handle: digest("h1")}, func(_ context.Context, _ store.MCPGrant) (store.MCPGrantChange, error) {
		called = true
		return store.MCPGrantChange{}, nil
	})
	if !errors.Is(err, store.ErrHandleRetired) || called {
		t.Fatalf("reuse = %v, called %v", err, called)
	}

	// A failing fn rolls back unless it deletes.
	if err := m.UpdateGrant(ctx, g.ID, store.MCPGrantUse{Handle: digest("h2")}, func(_ context.Context, _ store.MCPGrant) (store.MCPGrantChange, error) {
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
	// A failing fn keeps replaced tokens only: the handle stays active and no
	// handle, access token or expiry change is applied.
	before, err := m.GetGrant(ctx, g.ID)
	if err != nil {
		t.Fatal(err)
	}
	replaced, err := kr.Seal([]byte(`{"refresh_token":"rt-rotated"}`), crypto.MCPGrantAAD(g.ID.String()))
	if err != nil {
		t.Fatal(err)
	}
	if err := m.UpdateGrant(ctx, g.ID, store.MCPGrantUse{Handle: digest("h2")}, func(_ context.Context, _ store.MCPGrant) (store.MCPGrantChange, error) {
		return store.MCPGrantChange{
			Tokens: &replaced, NewHandle: digest("h3"), AccessToken: digest("at3"), AccessExpire: accessExpire,
			ExpireTime: time.Now().Add(48 * time.Hour),
		}, boom
	}); !errors.Is(err, boom) {
		t.Fatalf("fn error with tokens = %v", err)
	}
	after, err := m.GetGrant(ctx, g.ID)
	if err != nil {
		t.Fatal(err)
	}
	if plain, err := kr.Open(after.Tokens, crypto.MCPGrantAAD(g.ID.String())); err != nil || string(plain) != `{"refresh_token":"rt-rotated"}` {
		t.Fatalf("kept tokens = %q, %v", plain, err)
	}
	if !after.ExpireTime.Equal(before.ExpireTime) {
		t.Fatalf("expiry changed from %v to %v", before.ExpireTime, after.ExpireTime)
	}
	if _, retired, err := m.RefreshHandleGrant(ctx, digest("h2")); err != nil || retired {
		t.Fatalf("handle after kept tokens = %v %v", retired, err)
	}
	if _, _, err := m.RefreshHandleGrant(ctx, digest("h3")); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("h3 after kept tokens = %v", err)
	}
	if _, err := m.GrantByAccessToken(ctx, digest("at3")); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("at3 after kept tokens = %v", err)
	}
	if err := m.UpdateGrant(ctx, g.ID, store.MCPGrantUse{}, func(_ context.Context, _ store.MCPGrant) (store.MCPGrantChange, error) {
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
	if _, err := m.LookupCode(ctx, code.CodeHash); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("code after delete: %v", err)
	}
}

// Concurrent issuances with one code serialize on the grant lock: exactly one
// issues and consumes the code, and the others see ErrCodeReused without
// running fn.
func TestMCPOAuthConcurrentCodeUse(t *testing.T) {
	s := openTestStore(t)
	m := store.NewMCPOAuth(s)
	ctx := context.Background()
	kr := testKeys(t, "k1")
	g := newGrant(t, kr, time.Now().Add(time.Hour))
	code := store.MCPCode{CodeHash: digest("code"), ClientID: "client", RedirectURI: "r", CodeChallenge: "c", ExpireTime: time.Now().Add(time.Minute)}
	if err := m.InsertGrant(ctx, g, code); err != nil {
		t.Fatal(err)
	}
	const n = 4
	var calls atomic.Int32
	errs := make([]error, n)
	var wg sync.WaitGroup
	for i := range n {
		wg.Add(1)
		go func() {
			defer wg.Done()
			errs[i] = m.UpdateGrant(ctx, g.ID, store.MCPGrantUse{Code: code.CodeHash}, func(_ context.Context, _ store.MCPGrant) (store.MCPGrantChange, error) {
				calls.Add(1)
				// Hold the lock like an upstream refresh does.
				time.Sleep(50 * time.Millisecond)
				return store.MCPGrantChange{AccessToken: digest(fmt.Sprintf("at-%d", i)), AccessExpire: time.Now().Add(time.Hour)}, nil
			})
		}()
	}
	wg.Wait()
	issued := 0
	for _, err := range errs {
		switch {
		case err == nil:
			issued++
		case !errors.Is(err, store.ErrCodeReused):
			t.Fatalf("concurrent use: %v", err)
		}
	}
	if issued != 1 || calls.Load() != 1 {
		t.Fatalf("issued %d times, fn called %d times", issued, calls.Load())
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
	if _, err := m.LookupCode(ctx, digest("expired-code")); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("expired code: %v", err)
	}
	if err := m.UpdateGrant(ctx, live.ID, store.MCPGrantUse{Code: digest("expired-code")}, func(_ context.Context, _ store.MCPGrant) (store.MCPGrantChange, error) {
		t.Error("fn called for an expired code")
		return store.MCPGrantChange{}, nil
	}); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("issue with expired code: %v", err)
	}
	if err := m.UpdateGrant(ctx, live.ID, store.MCPGrantUse{}, func(_ context.Context, _ store.MCPGrant) (store.MCPGrantChange, error) {
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
	if err := m.UpdateGrant(ctx, dead.ID, store.MCPGrantUse{}, func(_ context.Context, _ store.MCPGrant) (store.MCPGrantChange, error) {
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

// Database failures of UpdateGrant wrap ErrStorage so the broker answers
// temporarily_unavailable; results that come from the caller or from the
// stored state never do.
func TestMCPOAuthUpdateGrantClassifiesStorageFailures(t *testing.T) {
	s := openTestStore(t)
	m := store.NewMCPOAuth(s)
	ctx := context.Background()
	kr := testKeys(t, "k1")
	g := newGrant(t, kr, time.Now().Add(time.Hour))
	code := store.MCPCode{CodeHash: digest("code"), ClientID: "client", RedirectURI: "r", CodeChallenge: "c", ExpireTime: time.Now().Add(time.Minute)}
	if err := m.InsertGrant(ctx, g, code); err != nil {
		t.Fatal(err)
	}
	codeUse := store.MCPGrantUse{Code: code.CodeHash}
	noop := func(_ context.Context, _ store.MCPGrant) (store.MCPGrantChange, error) {
		return store.MCPGrantChange{}, nil
	}

	// A cancelled context fails when the transaction begins.
	cancelled, cancel := context.WithCancel(ctx)
	cancel()
	called := false
	err := m.UpdateGrant(cancelled, g.ID, codeUse, func(_ context.Context, _ store.MCPGrant) (store.MCPGrantChange, error) {
		called = true
		return store.MCPGrantChange{}, nil
	})
	if !errors.Is(err, store.ErrStorage) || !errors.Is(err, context.Canceled) || called {
		t.Fatalf("cancelled context = %v, fn called %v", err, called)
	}

	// An unreachable database (closed pool) fails the same way.
	closed, err := store.Open(ctx, os.Getenv("TEST_DATABASE_URL"), 10*time.Second, store.ModeServe)
	if err != nil {
		t.Fatal(err)
	}
	closed.Close()
	if err := store.NewMCPOAuth(closed).UpdateGrant(ctx, g.ID, codeUse, noop); !errors.Is(err, store.ErrStorage) {
		t.Fatalf("closed pool = %v", err)
	}

	// A failing statement inside the transaction: the change inserts a handle
	// that already exists. Nothing is consumed, so the code still works.
	if err := m.UpdateGrant(ctx, g.ID, store.MCPGrantUse{}, func(_ context.Context, _ store.MCPGrant) (store.MCPGrantChange, error) {
		return store.MCPGrantChange{NewHandle: digest("dup")}, nil
	}); err != nil {
		t.Fatal(err)
	}
	err = m.UpdateGrant(ctx, g.ID, codeUse, func(_ context.Context, _ store.MCPGrant) (store.MCPGrantChange, error) {
		return store.MCPGrantChange{NewHandle: digest("dup")}, nil
	})
	if !errors.Is(err, store.ErrStorage) {
		t.Fatalf("duplicate handle = %v", err)
	}
	if _, err := m.LookupCode(ctx, code.CodeHash); err != nil {
		t.Fatalf("code consumed by a failed statement: %v", err)
	}

	// Errors of fn and sentinel results are not storage failures.
	boom := errors.New("boom")
	if err := m.UpdateGrant(ctx, g.ID, codeUse, func(_ context.Context, _ store.MCPGrant) (store.MCPGrantChange, error) {
		return store.MCPGrantChange{}, boom
	}); !errors.Is(err, boom) || errors.Is(err, store.ErrStorage) {
		t.Fatalf("fn error = %v", err)
	}
	if err := m.UpdateGrant(ctx, store.NewID(), codeUse, noop); !errors.Is(err, store.ErrNotFound) || errors.Is(err, store.ErrStorage) {
		t.Fatalf("unknown grant = %v", err)
	}
	if err := m.UpdateGrant(ctx, g.ID, store.MCPGrantUse{Handle: digest("missing")}, noop); !errors.Is(err, store.ErrHandleRetired) || errors.Is(err, store.ErrStorage) {
		t.Fatalf("missing handle = %v", err)
	}
	// A kept change that fails to apply is a storage failure, not fn's error.
	kept, err := kr.Seal([]byte(`{"refresh_token":"rt-2"}`), crypto.MCPGrantAAD(g.ID.String()))
	if err != nil {
		t.Fatal(err)
	}
	err = m.UpdateGrant(ctx, g.ID, store.MCPGrantUse{}, func(_ context.Context, _ store.MCPGrant) (store.MCPGrantChange, error) {
		return store.MCPGrantChange{Tokens: &kept}, boom
	})
	if !errors.Is(err, boom) || errors.Is(err, store.ErrStorage) {
		t.Fatalf("kept change = %v", err)
	}
}
