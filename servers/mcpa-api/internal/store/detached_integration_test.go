// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package store_test

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"

	"github.com/delinoio/apps/servers/mcpa-api/internal/crypto"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

// A request that ends while fn works after the lock must not roll back what
// fn changed: the rotated upstream refresh token it consumed is gone.
func TestMCPOAuthUpdateGrantCommitsAfterRequestCancellation(t *testing.T) {
	s := openTestStore(t)
	m := store.NewMCPOAuth(s)
	kr := testKeys(t, "k1")
	g := newGrant(t, kr, time.Now().Add(time.Hour))
	code := store.MCPCode{CodeHash: digest("code"), ClientID: "client", RedirectURI: "http://127.0.0.1/cb", CodeChallenge: "c", ExpireTime: time.Now().Add(time.Minute)}
	if err := m.InsertGrant(context.Background(), g, code); err != nil {
		t.Fatal(err)
	}
	rotated, err := kr.Seal([]byte(`{"refresh_token":"rt-rotated"}`), crypto.MCPGrantAAD(g.ID.String()))
	if err != nil {
		t.Fatal(err)
	}
	boom := errors.New("boom")
	for _, tc := range []struct {
		name  string
		fnErr error
	}{{"issuance succeeds", nil}, {"later step fails", boom}} {
		t.Run(tc.name, func(t *testing.T) {
			reqCtx, cancel := context.WithCancel(context.Background())
			defer cancel()
			err := m.UpdateGrant(reqCtx, g.ID, store.MCPGrantUse{}, func(ctx context.Context, _ store.MCPGrant) (store.MCPGrantChange, error) {
				cancel() // the request ends after the upstream rotation
				if ctx.Err() != nil {
					t.Error("the context passed to fn ended with the request")
				}
				return store.MCPGrantChange{Tokens: &rotated}, tc.fnErr
			})
			if !errors.Is(err, tc.fnErr) {
				t.Fatalf("UpdateGrant returned %v, want %v", err, tc.fnErr)
			}
			got, err := m.GetGrant(context.Background(), g.ID)
			if err != nil || got.Tokens.Ciphertext == nil || string(got.Tokens.Ciphertext) != string(rotated.Ciphertext) {
				t.Fatalf("stored grant tokens were not replaced: %v", err)
			}
		})
	}
}

// The wait for the row lock stays under the request context: a canceled
// request must not start fn.
func TestMCPOAuthUpdateGrantLockWaitStopsWithRequest(t *testing.T) {
	s := openTestStore(t)
	m := store.NewMCPOAuth(s)
	kr := testKeys(t, "k1")
	g := newGrant(t, kr, time.Now().Add(time.Hour))
	if err := m.InsertGrant(context.Background(), g, store.MCPCode{CodeHash: digest("c2"), ClientID: "client", RedirectURI: "http://127.0.0.1/cb", CodeChallenge: "c", ExpireTime: time.Now().Add(time.Minute)}); err != nil {
		t.Fatal(err)
	}
	ctx := context.Background()
	tx, err := s.Pool.Begin(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = tx.Rollback(ctx) }()
	if _, err := tx.Exec(ctx, `SELECT 1 FROM mcp_grants WHERE id = $1 FOR UPDATE`, g.ID); err != nil {
		t.Fatal(err)
	}
	reqCtx, cancel := context.WithTimeout(ctx, 200*time.Millisecond)
	defer cancel()
	called := false
	err = m.UpdateGrant(reqCtx, g.ID, store.MCPGrantUse{}, func(context.Context, store.MCPGrant) (store.MCPGrantChange, error) {
		called = true
		return store.MCPGrantChange{}, nil
	})
	if err == nil || called {
		t.Fatalf("UpdateGrant = %v, fn called = %v, want a lock wait failure without calling fn", err, called)
	}
}

// InTxDetached bounds detached work and rolls back on error.
func TestInTxDetachedRollsBackOnError(t *testing.T) {
	s := openTestStore(t)
	ctx := context.Background()
	if _, err := s.Pool.Exec(ctx, `CREATE TABLE detached_probe (n int)`); err != nil {
		t.Fatal(err)
	}
	boom := errors.New("boom")
	err := s.InTxDetached(ctx, func(tx pgx.Tx, detach func() context.Context) error {
		if _, err := tx.Exec(detach(), `INSERT INTO detached_probe VALUES (1)`); err != nil {
			return err
		}
		return boom
	})
	if !errors.Is(err, boom) {
		t.Fatalf("InTxDetached = %v, want boom", err)
	}
	var n int
	if err := s.Pool.QueryRow(ctx, `SELECT count(*) FROM detached_probe`).Scan(&n); err != nil || n != 0 {
		t.Fatalf("rows after a failed transaction: %d, %v", n, err)
	}
	err = s.InTxDetached(ctx, func(tx pgx.Tx, detach func() context.Context) error {
		d := detach()
		if dl, ok := d.Deadline(); !ok || time.Until(dl) > 41*time.Second {
			t.Errorf("detached context deadline %v ok=%v, want a bounded deadline", dl, ok)
		}
		_, err := tx.Exec(d, `INSERT INTO detached_probe VALUES (2)`)
		return err
	})
	if err != nil {
		t.Fatal(err)
	}
	if err := s.Pool.QueryRow(ctx, `SELECT count(*) FROM detached_probe`).Scan(&n); err != nil || n != 1 {
		t.Fatalf("rows after a committed transaction: %d, %v", n, err)
	}
}
