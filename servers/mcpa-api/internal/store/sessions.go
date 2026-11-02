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

// Session is a console browser session. Tokens holds the sealed Logto
// tokens; open it with crypto.SessionAAD(ID).
type Session struct {
	ID               uuid.UUID
	UserID           string
	Subject          string
	CSRFToken        string
	MFA              bool
	AccessExpireTime time.Time
	CreateTime       time.Time
	UpdateTime       time.Time
	ExpireTime       time.Time
	Tokens           crypto.Sealed
}

const sessionColumns = `id, user_id, subject, csrf_token, mfa, access_expire_time, create_time, update_time, expire_time,
	key_id, wrapped_key, key_nonce, nonce, ciphertext`

func scanSession(row pgx.Row) (Session, error) {
	var s Session
	err := row.Scan(&s.ID, &s.UserID, &s.Subject, &s.CSRFToken, &s.MFA, &s.AccessExpireTime, &s.CreateTime, &s.UpdateTime, &s.ExpireTime,
		&s.Tokens.KeyID, &s.Tokens.WrappedKey, &s.Tokens.KeyNonce, &s.Tokens.Nonce, &s.Tokens.Ciphertext)
	if errors.Is(err, pgx.ErrNoRows) {
		return Session{}, ErrNotFound
	}
	return s, err
}

// InsertSession stores a session keyed by the digest of its cookie value.
func InsertSession(ctx context.Context, db DB, s Session, tokenHash []byte) (Session, error) {
	err := db.QueryRow(ctx, `INSERT INTO console_sessions (id, token_hash, user_id, subject, csrf_token, mfa, access_expire_time, expire_time,
			key_id, wrapped_key, key_nonce, nonce, ciphertext)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) RETURNING create_time, update_time`,
		s.ID, tokenHash, s.UserID, s.Subject, s.CSRFToken, s.MFA, s.AccessExpireTime, s.ExpireTime,
		s.Tokens.KeyID, s.Tokens.WrappedKey, s.Tokens.KeyNonce, s.Tokens.Nonce, s.Tokens.Ciphertext).Scan(&s.CreateTime, &s.UpdateTime)
	return s, err
}

// SessionByTokenHash returns an unexpired session.
func SessionByTokenHash(ctx context.Context, db DB, tokenHash []byte) (Session, error) {
	return scanSession(db.QueryRow(ctx, `SELECT `+sessionColumns+` FROM console_sessions WHERE token_hash = $1 AND expire_time > now()`, tokenHash))
}

// LockSession locks an unexpired session for a token refresh.
func LockSession(ctx context.Context, tx pgx.Tx, id uuid.UUID) (Session, error) {
	return scanSession(tx.QueryRow(ctx, `SELECT `+sessionColumns+` FROM console_sessions WHERE id = $1 AND expire_time > now() FOR UPDATE`, id))
}

// UpdateSessionTokens stores refreshed tokens.
func UpdateSessionTokens(ctx context.Context, db DB, id uuid.UUID, mfa bool, accessExpire time.Time, tokens crypto.Sealed) error {
	_, err := db.Exec(ctx, `UPDATE console_sessions SET mfa = $2, access_expire_time = $3, key_id = $4, wrapped_key = $5, key_nonce = $6,
		nonce = $7, ciphertext = $8, update_time = now() WHERE id = $1`,
		id, mfa, accessExpire, tokens.KeyID, tokens.WrappedKey, tokens.KeyNonce, tokens.Nonce, tokens.Ciphertext)
	return err
}

// DeleteSession removes a session.
func DeleteSession(ctx context.Context, db DB, id uuid.UUID) error {
	_, err := db.Exec(ctx, `DELETE FROM console_sessions WHERE id = $1`, id)
	return err
}

// LoginFlow is a pending console login.
type LoginFlow struct {
	BrowserHash  []byte
	CodeVerifier string
	Nonce        string
	ReturnTo     string
	ExpireTime   time.Time
}

// InsertLoginFlow stores a pending login keyed by the digest of its state.
func InsertLoginFlow(ctx context.Context, db DB, stateHash []byte, f LoginFlow) error {
	_, err := db.Exec(ctx, `INSERT INTO console_login_flows (state_hash, browser_hash, code_verifier, nonce, return_to, expire_time)
		VALUES ($1, $2, $3, $4, $5, $6)`, stateHash, f.BrowserHash, f.CodeVerifier, f.Nonce, f.ReturnTo, f.ExpireTime)
	return err
}

// TakeLoginFlow deletes and returns an unexpired pending login. A state can
// therefore be redeemed only once.
func TakeLoginFlow(ctx context.Context, db DB, stateHash []byte) (LoginFlow, error) {
	var f LoginFlow
	err := db.QueryRow(ctx, `DELETE FROM console_login_flows WHERE state_hash = $1
		RETURNING browser_hash, code_verifier, nonce, return_to, expire_time`, stateHash).
		Scan(&f.BrowserHash, &f.CodeVerifier, &f.Nonce, &f.ReturnTo, &f.ExpireTime)
	if errors.Is(err, pgx.ErrNoRows) {
		return LoginFlow{}, ErrNotFound
	}
	if err != nil {
		return LoginFlow{}, err
	}
	if !f.ExpireTime.After(time.Now()) {
		return LoginFlow{}, ErrNotFound
	}
	return f, nil
}

// DeleteExpiredConsoleState removes expired sessions and login flows.
func DeleteExpiredConsoleState(ctx context.Context, db DB) error {
	if _, err := db.Exec(ctx, `DELETE FROM console_sessions WHERE expire_time <= now()`); err != nil {
		return err
	}
	_, err := db.Exec(ctx, `DELETE FROM console_login_flows WHERE expire_time <= now()`)
	return err
}
