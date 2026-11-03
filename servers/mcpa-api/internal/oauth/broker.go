// SPDX-License-Identifier: FSL-1.1-ALv2

// Package oauth is the OAuth 2.1 broker that MCP clients use to obtain
// access to /mcp.
//
// The broker publishes protected-resource and authorization-server metadata
// and serves /oauth/authorize, /oauth/callback, /oauth/token and
// /oauth/revoke. Clients are the operator-approved registry
// MCPA_OAUTH_CLIENTS; there is no dynamic registration. Authorization runs
// the broker's own Logto authorization code flow with PKCE that requests the
// MCP-only audience and the Delcore resource. The callback verifies MFA
// evidence, resolves the Delcore internal user with a Delcore-audience token
// and binds a durable grant to that user and client.
//
// Clients receive the Logto access token for the MCP-only audience and an
// opaque refresh handle. The upstream Logto refresh token stays sealed on the
// server. Every issued access token is recorded by digest so /mcp maps it to
// its grant and revocation takes effect before the token expires. The
// MCP-audience token is never sent to Delcore.
package oauth

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"strings"
	"time"

	"github.com/google/uuid"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/config"
	"github.com/delinoio/apps/servers/mcpa-api/internal/crypto"
	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
	"github.com/delinoio/apps/servers/mcpa-api/internal/obs"
	"github.com/delinoio/apps/servers/mcpa-api/internal/server"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

// Lifetimes and limits of broker state.
const (
	// RequestLifetime bounds the browser login between authorize and callback.
	RequestLifetime = 10 * time.Minute
	// defaultDependencyTimeout bounds the group snapshot refresh when the
	// configuration sets no dependency timeout.
	defaultDependencyTimeout = 5 * time.Second
	// CodeLifetime bounds an authorization code.
	CodeLifetime = time.Minute
	// GrantIdleLifetime expires a grant that issues no token for this long.
	GrantIdleLifetime = 30 * 24 * time.Hour
	// CleanupInterval is the period of the expiry cleanup worker.
	CleanupInterval = 5 * time.Minute
	// maxSnapshotTenants bounds the group snapshot refreshes of one login.
	maxSnapshotTenants = 20
	// tokenRefreshMargin refreshes the Delcore token shortly before expiry.
	tokenRefreshMargin = time.Minute
)

// Paths of the broker endpoints.
const (
	ProtectedResourcePath   = "/.well-known/oauth-protected-resource"
	AuthorizationServerPath = "/.well-known/oauth-authorization-server"
	AuthorizePath           = "/oauth/authorize"
	CallbackPath            = "/oauth/callback"
	TokenPath               = "/oauth/token"
	RevokePath              = "/oauth/revoke"
)

// browserCookie binds a pending authorization to the browser that started
// it, so a login completed in another browser issues no code.
const browserCookie = "mcpa_mcp_oauth"

// loginScopes are requested from Logto. offline_access yields the refresh
// token the grant keeps.
const loginScopes = "openid offline_access profile"

// Store persists broker state. *store.MCPOAuth implements it.
type Store interface {
	InsertAuthRequest(ctx context.Context, r store.MCPAuthRequest) error
	TakeAuthRequest(ctx context.Context, stateHash []byte) (store.MCPAuthRequest, error)
	InsertGrant(ctx context.Context, g store.MCPGrant, c store.MCPCode) error
	LookupCode(ctx context.Context, codeHash []byte) (store.MCPCode, error)
	RefreshHandleGrant(ctx context.Context, handleHash []byte) (uuid.UUID, bool, error)
	GetGrant(ctx context.Context, id uuid.UUID) (store.MCPGrant, error)
	GrantByAccessToken(ctx context.Context, tokenHash []byte) (store.MCPGrant, error)
	GrantIDByAccessToken(ctx context.Context, tokenHash []byte) (uuid.UUID, error)
	UpdateGrant(ctx context.Context, id uuid.UUID, use store.MCPGrantUse, fn func(ctx context.Context, g store.MCPGrant) (store.MCPGrantChange, error)) error
	DeleteGrant(ctx context.Context, id uuid.UUID) (store.MCPGrant, error)
	DeleteExpired(ctx context.Context) (int64, error)
}

// UserReader resolves the Delcore user of a Delcore-audience token.
type UserReader interface {
	GetCurrentUser(ctx context.Context, userToken string) (delcore.CurrentUser, error)
}

// GroupRefresher refreshes a tenant group-membership snapshot with the
// principal's Delcore user token. *authz.Authorizer implements it.
type GroupRefresher interface {
	RefreshGroups(ctx context.Context, p *auth.Principal, tenantID string)
}

// Config configures the broker.
type Config struct {
	Store   Store
	Keys    *crypto.Keyring
	Logto   *auth.Logto
	Delcore UserReader
	// Groups may be nil; then logins refresh no group snapshots.
	Groups  GroupRefresher
	Clients map[string]config.OAuthClient
	// PublicURL is the API origin and the broker issuer.
	PublicURL       string
	WebOrigin       string
	MCPResource     string
	DelcoreResource string
	MFAClaim        string
	ReauthURL       string
	InsecureCookies bool
	// DependencyTimeout bounds the whole post-login group snapshot refresh.
	// Zero means defaultDependencyTimeout.
	DependencyTimeout time.Duration
	Log               *slog.Logger
}

// Broker serves the OAuth endpoints and resolves MCP tokens to grants.
type Broker struct {
	cfg Config
	now func() time.Time
}

var _ auth.GrantResolver = (*Broker)(nil)

// New builds the broker.
func New(cfg Config) *Broker {
	cfg.PublicURL = strings.TrimSuffix(cfg.PublicURL, "/")
	if cfg.DependencyTimeout <= 0 {
		cfg.DependencyTimeout = defaultDependencyTimeout
	}
	return &Broker{cfg: cfg, now: time.Now}
}

// SetClock replaces the clock in tests.
func (b *Broker) SetClock(now func() time.Time) { b.now = now }

// ResourceMetadataURL is the protected-resource metadata URL announced in
// WWW-Authenticate challenges of /mcp.
func ResourceMetadataURL(publicURL string) string {
	return strings.TrimSuffix(publicURL, "/") + ProtectedResourcePath
}

// Mounts returns the broker routes for app.Mount.
func (b *Broker) Mounts() []server.Mount {
	mux := http.NewServeMux()
	mux.HandleFunc("GET "+AuthorizePath, b.authorize)
	mux.HandleFunc("GET "+CallbackPath, b.callback)
	mux.Handle("POST "+TokenPath, b.apiPolicy(http.HandlerFunc(b.token)))
	mux.Handle("POST "+RevokePath, b.apiPolicy(http.HandlerFunc(b.revoke)))
	oauthRoutes := http.Handler(mux)
	return []server.Mount{
		{Pattern: ProtectedResourcePath, Handler: metadataHandler(b.protectedResourceMetadata), Surface: obs.SurfaceOAuth},
		// RFC 9728 path-inserted form for the /mcp resource.
		{Pattern: ProtectedResourcePath + "/mcp", Handler: metadataHandler(b.protectedResourceMetadata), Surface: obs.SurfaceOAuth},
		{Pattern: AuthorizationServerPath, Handler: metadataHandler(b.authorizationServerMetadata), Surface: obs.SurfaceOAuth},
		{Pattern: "/oauth/", Handler: oauthRoutes, Surface: obs.SurfaceOAuth},
	}
}

// apiPolicy rejects a present Origin other than the console origin before
// the handler runs, following the Lane origin rule. The token and revocation
// endpoints grant no CORS access: MCP clients call them from native or
// server-side code.
func (b *Broker) apiPolicy(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !auth.CheckOrigin(r.Header.Get("Origin"), b.cfg.WebOrigin) {
			writeJSON(w, http.StatusForbidden, map[string]string{"error": "origin_not_allowed"})
			return
		}
		next.ServeHTTP(w, r)
	})
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("Pragma", "no-cache")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

// grantTokens is the sealed payload of a grant. Its fields are secrets.
type grantTokens struct {
	RefreshToken       string    `json:"refresh_token"`
	DelcoreAccessToken string    `json:"delcore_access_token"`
	DelcoreExpiry      time.Time `json:"delcore_expiry"`
}

func (b *Broker) seal(id uuid.UUID, t grantTokens) (crypto.Sealed, error) {
	plain, err := json.Marshal(t)
	if err != nil {
		return crypto.Sealed{}, err
	}
	defer clear(plain)
	return b.cfg.Keys.Seal(plain, crypto.MCPGrantAAD(id.String()))
}

func (b *Broker) open(g store.MCPGrant) (grantTokens, error) {
	plain, err := b.cfg.Keys.Open(g.Tokens, crypto.MCPGrantAAD(g.ID.String()))
	if err != nil {
		return grantTokens{}, err
	}
	defer clear(plain)
	var t grantTokens
	if err := json.Unmarshal(plain, &t); err != nil {
		return grantTokens{}, err
	}
	return t, nil
}

// ResolveMCPToken maps a verified MCP-audience access token to its grant.
// Only tokens that the broker issued for a live grant are accepted.
func (b *Broker) ResolveMCPToken(ctx context.Context, tok *auth.VerifiedToken) (*auth.MCPGrant, error) {
	if len(tok.Digest) == 0 {
		return nil, apperr.Unauthenticated("the MCP grant is not valid; sign in again")
	}
	g, err := b.cfg.Store.GrantByAccessToken(ctx, tok.Digest)
	if errors.Is(err, store.ErrNotFound) {
		return nil, apperr.Unauthenticated("the MCP grant is not valid; sign in again")
	}
	if err != nil {
		return nil, apperr.DependencyUnavailable(err)
	}
	if g.Subject != tok.Subject {
		return nil, apperr.Unauthenticated("the MCP grant is not valid; sign in again")
	}
	cached, err := b.open(g)
	if err != nil {
		return nil, apperr.Internal(fmt.Errorf("open grant tokens: %w", err))
	}
	grantID := g.ID
	return &auth.MCPGrant{
		UserID: g.UserID,
		DelcoreToken: func(ctx context.Context) (string, error) {
			if b.now().Add(tokenRefreshMargin).Before(cached.DelcoreExpiry) {
				return cached.DelcoreAccessToken, nil
			}
			// The refreshed token is not cached here: a later call finds it
			// under the grant lock without another upstream refresh.
			return b.refreshDelcoreToken(ctx, grantID)
		},
	}, nil
}

// refreshDelcoreToken renews the Delcore-audience token of a grant under the
// grant lock and returns it. A rejected upstream refresh token deletes the
// grant.
func (b *Broker) refreshDelcoreToken(ctx context.Context, id uuid.UUID) (string, error) {
	var token string
	ran := false
	err := b.cfg.Store.UpdateGrant(ctx, id, store.MCPGrantUse{}, func(wctx context.Context, g store.MCPGrant) (store.MCPGrantChange, error) {
		ran = true
		t, err := b.open(g)
		if err != nil {
			return store.MCPGrantChange{}, apperr.Internal(fmt.Errorf("open grant tokens: %w", err))
		}
		// Another request may have refreshed while this one waited.
		if b.now().Add(tokenRefreshMargin).Before(t.DelcoreExpiry) {
			token = t.DelcoreAccessToken
			return store.MCPGrantChange{}, nil
		}
		dc, err := b.cfg.Logto.RefreshForResource(wctx, t.RefreshToken, b.cfg.DelcoreResource)
		if errors.Is(err, auth.ErrInvalidGrant) {
			b.cfg.Log.InfoContext(wctx, "mcp grant upstream refresh token rejected; grant deleted", "grant_id", id)
			return store.MCPGrantChange{Delete: true}, auth.ErrNoUserToken
		}
		if err != nil {
			return store.MCPGrantChange{}, err
		}
		if dc.RefreshToken != "" {
			t.RefreshToken = dc.RefreshToken
		}
		t.DelcoreAccessToken = dc.AccessToken
		t.DelcoreExpiry = b.now().Add(time.Duration(dc.ExpiresIn) * time.Second)
		sealed, err := b.seal(id, t)
		if err != nil {
			return store.MCPGrantChange{}, apperr.Internal(err)
		}
		token = dc.AccessToken
		return store.MCPGrantChange{Tokens: &sealed}, nil
	})
	if ran && ctx.Err() != nil {
		b.cfg.Log.WarnContext(ctx, "mcp grant Delcore token refresh finished after the request was canceled",
			"grant_id", id, "request_cancelled", true, "ok", err == nil)
	}
	if errors.Is(err, store.ErrNotFound) {
		return "", auth.ErrNoUserToken
	}
	return token, err
}

// revokeGrant deletes a grant and revokes its upstream refresh token. It
// returns a failure to delete the local grant, which then stays usable; an
// already deleted grant is not an error. Upstream revocation is best effort:
// its failures are logged once the local grant is gone. Token endpoint
// callers that detect reuse answer invalid_grant whatever the result, because
// the presented code or handle is invalid either way.
func (b *Broker) revokeGrant(ctx context.Context, id uuid.UUID, reason string) error {
	g, err := b.cfg.Store.DeleteGrant(ctx, id)
	if errors.Is(err, store.ErrNotFound) {
		return nil
	}
	if err != nil {
		b.cfg.Log.ErrorContext(ctx, "delete mcp grant failed", "grant_id", id, "reason", reason, "error", err)
		return fmt.Errorf("delete mcp grant: %w", err)
	}
	b.cfg.Log.InfoContext(ctx, "mcp grant revoked", "grant_id", id, "client_id", g.ClientID, "reason", reason)
	t, err := b.open(g)
	if err != nil || t.RefreshToken == "" {
		return nil
	}
	b.revokeUpstream(ctx, t.RefreshToken)
	return nil
}

func (b *Broker) revokeUpstream(ctx context.Context, refreshToken string) {
	if err := b.cfg.Logto.RevokeRefreshToken(context.WithoutCancel(ctx), refreshToken); err != nil {
		b.cfg.Log.WarnContext(ctx, "upstream refresh token revocation failed", "error", err)
	}
}

// RunCleanup deletes expired broker state every CleanupInterval until ctx
// ends. Failures are logged and retried at the next interval.
func (b *Broker) RunCleanup(ctx context.Context) error {
	ticker := time.NewTicker(CleanupInterval)
	defer ticker.Stop()
	for {
		n, err := b.cfg.Store.DeleteExpired(ctx)
		switch {
		case err != nil && ctx.Err() == nil:
			b.cfg.Log.WarnContext(ctx, "mcp oauth cleanup failed", "error", err)
		case n > 0:
			b.cfg.Log.InfoContext(ctx, "mcp oauth cleanup removed expired rows", "rows", n)
		}
		select {
		case <-ctx.Done():
			return nil
		case <-ticker.C:
		}
	}
}

// mfaRequired is the typed MFA error of this broker.
func (b *Broker) mfaRequired() *apperr.Error { return apperr.MFARequired(b.cfg.ReauthURL) }

// isMFA reports whether err is the typed MFA error.
func isMFA(err error) bool {
	var ae *apperr.Error
	return errors.As(err, &ae) && ae.Reason == mcpav1.ErrorReason_ERROR_REASON_MFA_REQUIRED
}
