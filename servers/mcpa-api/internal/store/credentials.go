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

// Credential is the non-secret metadata of a provider credential.
type Credential struct {
	ID              uuid.UUID
	ResourceID      uuid.UUID
	TenantID        string
	Provider        mcpav1.Provider
	Kind            mcpav1.CredentialKind
	Name            string
	AccountLabel    string
	SecretHint      string
	BaseURL         string
	Models          []string
	CreatedByUserID string
	CreateTime      time.Time
	UpdateTime      time.Time
}

const credentialColumns = `id, resource_id, tenant_id, provider, kind, name, account_label, secret_hint, base_url, models,
	created_by_user_id, create_time, update_time`

func scanCredential(row pgx.Row, extra ...any) (Credential, error) {
	var c Credential
	var provider, kind int16
	dest := append([]any{&c.ID, &c.ResourceID, &c.TenantID, &provider, &kind, &c.Name, &c.AccountLabel, &c.SecretHint,
		&c.BaseURL, &c.Models, &c.CreatedByUserID, &c.CreateTime, &c.UpdateTime}, extra...)
	if err := row.Scan(dest...); err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return Credential{}, ErrNotFound
		}
		return Credential{}, err
	}
	c.Provider = mcpav1.Provider(provider)
	c.Kind = mcpav1.CredentialKind(kind)
	if c.Models == nil {
		c.Models = []string{}
	}
	return c, nil
}

// InsertCredential stores a credential with its sealed payload. The payload
// must be sealed with crypto.CredentialAAD(c.ID, c.ResourceID).
func InsertCredential(ctx context.Context, tx pgx.Tx, c Credential, s crypto.Sealed) (Credential, error) {
	if c.Models == nil {
		c.Models = []string{}
	}
	err := tx.QueryRow(ctx, `INSERT INTO credentials (id, resource_id, tenant_id, provider, kind, name, account_label, secret_hint,
			base_url, models, created_by_user_id, key_id, wrapped_key, key_nonce, nonce, ciphertext)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
		RETURNING create_time, update_time`,
		c.ID, c.ResourceID, c.TenantID, int16(c.Provider), int16(c.Kind), c.Name, c.AccountLabel, c.SecretHint,
		c.BaseURL, c.Models, c.CreatedByUserID, s.KeyID, s.WrappedKey, s.KeyNonce, s.Nonce, s.Ciphertext).Scan(&c.CreateTime, &c.UpdateTime)
	return c, err
}

// ListCredentials returns a resource's credentials without secrets.
func ListCredentials(ctx context.Context, db DB, resourceID uuid.UUID) ([]Credential, error) {
	rows, err := db.Query(ctx, `SELECT `+credentialColumns+` FROM credentials WHERE resource_id = $1 ORDER BY id`, resourceID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Credential
	for rows.Next() {
		c, err := scanCredential(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, c)
	}
	return out, rows.Err()
}

// GetCredentialSealed returns a credential and its sealed payload. Open the
// payload with crypto.CredentialAAD(c.ID, c.ResourceID).
func GetCredentialSealed(ctx context.Context, db DB, id uuid.UUID) (Credential, crypto.Sealed, error) {
	var s crypto.Sealed
	c, err := scanCredential(db.QueryRow(ctx, `SELECT `+credentialColumns+`, key_id, wrapped_key, key_nonce, nonce, ciphertext
		FROM credentials WHERE id = $1`, id), &s.KeyID, &s.WrappedKey, &s.KeyNonce, &s.Nonce, &s.Ciphertext)
	return c, s, err
}

// ListCredentialsSealed returns every credential of a resource with its
// sealed payload. The data plane uses it to build provider auth entries.
func ListCredentialsSealed(ctx context.Context, db DB, resourceID uuid.UUID) ([]Credential, []crypto.Sealed, error) {
	rows, err := db.Query(ctx, `SELECT `+credentialColumns+`, key_id, wrapped_key, key_nonce, nonce, ciphertext
		FROM credentials WHERE resource_id = $1 ORDER BY id`, resourceID)
	if err != nil {
		return nil, nil, err
	}
	defer rows.Close()
	var creds []Credential
	var sealed []crypto.Sealed
	for rows.Next() {
		var s crypto.Sealed
		c, err := scanCredential(rows, &s.KeyID, &s.WrappedKey, &s.KeyNonce, &s.Nonce, &s.Ciphertext)
		if err != nil {
			return nil, nil, err
		}
		creds = append(creds, c)
		sealed = append(sealed, s)
	}
	return creds, sealed, rows.Err()
}

// UpdateCredentialSecret replaces the sealed payload, for example after a
// provider token refresh, and optionally the account label.
func UpdateCredentialSecret(ctx context.Context, db DB, id uuid.UUID, accountLabel string, s crypto.Sealed) error {
	tag, err := db.Exec(ctx, `UPDATE credentials SET key_id = $2, wrapped_key = $3, key_nonce = $4, nonce = $5, ciphertext = $6,
		account_label = $7, update_time = now() WHERE id = $1`, id, s.KeyID, s.WrappedKey, s.KeyNonce, s.Nonce, s.Ciphertext, accountLabel)
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	return nil
}

// DeleteCredential removes a credential of a resource.
func DeleteCredential(ctx context.Context, tx pgx.Tx, resourceID, id uuid.UUID) (Credential, error) {
	return scanCredential(tx.QueryRow(ctx, `DELETE FROM credentials WHERE resource_id = $1 AND id = $2 RETURNING `+credentialColumns, resourceID, id))
}
