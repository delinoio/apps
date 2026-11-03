// SPDX-License-Identifier: FSL-1.1-ALv2

package store

import (
	"context"
	"errors"
	"fmt"
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

// ErrStorage marks an error caused by the database: an unreachable server, a
// broken connection, a timeout or a failed statement or commit. It is
// retryable. It wraps the original error, so errors.Is still matches it.
var ErrStorage = errors.New("store: storage failure")

// storageErr marks a non-nil err as a storage failure.
func storageErr(err error) error {
	if err == nil {
		return nil
	}
	return fmt.Errorf("%w: %w", ErrStorage, err)
}

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

// MCPGrantUse names the one-use credentials that UpdateGrant consumes when
// its change succeeds. Nil members consume nothing.
type MCPGrantUse struct {
	// Handle is the digest of an active refresh handle of the grant; a
	// successful change retires it.
	Handle []byte
	// Code is the digest of an unused, unexpired authorization code of the
	// grant; a successful change marks it used.
	Code []byte
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

// LookupCode returns an unexpired, unused code without consuming it.
// UpdateGrant consumes the code together with the first successful token
// issuance, so an exchange that fails before issuing tokens leaves the code
// usable for a retry. A code that was already used returns ErrCodeReused
// with its grant ID; an unknown or expired code returns ErrNotFound.
func (m *MCPOAuth) LookupCode(ctx context.Context, codeHash []byte) (MCPCode, error) {
	c := MCPCode{CodeHash: codeHash}
	var used, live bool
	err := m.s.Pool.QueryRow(ctx, `SELECT grant_id, client_id, redirect_uri, code_challenge, expire_time,
			use_time IS NOT NULL, expire_time > now()
		FROM mcp_authorization_codes WHERE code_hash = $1`, codeHash).
		Scan(&c.GrantID, &c.ClientID, &c.RedirectURI, &c.CodeChallenge, &c.ExpireTime, &used, &live)
	switch {
	case errors.Is(err, pgx.ErrNoRows):
		return MCPCode{}, ErrNotFound
	case err != nil:
		return MCPCode{}, err
	case used:
		return MCPCode{CodeHash: codeHash, GrantID: c.GrantID}, ErrCodeReused
	case !live:
		return MCPCode{}, ErrNotFound
	}
	return c, nil
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
// returns. When use.Handle is non-nil it must be an active refresh handle of
// the grant, or ErrHandleRetired is returned without calling fn. When
// use.Code is non-nil it must be an unexpired authorization code of the
// grant: a used code returns ErrCodeReused and an unknown or expired code
// returns ErrNotFound, both without calling fn. The handle is retired and
// the code is marked used in the same transaction when fn succeeds, so a
// credential is consumed only together with the tokens it issues.
//
// When fn also returns an error, only the Delete and Tokens parts of its
// change are applied, the handle stays active, the code stays unused and the
// error is returned. A failed issuance can so keep a rotated upstream refresh
// token, which the upstream refresh already consumed, without issuing a
// handle or token, and the client can retry with the same credential.
//
// Once the lock and the checks pass, fn runs with a context that is not
// canceled with ctx but ends after the store's detach bound, and the updates
// and the commit use the same context. A canceled request therefore cannot
// roll back a rotated upstream refresh token that fn already consumed.
//
// Database failures (begin, lock, checks, updates and commit) wrap
// ErrStorage so callers can tell a retryable storage outage from an error of
// fn or from a sentinel result. Errors returned by fn and the sentinels
// ErrNotFound, ErrHandleRetired and ErrCodeReused are never wrapped.
func (m *MCPOAuth) UpdateGrant(ctx context.Context, id uuid.UUID, use MCPGrantUse, fn func(ctx context.Context, g MCPGrant) (MCPGrantChange, error)) error {
	// fnErr is the error of fn when its partial change was committed.
	var fnErr error
	// logicErr is a non-database result that aborts the transaction: a
	// sentinel or an error of fn without a change to keep.
	var logicErr error
	abort := func(err error) error {
		logicErr = err
		return err
	}
	err := m.s.InTxDetached(ctx, func(tx pgx.Tx, detach func() context.Context) error {
		g, err := scanMCPGrant(tx.QueryRow(ctx, `SELECT `+mcpGrantColumns+` FROM mcp_grants WHERE id = $1 AND expire_time > now() FOR UPDATE`, id))
		if errors.Is(err, ErrNotFound) {
			return abort(err)
		}
		if err != nil {
			return storageErr(err)
		}
		// The grant row lock serializes every update of the grant, so the
		// handle or code cannot be consumed between this check and the
		// consumption below. Concurrent exchanges of one code therefore issue
		// tokens at most once; the later one sees ErrCodeReused.
		if use.Handle != nil {
			var active bool
			if err := tx.QueryRow(ctx, `SELECT EXISTS (SELECT 1 FROM mcp_refresh_handles
				WHERE handle_hash = $1 AND grant_id = $2 AND retire_time IS NULL)`, use.Handle, id).Scan(&active); err != nil {
				return storageErr(err)
			}
			if !active {
				return abort(ErrHandleRetired)
			}
		}
		if use.Code != nil {
			var used, live bool
			err := tx.QueryRow(ctx, `SELECT use_time IS NOT NULL, expire_time > now() FROM mcp_authorization_codes
				WHERE code_hash = $1 AND grant_id = $2`, use.Code, id).Scan(&used, &live)
			switch {
			case errors.Is(err, pgx.ErrNoRows):
				return abort(ErrNotFound)
			case err != nil:
				return storageErr(err)
			case used:
				return abort(ErrCodeReused)
			case !live:
				return abort(ErrNotFound)
			}
		}
		// Everything from here on belongs to one consumption of the upstream
		// refresh token: fn may rotate it at the identity provider, so fn, the
		// consumption of the code or handle and the commit must finish even
		// when the request is canceled meanwhile. The lock wait and the checks
		// above stay under the request context.
		wctx := detach()
		ch, err := fn(wctx, g)
		if err != nil {
			if !ch.Delete && ch.Tokens == nil {
				return abort(err)
			}
			fnErr = err
			return storageErr(applyMCPGrantChange(wctx, tx, id, MCPGrantChange{Delete: ch.Delete, Tokens: ch.Tokens}))
		}
		if use.Handle != nil {
			tag, err := tx.Exec(wctx, `UPDATE mcp_refresh_handles SET retire_time = now()
				WHERE handle_hash = $1 AND grant_id = $2 AND retire_time IS NULL`, use.Handle, id)
			if err != nil {
				return storageErr(err)
			}
			if tag.RowsAffected() != 1 {
				return abort(ErrHandleRetired)
			}
		}
		if use.Code != nil {
			tag, err := tx.Exec(wctx, `UPDATE mcp_authorization_codes SET use_time = now()
				WHERE code_hash = $1 AND grant_id = $2 AND use_time IS NULL`, use.Code, id)
			if err != nil {
				return storageErr(err)
			}
			if tag.RowsAffected() != 1 {
				return abort(ErrCodeReused)
			}
		}
		return storageErr(applyMCPGrantChange(wctx, tx, id, ch))
	})
	if err != nil {
		if logicErr != nil {
			return logicErr
		}
		// Whatever is left failed in the database: begin, a statement or the
		// commit.
		if errors.Is(err, ErrStorage) {
			return err
		}
		return storageErr(err)
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
