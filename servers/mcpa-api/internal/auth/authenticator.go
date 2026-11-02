// SPDX-License-Identifier: FSL-1.1-ALv2

package auth

import (
	"context"
	"errors"
	"net/http"
	"strings"
	"sync/atomic"

	delcorev1 "github.com/delinoio/apps/protos/delcore/v1"
	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

// PersonalKeyPrefix starts every Delcore API key secret.
const PersonalKeyPrefix = "dlk_"

// MCPGrant is the Delcore identity bound to an MCP token by the MCP OAuth
// broker.
type MCPGrant struct {
	// UserID is the Delcore internal user ID.
	UserID string
	// DelcoreToken returns a Delcore-audience user token obtained during the
	// broker login. The MCP-audience token itself is never sent to Delcore.
	DelcoreToken TokenFunc
}

// GrantResolver maps a verified MCP-audience Logto access token to the
// grant that the MCP OAuth broker created at approval. The MCP layer
// implements it; without one, MCP tokens are rejected.
type GrantResolver interface {
	ResolveMCPToken(ctx context.Context, token *VerifiedToken) (*MCPGrant, error)
}

// KeyAuthorizer verifies Delcore API keys.
type KeyAuthorizer interface {
	AuthorizeAPIKey(ctx context.Context, secret string, capability delcore.Capability, paid bool) (delcore.KeyDecision, error)
}

// Authenticator resolves principals from HTTP credentials.
type Authenticator struct {
	sessions    *Sessions
	logto       *Logto
	keys        KeyAuthorizer
	webOrigin   string
	mcpResource string
	mfaClaim    string
	reauthURL   string
	grants      atomic.Pointer[GrantResolver]
}

// AuthenticatorConfig configures an Authenticator.
type AuthenticatorConfig struct {
	Sessions    *Sessions
	Logto       *Logto
	Keys        KeyAuthorizer
	WebOrigin   string
	MCPResource string
	MFAClaim    string
	ReauthURL   string
}

// NewAuthenticator builds an Authenticator.
func NewAuthenticator(cfg AuthenticatorConfig) *Authenticator {
	return &Authenticator{
		sessions: cfg.Sessions, logto: cfg.Logto, keys: cfg.Keys, webOrigin: cfg.WebOrigin,
		mcpResource: cfg.MCPResource, mfaClaim: cfg.MFAClaim, reauthURL: cfg.ReauthURL,
	}
}

// SetGrantResolver installs the MCP grant resolver.
func (a *Authenticator) SetGrantResolver(g GrantResolver) {
	if g == nil {
		a.grants.Store(nil)
		return
	}
	a.grants.Store(&g)
}

// ReauthURL returns the MFA enrollment or reauthentication URL.
func (a *Authenticator) ReauthURL() string { return a.reauthURL }

// WebOrigin returns the console origin.
func (a *Authenticator) WebOrigin() string { return a.webOrigin }

// Sessions returns the console session manager.
func (a *Authenticator) Sessions() *Sessions { return a.sessions }

// BearerToken extracts a bearer token from an Authorization header value.
func BearerToken(header string) (string, bool) {
	scheme, token, ok := strings.Cut(strings.TrimSpace(header), " ")
	if !ok || !strings.EqualFold(scheme, "Bearer") {
		return "", false
	}
	token = strings.TrimSpace(token)
	return token, token != ""
}

// Bearer authenticates an MCP-audience Logto token or a personal key. The
// MCP layer uses it for /mcp; management uses it through Request.
func (a *Authenticator) Bearer(ctx context.Context, token string) (*Principal, error) {
	if strings.HasPrefix(token, PersonalKeyPrefix) {
		return a.personalKey(ctx, token)
	}
	if strings.Count(token, ".") != 2 {
		return nil, apperr.Unauthenticated("invalid bearer token")
	}
	verified, err := a.logto.VerifyAccessToken(ctx, token, a.mcpResource)
	if err != nil {
		if errors.Is(err, ErrLogtoUnavailable) {
			return nil, apperr.DependencyUnavailable(err)
		}
		return nil, apperr.Unauthenticated("invalid or expired access token")
	}
	if !MFAEvidence(verified.Claims, a.mfaClaim) {
		return nil, apperr.MFARequired(a.reauthURL)
	}
	resolver := a.grants.Load()
	if resolver == nil {
		return nil, apperr.Unauthenticated("MCP access tokens are not accepted by this server")
	}
	grant, err := (*resolver).ResolveMCPToken(ctx, verified)
	if err != nil {
		return nil, err
	}
	if grant == nil || grant.UserID == "" {
		return nil, apperr.Unauthenticated("the MCP grant is not valid; sign in again")
	}
	return NewUserPrincipal(mcpav1.ActorKind_ACTOR_KIND_MCP, grant.UserID, verified.Subject, grant.DelcoreToken), nil
}

func (a *Authenticator) personalKey(ctx context.Context, secret string) (*Principal, error) {
	// The capability only selects the grant evaluated; attribution of a valid
	// key is returned even when this capability is not granted.
	d, err := a.keys.AuthorizeAPIKey(ctx, secret, delcore.CapabilityAccessResources, false)
	if err != nil {
		if delcore.IsUnauthenticated(err) {
			return nil, apperr.Unauthenticated("invalid Delcore API key")
		}
		return nil, apperr.DependencyUnavailable(err)
	}
	switch d.OwnerKind {
	case delcorev1.ApiKeyOwnerKind_API_KEY_OWNER_KIND_PERSONAL:
	case delcorev1.ApiKeyOwnerKind_API_KEY_OWNER_KIND_TENANT:
		return nil, apperr.TenantServiceKeyRejected()
	default:
		return nil, apperr.Unauthenticated("invalid Delcore API key")
	}
	if d.UserID == "" || d.TenantID == "" {
		return nil, apperr.Unauthenticated("invalid Delcore API key")
	}
	return NewPersonalKeyPrincipal(d.UserID, d.TenantID, secret), nil
}

// Request authenticates a management request. A bearer credential takes
// precedence; otherwise the console session cookie is used and the CSRF
// header must match. A present Origin must equal the console origin.
func (a *Authenticator) Request(ctx context.Context, h http.Header) (*Principal, error) {
	if !CheckOrigin(h.Get("Origin"), a.webOrigin) {
		return nil, apperr.PermissionDenied("origin is not allowed")
	}
	if authz := h.Get("Authorization"); authz != "" {
		token, ok := BearerToken(authz)
		if !ok {
			return nil, apperr.Unauthenticated("unsupported authorization scheme")
		}
		return a.Bearer(ctx, token)
	}
	st, err := a.SessionFromHeader(ctx, h)
	if err != nil {
		return nil, err
	}
	if !st.Session.MFA {
		return nil, apperr.MFARequired(a.reauthURL)
	}
	if !CheckCSRF(h.Get(CSRFHeader), st.Session.CSRFToken) {
		return nil, apperr.PermissionDenied("missing or invalid CSRF token")
	}
	return a.sessions.principalFromSession(st), nil
}

// SessionFromHeader resolves the console session cookie in h.
func (a *Authenticator) SessionFromHeader(ctx context.Context, h http.Header) (*SessionState, error) {
	c, err := (&http.Request{Header: h}).Cookie(SessionCookie)
	if err != nil {
		return nil, apperr.Unauthenticated("sign in to continue")
	}
	return a.sessions.Lookup(ctx, c.Value)
}

// StoredSessionFromHeader resolves the console session cookie in h without
// refreshing its tokens.
func (a *Authenticator) StoredSessionFromHeader(ctx context.Context, h http.Header) (store.Session, error) {
	c, err := (&http.Request{Header: h}).Cookie(SessionCookie)
	if err != nil {
		return store.Session{}, apperr.Unauthenticated("sign in to continue")
	}
	return a.sessions.Stored(ctx, c.Value)
}
