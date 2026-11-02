// SPDX-License-Identifier: FSL-1.1-ALv2

// Package auth resolves the caller of management, console and MCP surfaces.
//
// Three credentials are accepted:
//   - a console session cookie (mcpa_session) with the X-Mcpa-CSRF header,
//   - a Logto access token for the MCP-only audience, mapped to a Delcore user
//     by the MCP OAuth broker through a GrantResolver,
//   - a Delcore personal key (dlk_...), verified with AuthorizeApiKey.
//
// Console sessions and MCP tokens require Logto MFA evidence. Personal keys
// are exempt. Tenant service keys are rejected.
package auth

import (
	"context"
	"errors"

	"github.com/google/uuid"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
)

// ErrNoUserToken means the principal has no Delcore user token, for
// example because it authenticated with a personal key.
var ErrNoUserToken = errors.New("auth: principal has no Delcore user token")

// TokenFunc returns a current Delcore-audience access token of the user.
type TokenFunc func(ctx context.Context) (string, error)

// Principal is an authenticated management caller.
type Principal struct {
	Kind mcpav1.ActorKind
	// UserID is the Delcore internal user ID.
	UserID string
	// Subject is the Logto subject for console and MCP principals.
	Subject string
	// KeyTenantID is the only tenant a personal key may act in.
	KeyTenantID string
	// SessionID identifies a console session.
	SessionID uuid.UUID

	keySecret    string
	delcoreToken TokenFunc
}

// NewPersonalKeyPrincipal builds a principal for a verified personal key.
func NewPersonalKeyPrincipal(userID, tenantID, secret string) *Principal {
	return &Principal{Kind: mcpav1.ActorKind_ACTOR_KIND_PERSONAL_KEY, UserID: userID, KeyTenantID: tenantID, keySecret: secret}
}

// NewUserPrincipal builds a console or MCP principal with a source of
// Delcore user tokens.
func NewUserPrincipal(kind mcpav1.ActorKind, userID, subject string, tokens TokenFunc) *Principal {
	return &Principal{Kind: kind, UserID: userID, Subject: subject, delcoreToken: tokens}
}

// IsPersonalKey reports whether the principal is a Delcore personal key.
func (p *Principal) IsPersonalKey() bool { return p.Kind == mcpav1.ActorKind_ACTOR_KIND_PERSONAL_KEY }

// APIKeySecret returns the personal key secret for AuthorizeApiKey checks.
// It must never be logged or stored.
func (p *Principal) APIKeySecret() string { return p.keySecret }

// HasUserToken reports whether DelcoreToken can succeed.
func (p *Principal) HasUserToken() bool { return p.delcoreToken != nil }

// DelcoreToken returns a Delcore-audience user token. Personal keys have
// none.
func (p *Principal) DelcoreToken(ctx context.Context) (string, error) {
	if p.delcoreToken == nil {
		return "", ErrNoUserToken
	}
	return p.delcoreToken(ctx)
}

type principalKey struct{}

// WithPrincipal stores the principal in ctx.
func WithPrincipal(ctx context.Context, p *Principal) context.Context {
	return context.WithValue(ctx, principalKey{}, p)
}

// FromContext returns the authenticated principal or nil.
func FromContext(ctx context.Context) *Principal {
	p, _ := ctx.Value(principalKey{}).(*Principal)
	return p
}
