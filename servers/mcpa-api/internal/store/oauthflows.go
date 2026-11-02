// SPDX-License-Identifier: FSL-1.1-ALv2

package store

import (
	"context"
	"errors"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/crypto"
)

// OAuthFlow is the non-secret part of a provider OAuth flow. The sealed
// payload holds the state, PKCE verifier and device codes.
type OAuthFlow struct {
	ID               uuid.UUID
	ResourceID       uuid.UUID
	TenantID         string
	Provider         mcpav1.Provider
	Kind             mcpav1.OauthFlowKind
	State            mcpav1.OauthFlowState
	Name             string
	AuthorizationURL string
	UserCode         string
	VerificationURI  string
	ExpireTime       time.Time
	CredentialID     uuid.UUID // uuid.Nil until completed
	FailureMessage   string
	CreatedByUserID  string
	CreateTime       time.Time
	UpdateTime       time.Time
}

const oauthFlowColumns = `id, resource_id, tenant_id, provider, kind, state, name, authorization_url, user_code, verification_uri,
	expire_time, credential_id, failure_message, created_by_user_id, create_time, update_time`

func scanOAuthFlow(row pgx.Row, extra ...any) (OAuthFlow, error) {
	var f OAuthFlow
	var provider, kind, state int16
	var credentialID *uuid.UUID
	dest := append([]any{&f.ID, &f.ResourceID, &f.TenantID, &provider, &kind, &state, &f.Name, &f.AuthorizationURL, &f.UserCode,
		&f.VerificationURI, &f.ExpireTime, &credentialID, &f.FailureMessage, &f.CreatedByUserID, &f.CreateTime, &f.UpdateTime}, extra...)
	if err := row.Scan(dest...); err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return OAuthFlow{}, ErrNotFound
		}
		return OAuthFlow{}, err
	}
	f.Provider = mcpav1.Provider(provider)
	f.Kind = mcpav1.OauthFlowKind(kind)
	f.State = mcpav1.OauthFlowState(state)
	if credentialID != nil {
		f.CredentialID = *credentialID
	}
	return f, nil
}

// InsertOAuthFlow stores a new pending flow. The payload must be sealed with
// crypto.OAuthFlowAAD(f.ID, f.ResourceID).
func InsertOAuthFlow(ctx context.Context, db DB, f OAuthFlow, s crypto.Sealed) (OAuthFlow, error) {
	err := db.QueryRow(ctx, `INSERT INTO provider_oauth_flows (id, resource_id, tenant_id, provider, kind, state, name,
			authorization_url, user_code, verification_uri, expire_time, created_by_user_id, key_id, wrapped_key, key_nonce, nonce, ciphertext)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)
		RETURNING create_time, update_time`,
		f.ID, f.ResourceID, f.TenantID, int16(f.Provider), int16(f.Kind), int16(f.State), f.Name, f.AuthorizationURL, f.UserCode,
		f.VerificationURI, f.ExpireTime, f.CreatedByUserID, s.KeyID, s.WrappedKey, s.KeyNonce, s.Nonce, s.Ciphertext).
		Scan(&f.CreateTime, &f.UpdateTime)
	return f, err
}

// GetOAuthFlow returns a flow of a resource without its secrets.
func GetOAuthFlow(ctx context.Context, db DB, resourceID, id uuid.UUID) (OAuthFlow, error) {
	return scanOAuthFlow(db.QueryRow(ctx, `SELECT `+oauthFlowColumns+` FROM provider_oauth_flows WHERE resource_id = $1 AND id = $2`, resourceID, id))
}

// LockOAuthFlow returns a flow with its sealed payload and locks the row, so
// one completion or poll at a time exchanges its codes.
func LockOAuthFlow(ctx context.Context, tx pgx.Tx, resourceID, id uuid.UUID) (OAuthFlow, crypto.Sealed, error) {
	var s crypto.Sealed
	f, err := scanOAuthFlow(tx.QueryRow(ctx, `SELECT `+oauthFlowColumns+`, key_id, wrapped_key, key_nonce, nonce, ciphertext
		FROM provider_oauth_flows WHERE resource_id = $1 AND id = $2 FOR UPDATE`, resourceID, id),
		&s.KeyID, &s.WrappedKey, &s.KeyNonce, &s.Nonce, &s.Ciphertext)
	return f, s, err
}

// UpdateOAuthFlowSecret replaces the sealed payload of a pending flow, for
// example after a device-code poll.
func UpdateOAuthFlowSecret(ctx context.Context, db DB, id uuid.UUID, s crypto.Sealed) error {
	_, err := db.Exec(ctx, `UPDATE provider_oauth_flows SET key_id = $2, wrapped_key = $3, key_nonce = $4, nonce = $5, ciphertext = $6,
		update_time = now() WHERE id = $1`, id, s.KeyID, s.WrappedKey, s.KeyNonce, s.Nonce, s.Ciphertext)
	return err
}

// FinishOAuthFlow moves a flow to a terminal state.
func FinishOAuthFlow(ctx context.Context, db DB, id uuid.UUID, state mcpav1.OauthFlowState, credentialID uuid.UUID, failure string) (OAuthFlow, error) {
	var cred *uuid.UUID
	if credentialID != uuid.Nil {
		cred = &credentialID
	}
	// The consent URL and user code are only useful while the flow is
	// pending; they are cleared so ended flows keep no flow parameters.
	return scanOAuthFlow(db.QueryRow(ctx, `UPDATE provider_oauth_flows SET state = $2, credential_id = $3, failure_message = $4,
		authorization_url = '', user_code = '', verification_uri = '', update_time = now() WHERE id = $1 RETURNING `+oauthFlowColumns, id, int16(state), cred, TruncateUTF8(failure, 200)))
}
