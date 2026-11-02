// SPDX-License-Identifier: FSL-1.1-ALv2

package auth_test

import (
	"context"
	"errors"
	"net/http"
	"testing"
	"time"

	delcorev1 "github.com/delinoio/apps/protos/delcore/v1"
	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth/authtest"
	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
)

const (
	webOrigin   = "https://mcpa.example.test"
	mcpAudience = "https://api.example.test/mcp"
	apiAudience = "https://api.example.test"
	reauthURL   = "https://login.example.test/mfa"
)

func TestMFAEvidence(t *testing.T) {
	cases := []struct {
		name   string
		claims map[string]any
		want   bool
	}{
		{"claim true", map[string]any{"mcpa_mfa": true}, true},
		{"claim false", map[string]any{"mcpa_mfa": false}, false},
		{"claim string", map[string]any{"mcpa_mfa": "true"}, false},
		{"amr mfa", map[string]any{"amr": []any{"pwd", "mfa"}}, true},
		{"amr otp", map[string]any{"amr": []any{"otp"}}, true},
		{"amr hwk", map[string]any{"amr": []any{"hwk"}}, true},
		{"amr pwd", map[string]any{"amr": []any{"pwd"}}, false},
		{"amr string", map[string]any{"amr": "mfa"}, false},
		{"none", map[string]any{}, false},
		{"other claim", map[string]any{"other": true}, false},
	}
	for _, c := range cases {
		if got := auth.MFAEvidence(c.claims, "mcpa_mfa"); got != c.want {
			t.Errorf("%s: got %v", c.name, got)
		}
	}
}

func TestCSRFAndOrigin(t *testing.T) {
	if !auth.CheckCSRF("abc", "abc") || auth.CheckCSRF("abd", "abc") || auth.CheckCSRF("", "") || auth.CheckCSRF("", "abc") {
		t.Fatal("CSRF comparison")
	}
	if !auth.CheckOrigin("", webOrigin) || !auth.CheckOrigin(webOrigin, webOrigin) {
		t.Fatal("allowed origins rejected")
	}
	for _, o := range []string{"null", "https://evil.test", webOrigin + ":443", "http://mcpa.example.test"} {
		if auth.CheckOrigin(o, webOrigin) {
			t.Fatalf("origin %q accepted", o)
		}
	}
}

type fakeKeys struct {
	decision delcore.KeyDecision
	err      error
}

func (f fakeKeys) AuthorizeAPIKey(context.Context, string, delcore.Capability, bool) (delcore.KeyDecision, error) {
	return f.decision, f.err
}

type fakeResolver struct{}

func (fakeResolver) ResolveMCPToken(_ context.Context, tok *auth.VerifiedToken) (*auth.MCPGrant, error) {
	return &auth.MCPGrant{UserID: "user-for-" + tok.Subject, DelcoreToken: func(context.Context) (string, error) { return "dc", nil }}, nil
}

func newAuthenticator(t *testing.T, keys auth.KeyAuthorizer) (*auth.Authenticator, *authtest.Provider) {
	p := authtest.New(t)
	logto := auth.NewLogto(p.Issuer(), "console-client", "secret", p.Server.Client())
	a := auth.NewAuthenticator(auth.AuthenticatorConfig{
		Logto: logto, Keys: keys, WebOrigin: webOrigin, MCPResource: mcpAudience, MFAClaim: "mcpa_mfa", ReauthURL: reauthURL,
	})
	return a, p
}

func reasonOf(err error) mcpav1.ErrorReason { return apperr.As(err).Reason }

func TestBearerMCPToken(t *testing.T) {
	a, p := newAuthenticator(t, fakeKeys{})
	ctx := context.Background()
	good := p.Sign(t, map[string]any{"sub": "logto-1", "aud": mcpAudience, "mcpa_mfa": true})

	if _, err := a.Bearer(ctx, good); reasonOf(err) != mcpav1.ErrorReason_ERROR_REASON_UNAUTHENTICATED {
		t.Fatalf("without a grant resolver MCP tokens must be rejected: %v", err)
	}
	a.SetGrantResolver(fakeResolver{})
	pr, err := a.Bearer(ctx, good)
	if err != nil {
		t.Fatal(err)
	}
	if pr.Kind != mcpav1.ActorKind_ACTOR_KIND_MCP || pr.UserID != "user-for-logto-1" || !pr.HasUserToken() {
		t.Fatalf("principal %+v", pr)
	}

	noMFA := p.Sign(t, map[string]any{"sub": "logto-1", "aud": mcpAudience})
	_, err = a.Bearer(ctx, noMFA)
	if e := apperr.As(err); e.Reason != mcpav1.ErrorReason_ERROR_REASON_MFA_REQUIRED || e.ReauthURL != reauthURL {
		t.Fatalf("missing MFA: %+v", e)
	}
	amr := p.Sign(t, map[string]any{"sub": "logto-1", "aud": []string{mcpAudience}, "amr": []string{"pwd", "otp"}})
	if _, err := a.Bearer(ctx, amr); err != nil {
		t.Fatalf("amr evidence: %v", err)
	}

	for name, tok := range map[string]string{
		"console audience": p.Sign(t, map[string]any{"sub": "logto-1", "aud": apiAudience, "mcpa_mfa": true}),
		"expired":          p.Sign(t, map[string]any{"sub": "logto-1", "aud": mcpAudience, "mcpa_mfa": true, "exp": time.Now().Add(-time.Hour).Unix()}),
		"other issuer":     p.Sign(t, map[string]any{"iss": "https://evil.test", "sub": "logto-1", "aud": mcpAudience, "mcpa_mfa": true}),
		"garbage":          "a.b.c",
		"not a jwt":        "opaque",
	} {
		if _, err := a.Bearer(ctx, tok); reasonOf(err) != mcpav1.ErrorReason_ERROR_REASON_UNAUTHENTICATED {
			t.Errorf("%s: %v", name, err)
		}
	}
}

func TestBearerPersonalKey(t *testing.T) {
	ctx := context.Background()
	personal := delcore.KeyDecision{OwnerKind: delcorev1.ApiKeyOwnerKind_API_KEY_OWNER_KIND_PERSONAL, UserID: "u1", TenantID: "t1"}
	a, _ := newAuthenticator(t, fakeKeys{decision: personal})
	pr, err := a.Bearer(ctx, "dlk_secret")
	if err != nil {
		t.Fatal(err)
	}
	if !pr.IsPersonalKey() || pr.UserID != "u1" || pr.KeyTenantID != "t1" || pr.HasUserToken() || pr.APIKeySecret() != "dlk_secret" {
		t.Fatalf("principal %+v", pr)
	}
	if _, err := pr.DelcoreToken(ctx); !errors.Is(err, auth.ErrNoUserToken) {
		t.Fatalf("personal keys have no user token: %v", err)
	}

	tenant := delcore.KeyDecision{OwnerKind: delcorev1.ApiKeyOwnerKind_API_KEY_OWNER_KIND_TENANT, UserID: "u1", TenantID: "t1", Allowed: true}
	a, _ = newAuthenticator(t, fakeKeys{decision: tenant})
	if _, err := a.Bearer(ctx, "dlk_secret"); reasonOf(err) != mcpav1.ErrorReason_ERROR_REASON_TENANT_SERVICE_KEY_REJECTED {
		t.Fatalf("tenant key: %v", err)
	}

	a, _ = newAuthenticator(t, fakeKeys{err: &delcore.Error{Kind: delcore.KindUnauthenticated}})
	if _, err := a.Bearer(ctx, "dlk_secret"); reasonOf(err) != mcpav1.ErrorReason_ERROR_REASON_UNAUTHENTICATED {
		t.Fatalf("invalid key: %v", err)
	}
	a, _ = newAuthenticator(t, fakeKeys{err: &delcore.Error{Kind: delcore.KindUnavailable}})
	if _, err := a.Bearer(ctx, "dlk_secret"); reasonOf(err) != mcpav1.ErrorReason_ERROR_REASON_DEPENDENCY_UNAVAILABLE {
		t.Fatalf("Delcore down must fail closed: %v", err)
	}
}

func TestRequestOriginAndScheme(t *testing.T) {
	personal := delcore.KeyDecision{OwnerKind: delcorev1.ApiKeyOwnerKind_API_KEY_OWNER_KIND_PERSONAL, UserID: "u1", TenantID: "t1"}
	a, _ := newAuthenticator(t, fakeKeys{decision: personal})
	ctx := context.Background()

	h := http.Header{}
	h.Set("Authorization", "Bearer dlk_x")
	if _, err := a.Request(ctx, h); err != nil {
		t.Fatalf("bearer without Origin: %v", err)
	}
	h.Set("Origin", webOrigin)
	if _, err := a.Request(ctx, h); err != nil {
		t.Fatalf("bearer with console Origin: %v", err)
	}
	h.Set("Origin", "https://evil.test")
	if _, err := a.Request(ctx, h); reasonOf(err) != mcpav1.ErrorReason_ERROR_REASON_PERMISSION_DENIED {
		t.Fatalf("hostile origin: %v", err)
	}
	h = http.Header{}
	h.Set("Authorization", "Basic abc")
	if _, err := a.Request(ctx, h); reasonOf(err) != mcpav1.ErrorReason_ERROR_REASON_UNAUTHENTICATED {
		t.Fatalf("basic auth: %v", err)
	}
	if _, err := a.Request(ctx, http.Header{}); reasonOf(err) != mcpav1.ErrorReason_ERROR_REASON_UNAUTHENTICATED {
		t.Fatalf("no credentials: %v", err)
	}
}

func TestBearerToken(t *testing.T) {
	if tok, ok := auth.BearerToken("bearer abc"); !ok || tok != "abc" {
		t.Fatal("case-insensitive scheme")
	}
	for _, h := range []string{"", "Bearer", "Bearer ", "Token abc"} {
		if _, ok := auth.BearerToken(h); ok {
			t.Errorf("accepted %q", h)
		}
	}
}
