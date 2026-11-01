// SPDX-License-Identifier: FSL-1.1-ALv2

package store

import (
	"context"
	"errors"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"

	"github.com/delinoio/apps/servers/mcpa-api/internal/crypto"
)

// ErrCodeReused means an MCP authorization code was already redeemed. The
// returned code still names its grant so the broker can revoke it.
var ErrCodeReused = errors.New("store: authorization code was already used")

// ErrHandleRetired means an MCP refresh handle was already exchanged or does
// not belong to the grant.
var ErrHandleRetired = errors.New("store: refresh handle was already used")

// MCPAuthRequest is a pending MCP authorization between /oauth/authorize and
// /oauth/callback, keyed by the digest of the upstream Logto state.
type MCPAuthRequest struct {
	StateHash     []byte
	BrowserHash   []byte
	ClientID      string
	RedirectURI   string
	ClientState   string
	CodeChallenge string
	// CodeVerifier and Nonce belong to the upstream Logto flow.
	CodeVerifier string
	Nonce        string
	ExpireTime   time.Time
}

// MCPGrant is a durable MCP broker grant. Tokens holds the sealed upstream
// Logto tokens; open it with crypto.MCPGrantAAD(ID).
type MCPGrant struct {
	ID         uuid.UUID
	ClientID   string
	UserID     string
	Subject    string
	CreateTime time.Time
	UpdateTime time.Time
	ExpireTime time.Time
	Tokens     crypto.Sealed
}

// MCPCode is a one-use MCP authorization code.
type MCPCode struct {
	CodeHash      []byte
	GrantID       uuid.UUID
	ClientID      string
	RedirectURI   string
	CodeChallenge string
	ExpireTime    time.Time
}

// MCPGrantChange is applied to a locked grant by UpdateMCPGrant. Zero
// members leave the grant unchanged.
type MCPGrantChange struct {
	// Delete removes the grant with its codes, handles and access tokens.
	Delete bool
	// Tokens replaces the sealed grant tokens.
	Tokens *crypto.Sealed
	// ExpireTime replaces the idle expiry when non-zero.
	ExpireTime time.Time
	// NewHandle is the digest of a new active refresh handle.
	NewHandle []byte
	// AccessToken is the digest of an issued access token valid until
	// AccessExpire.
	AccessToken  []byte
	AccessExpire time.Time
}

// MCPOAuth stores MCP OAuth broker state in PostgreSQL.
type MCPOAuth struct {
	s *Store
}

// NewMCPOAuth wraps s.
func NewMCPOAuth(s *Store) *MCPOAuth { return &MCPOAuth{s: s} }

// InsertAuthRequest stores a pending authorization.
func (m *MCPOAuth) InsertAuthRequest(ctx context.Context, r MCPAuthRequest) error {
	_, err := m.s.Pool.Exec(ctx, `INSERT INTO mcp_authorization_requests (state_hash, browser_hash, client_id, redirect_uri,
			client_state, code_challenge, code_verifier, nonce, expire_time)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)`,
		r.StateHash, r.BrowserHash, r.ClientID, r.RedirectURI, r.ClientState, r.CodeChallenge, r.CodeVerifier, r.Nonce, r.ExpireTime)
	return err
}

// TakeAuthRequest deletes and returns an unexpired pending authorization, so
// a state can be redeemed only once.
func (m *MCPOAuth) TakeAuthRequest(ctx context.Context, stateHash []byte) (MCPAuthRequest, error) {
	r := MCPAuthRequest{StateHash: stateHash}
	err := m.s.Pool.QueryRow(ctx, `DELETE FROM mcp_authorization_requests WHERE state_hash = $1
		RETURNING browser_hash, client_id, redirect_uri, client_state, code_challenge, code_verifier, nonce, expire_time`, stateHash).
		Scan(&r.BrowserHash, &r.ClientID, &r.RedirectURI, &r.ClientState, &r.CodeChallenge, &r.CodeVerifier, &r.Nonce, &r.ExpireTime)
	if errors.Is(err, pgx.ErrNoRows) {
		return MCPAuthRequest{}, ErrNotFound
	}
	if err != nil {
		return MCPAuthRequest{}, err
	}
	if !r.ExpireTime.After(time.Now()) {
		return MCPAuthRequest{}, ErrNotFound
	}
	return r, nil
}

// InsertGrant stores a new grant together with its first authorization code.
func (m *MCPOAuth) InsertGrant(ctx context.Context, g MCPGrant, c MCPCode) error {
	return m.s.InTx(ctx, func(tx pgx.Tx) error {
		if _, err := tx.Exec(ctx, `INSERT INTO mcp_grants (id, client_id, user_id, subject, expire_time,
				key_id, wrapped_key, key_nonce, nonce, ciphertext)
			VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)`,
			g.ID, g.ClientID, g.UserID, g.Subject, g.ExpireTime,
			g.Tokens.KeyID, g.Tokens.WrappedKey, g.Tokens.KeyNonce, g.Tokens.Nonce, g.Tokens.Ciphertext); err != nil {
			return err
		}
		_, err := tx.Exec(ctx, `INSERT INTO mcp_authorization_codes (code_hash, grant_id, client_id, redirect_uri, code_challenge, expire_time)
			VALUES ($1, $2, $3, $4, $5, $6)`, c.CodeHash, g.ID, c.ClientID, c.RedirectURI, c.CodeChallenge, c.ExpireTime)
		return err
	})
}

// RedeemCode marks an unexpired, unused code as used and returns it. A code
// that was already used returns ErrCodeReused with its grant ID; an unknown
// or expired code returns ErrNotFound.
func (m *MCPOAuth) RedeemCode(ctx context.Context, codeHash []byte) (MCPCode, error) {
	c := MCPCode{CodeHash: codeHash}
	err := m.s.Pool.QueryRow(ctx, `UPDATE mcp_authorization_codes SET use_time = now()
		WHERE code_hash = $1 AND use_time IS NULL AND expire_time > now()
		RETURNING grant_id, client_id, redirect_uri, code_challenge, expire_time`, codeHash).
		Scan(&c.GrantID, &c.ClientID, &c.RedirectURI, &c.CodeChallenge, &c.ExpireTime)
	if err == nil {
		return c, nil
	}
	if !errors.Is(err, pgx.ErrNoRows) {
		return MCPCode{}, err
	}
	var used bool
	err = m.s.Pool.QueryRow(ctx, `SELECT grant_id, use_time IS NOT NULL FROM mcp_authorization_codes WHERE code_hash = $1`, codeHash).
		Scan(&c.GrantID, &used)
	if errors.Is(err, pgx.ErrNoRows) {
		return MCPCode{}, ErrNotFound
	}
	if err != nil {
		return MCPCode{}, err
	}
	if used {
		return c, ErrCodeReused
	}
	return MCPCode{}, ErrNotFound
}

// RefreshHandleGrant returns the grant of a refresh handle and whether the
// handle was already retired.
func (m *MCPOAuth) RefreshHandleGrant(ctx context.Context, handleHash []byte) (uuid.UUID, bool, error) {
	var id uuid.UUID
	var retired bool
	err := m.s.Pool.QueryRow(ctx, `SELECT grant_id, retire_time IS NOT NULL FROM mcp_refresh_handles WHERE handle_hash = $1`, handleHash).
		Scan(&id, &retired)
	if errors.Is(err, pgx.ErrNoRows) {
		return uuid.Nil, false, ErrNotFound
	}
	return id, retired, err
}

const mcpGrantColumns = `id, client_id, user_id, subject, create_time, update_time, expire_time,
	key_id, wrapped_key, key_nonce, nonce, ciphertext`

func scanMCPGrant(row pgx.Row) (MCPGrant, error) {
	var g MCPGrant
	err := row.Scan(&g.ID, &g.ClientID, &g.UserID, &g.Subject, &g.CreateTime, &g.UpdateTime, &g.ExpireTime,
		&g.Tokens.KeyID, &g.Tokens.WrappedKey, &g.Tokens.KeyNonce, &g.Tokens.Nonce, &g.Tokens.Ciphertext)
	if errors.Is(err, pgx.ErrNoRows) {
		return MCPGrant{}, ErrNotFound
	}
	return g, err
}

// GetGrant returns an unexpired grant.
func (m *MCPOAuth) GetGrant(ctx context.Context, id uuid.UUID) (MCPGrant, error) {
	return scanMCPGrant(m.s.Pool.QueryRow(ctx, `SELECT `+mcpGrantColumns+` FROM mcp_grants WHERE id = $1 AND expire_time > now()`, id))
}

// GrantByAccessToken returns the unexpired grant of an unexpired issued
// access token.
func (m *MCPOAuth) GrantByAccessToken(ctx context.Context, tokenHash []byte) (MCPGrant, error) {
	return scanMCPGrant(m.s.Pool.QueryRow(ctx, `SELECT g.id, g.client_id, g.user_id, g.subject, g.create_time, g.update_time, g.expire_time,
			g.key_id, g.wrapped_key, g.key_nonce, g.nonce, g.ciphertext
		FROM mcp_access_tokens t JOIN mcp_grants g ON g.id = t.grant_id
		WHERE t.token_hash = $1 AND t.expire_time > now() AND g.expire_time > now()`, tokenHash))
}

// UpdateGrant locks an unexpired grant and applies the change that fn
// returns. When handle is non-nil it must be an active refresh handle of the
// grant, or ErrHandleRetired is returned without calling fn; the handle is
// retired in the same transaction when fn succeeds.
//
// When fn also returns an error, only the Delete and Tokens parts of its
// change are applied, the handle stays active and the error is returned. A
// failed issuance can so keep a rotated upstream refresh token, which the
// upstream refresh already consumed, without issuing a handle or token.
func (m *MCPOAuth) UpdateGrant(ctx context.Context, id uuid.UUID, handle []byte, fn func(MCPGrant) (MCPGrantChange, error)) error {
	var fnErr error
	err := m.s.InTx(ctx, func(tx pgx.Tx) error {
		g, err := scanMCPGrant(tx.QueryRow(ctx, `SELECT `+mcpGrantColumns+` FROM mcp_grants WHERE id = $1 AND expire_time > now() FOR UPDATE`, id))
		if err != nil {
			return err
		}
		// The grant row lock serializes every update of the grant, so the
		// handle cannot be retired between this check and the retirement.
		if handle != nil {
			var active bool
			if err := tx.QueryRow(ctx, `SELECT EXISTS (SELECT 1 FROM mcp_refresh_handles
				WHERE handle_hash = $1 AND grant_id = $2 AND retire_time IS NULL)`, handle, id).Scan(&active); err != nil {
				return err
			}
			if !active {
				return ErrHandleRetired
			}
		}
		ch, err := fn(g)
		if err != nil {
			if !ch.Delete && ch.Tokens == nil {
				return err
			}
			fnErr = err
			return applyMCPGrantChange(ctx, tx, id, MCPGrantChange{Delete: ch.Delete, Tokens: ch.Tokens})
		}
		if handle != nil {
			tag, err := tx.Exec(ctx, `UPDATE mcp_refresh_handles SET retire_time = now()
				WHERE handle_hash = $1 AND grant_id = $2 AND retire_time IS NULL`, handle, id)
			if err != nil {
				return err
			}
			if tag.RowsAffected() != 1 {
				return ErrHandleRetired
			}
		}
		return applyMCPGrantChange(ctx, tx, id, ch)
	})
	if err != nil {
		return err
	}
	return fnErr
}

func applyMCPGrantChange(ctx context.Context, tx pgx.Tx, id uuid.UUID, ch MCPGrantChange) error {
	if ch.Delete {
		_, err := tx.Exec(ctx, `DELETE FROM mcp_grants WHERE id = $1`, id)
		return err
	}
	if ch.Tokens != nil {
		if _, err := tx.Exec(ctx, `UPDATE mcp_grants SET key_id = $2, wrapped_key = $3, key_nonce = $4, nonce = $5, ciphertext = $6,
				update_time = now() WHERE id = $1`,
			id, ch.Tokens.KeyID, ch.Tokens.WrappedKey, ch.Tokens.KeyNonce, ch.Tokens.Nonce, ch.Tokens.Ciphertext); err != nil {
			return err
		}
	}
	if !ch.ExpireTime.IsZero() {
		if _, err := tx.Exec(ctx, `UPDATE mcp_grants SET expire_time = $2, update_time = now() WHERE id = $1`, id, ch.ExpireTime); err != nil {
			return err
		}
	}
	if ch.NewHandle != nil {
		if _, err := tx.Exec(ctx, `INSERT INTO mcp_refresh_handles (handle_hash, grant_id) VALUES ($1, $2)`, ch.NewHandle, id); err != nil {
			return err
		}
	}
	if ch.AccessToken != nil {
		// A refresh can return a token that is already recorded; keep the
		// later expiry.
		if _, err := tx.Exec(ctx, `INSERT INTO mcp_access_tokens (token_hash, grant_id, expire_time) VALUES ($1, $2, $3)
			ON CONFLICT (token_hash) DO UPDATE SET expire_time = GREATEST(mcp_access_tokens.expire_time, EXCLUDED.expire_time)
			WHERE mcp_access_tokens.grant_id = EXCLUDED.grant_id`, ch.AccessToken, id, ch.AccessExpire); err != nil {
			return err
		}
	}
	return nil
}

// DeleteGrant removes a grant with its codes, handles and access tokens and
// returns it so the caller can revoke the upstream refresh token. Expired
// grants are returned as well.
func (m *MCPOAuth) DeleteGrant(ctx context.Context, id uuid.UUID) (MCPGrant, error) {
	return scanMCPGrant(m.s.Pool.QueryRow(ctx, `DELETE FROM mcp_grants WHERE id = $1 RETURNING `+mcpGrantColumns, id))
}

// GrantIDByAccessToken returns the grant of an issued access token, expired
// or not, for revocation.
func (m *MCPOAuth) GrantIDByAccessToken(ctx context.Context, tokenHash []byte) (uuid.UUID, error) {
	var id uuid.UUID
	err := m.s.Pool.QueryRow(ctx, `SELECT grant_id FROM mcp_access_tokens WHERE token_hash = $1`, tokenHash).Scan(&id)
	if errors.Is(err, pgx.ErrNoRows) {
		return uuid.Nil, ErrNotFound
	}
	return id, err
}

// DeleteExpired removes expired authorization requests, codes,
// access token records and grants. It returns the number of rows removed.
func (m *MCPOAuth) DeleteExpired(ctx context.Context) (int64, error) {
	var total int64
	for _, q := range []string{
		`DELETE FROM mcp_authorization_requests WHERE expire_time <= now()`,
		`DELETE FROM mcp_authorization_codes WHERE expire_time <= now()`,
		`DELETE FROM mcp_access_tokens WHERE expire_time <= now()`,
		`DELETE FROM mcp_grants WHERE expire_time <= now()`,
	} {
		tag, err := m.s.Pool.Exec(ctx, q)
		if err != nil {
			return total, err
		}
		total += tag.RowsAffected()
	}
	return total, nil
}
