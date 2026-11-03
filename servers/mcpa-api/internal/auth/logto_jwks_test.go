// SPDX-License-Identifier: FSL-1.1-ALv2

package auth_test

import (
	"context"
	"errors"
	"sync"
	"testing"
	"time"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth/authtest"
)

const logtoClientID = "console-client"

func newLogto(t *testing.T) (*auth.Logto, *authtest.Provider) {
	t.Helper()
	p := authtest.New(t)
	return auth.NewLogto(p.Issuer(), logtoClientID, "secret", p.Server.Client()), p
}

func accessClaims() map[string]any {
	return map[string]any{"sub": "logto-1", "aud": mcpAudience, "mcpa_mfa": true}
}

func idClaims(nonce string) map[string]any {
	return map[string]any{"sub": "logto-1", "aud": logtoClientID, "nonce": nonce}
}

func TestLogtoCachesSigningKeys(t *testing.T) {
	l, p := newLogto(t)
	ctx := context.Background()
	for range 3 {
		if _, err := l.VerifyAccessToken(ctx, p.Sign(t, accessClaims()), mcpAudience); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := l.VerifyIDToken(ctx, p.Sign(t, idClaims("n1")), "n1"); err != nil {
		t.Fatal(err)
	}
	if n := p.JWKSRequests(); n != 1 {
		t.Fatalf("JWKS fetched %d times for 4 verifications, want 1", n)
	}
}

func TestLogtoConcurrentColdVerificationsShareOneFetch(t *testing.T) {
	l, p := newLogto(t)
	tok := p.Sign(t, accessClaims())
	var wg sync.WaitGroup
	for range 8 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if _, err := l.VerifyAccessToken(context.Background(), tok, mcpAudience); err != nil {
				t.Error(err)
			}
		}()
	}
	wg.Wait()
	if n := p.JWKSRequests(); n != 1 {
		t.Fatalf("JWKS fetched %d times, want 1", n)
	}
}

func TestLogtoRefetchesOnlyForUnknownKeyID(t *testing.T) {
	l, p := newLogto(t)
	ctx := context.Background()
	if _, err := l.VerifyAccessToken(ctx, p.Sign(t, accessClaims()), mcpAudience); err != nil {
		t.Fatal(err)
	}

	// A wrong signature under a cached key ID is a token failure and must not
	// reach Logto.
	_, err := l.VerifyAccessToken(ctx, p.SignForged(t, "k1", accessClaims()), mcpAudience)
	if err == nil || errors.Is(err, auth.ErrLogtoUnavailable) {
		t.Fatalf("forged signature: %v", err)
	}
	if n := p.JWKSRequests(); n != 1 {
		t.Fatalf("bad signature caused a refetch: %d requests", n)
	}

	// Rotation: the first token with the new key ID triggers exactly one
	// refetch; later tokens use the cache.
	p.RotateKey(t)
	for range 3 {
		if _, err := l.VerifyAccessToken(ctx, p.Sign(t, accessClaims()), mcpAudience); err != nil {
			t.Fatalf("rotated key: %v", err)
		}
	}
	if n := p.JWKSRequests(); n != 2 {
		t.Fatalf("rotation caused %d JWKS requests, want 2", n)
	}

	// An unknown key ID forces a refetch and still fails as a token failure.
	_, err = l.VerifyAccessToken(ctx, p.SignForged(t, "unknown", accessClaims()), mcpAudience)
	if err == nil || errors.Is(err, auth.ErrLogtoUnavailable) {
		t.Fatalf("unknown key ID: %v", err)
	}
	if n := p.JWKSRequests(); n != 3 {
		t.Fatalf("unknown key ID caused %d JWKS requests, want 3", n)
	}
}

func TestLogtoJWKSOutageIsUnavailable(t *testing.T) {
	l, p := newLogto(t)
	ctx := context.Background()
	p.SetJWKSStatus(503)
	if _, err := l.VerifyAccessToken(ctx, p.Sign(t, accessClaims()), mcpAudience); !errors.Is(err, auth.ErrLogtoUnavailable) {
		t.Fatalf("access token during outage: %v", err)
	}
	if _, err := l.VerifyIDToken(ctx, p.Sign(t, idClaims("n")), "n"); !errors.Is(err, auth.ErrLogtoUnavailable) {
		t.Fatalf("ID token during outage: %v", err)
	}

	// The failure is not cached: the next verification fetches again.
	p.SetJWKSStatus(0)
	if _, err := l.VerifyAccessToken(ctx, p.Sign(t, accessClaims()), mcpAudience); err != nil {
		t.Fatalf("after recovery: %v", err)
	}

	// With a warm cache a known key keeps working through an outage, while a
	// rotated key cannot be fetched.
	old := p.Sign(t, accessClaims())
	p.RotateKey(t)
	p.SetJWKSStatus(503)
	if _, err := l.VerifyAccessToken(ctx, old, mcpAudience); err != nil {
		t.Fatalf("cached key during outage: %v", err)
	}
	if _, err := l.VerifyAccessToken(ctx, p.Sign(t, accessClaims()), mcpAudience); !errors.Is(err, auth.ErrLogtoUnavailable) {
		t.Fatalf("rotated key during outage: %v", err)
	}
}

func TestLogtoJWKSTimeoutIsUnavailable(t *testing.T) {
	l, p := newLogto(t)
	p.SetJWKSHang(true)
	t.Cleanup(func() { p.SetJWKSHang(false) })
	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()
	if _, err := l.VerifyAccessToken(ctx, p.Sign(t, accessClaims()), mcpAudience); !errors.Is(err, auth.ErrLogtoUnavailable) {
		t.Fatalf("hanging JWKS: %v", err)
	}
}

func TestLogtoTokenFailuresAreNotUnavailable(t *testing.T) {
	l, p := newLogto(t)
	ctx := context.Background()
	// Warm the cache so that no case below can be an outage.
	if _, err := l.VerifyAccessToken(ctx, p.Sign(t, accessClaims()), mcpAudience); err != nil {
		t.Fatal(err)
	}
	access := map[string]string{
		"bad signature": p.SignForged(t, "k1", accessClaims()),
		"expired":       p.Sign(t, map[string]any{"sub": "logto-1", "aud": mcpAudience, "exp": time.Now().Add(-time.Hour).Unix()}),
		"issuer":        p.Sign(t, map[string]any{"iss": "https://evil.test", "sub": "logto-1", "aud": mcpAudience}),
		"audience":      p.Sign(t, map[string]any{"sub": "logto-1", "aud": apiAudience}),
		"malformed":     "a.b.c",
	}
	for name, tok := range access {
		if _, err := l.VerifyAccessToken(ctx, tok, mcpAudience); err == nil || errors.Is(err, auth.ErrLogtoUnavailable) {
			t.Errorf("access token %s: %v", name, err)
		}
	}
	id := map[string]string{
		"nonce":         p.Sign(t, idClaims("other")),
		"client":        p.Sign(t, map[string]any{"sub": "logto-1", "aud": "another-client", "nonce": "n"}),
		"bad signature": p.SignForged(t, "k1", idClaims("n")),
	}
	for name, tok := range id {
		if _, err := l.VerifyIDToken(ctx, tok, "n"); err == nil || errors.Is(err, auth.ErrLogtoUnavailable) {
			t.Errorf("ID token %s: %v", name, err)
		}
	}
	if n := p.JWKSRequests(); n != 1 {
		t.Fatalf("token failures caused %d JWKS requests, want 1", n)
	}
}

func TestBearerReportsJWKSOutageAsDependencyUnavailable(t *testing.T) {
	a, p := newAuthenticator(t, fakeKeys{})
	a.SetGrantResolver(fakeResolver{})
	ctx := context.Background()
	good := p.Sign(t, accessClaims())
	p.SetJWKSStatus(503)
	if _, err := a.Bearer(ctx, good); reasonOf(err) != mcpav1.ErrorReason_ERROR_REASON_DEPENDENCY_UNAVAILABLE {
		t.Fatalf("outage: %v", err)
	}
	p.SetJWKSStatus(0)
	if _, err := a.Bearer(ctx, good); err != nil {
		t.Fatalf("recovered: %v", err)
	}
	if _, err := a.Bearer(ctx, p.SignForged(t, "k1", accessClaims())); reasonOf(err) != mcpav1.ErrorReason_ERROR_REASON_UNAUTHENTICATED {
		t.Fatalf("bad signature: %v", err)
	}
}
