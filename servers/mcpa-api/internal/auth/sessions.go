// SPDX-License-Identifier: FSL-1.1-ALv2

package auth

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/crypto"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

// Cookie and header names of the console session.
const (
	SessionCookie = "mcpa_session"
	CSRFHeader    = "X-Mcpa-CSRF"
)

// SessionLifetime is the absolute lifetime of a console session. Tokens are
// refreshed within it; after it the user signs in again.
const SessionLifetime = 12 * time.Hour

// refreshMargin refreshes access tokens shortly before they expire.
const refreshMargin = time.Minute

// SessionTokens are the Logto tokens of a console session. They are sealed
// at rest and never leave the server.
type SessionTokens struct {
	APIAccessToken     string    `json:"api_access_token"`
	DelcoreAccessToken string    `json:"delcore_access_token"`
	DelcoreExpiry      time.Time `json:"delcore_expiry"`
	RefreshToken       string    `json:"refresh_token"`
}

// SessionState is a resolved console session.
type SessionState struct {
	Session store.Session
	Tokens  SessionTokens
}

// Sessions manages console sessions.
type Sessions struct {
	db          *store.Store
	keys        *crypto.Keyring
	logto       *Logto
	apiResource string
	delcoreRes  string
	mfaClaim    string
	reauthURL   string
	log         *slog.Logger
	now         func() time.Time
}

// SessionsConfig configures Sessions.
type SessionsConfig struct {
	Store           *store.Store
	Keys            *crypto.Keyring
	Logto           *Logto
	APIResource     string
	DelcoreResource string
	MFAClaim        string
	// ReauthURL is the MFA enrollment or reauthentication URL reported when a
	// refresh inside a request finds that the session lost MFA evidence.
	ReauthURL string
	Log       *slog.Logger
}

// NewSessions builds the console session manager.
func NewSessions(cfg SessionsConfig) *Sessions {
	return &Sessions{
		db: cfg.Store, keys: cfg.Keys, logto: cfg.Logto, apiResource: cfg.APIResource,
		delcoreRes: cfg.DelcoreResource, mfaClaim: cfg.MFAClaim, reauthURL: cfg.ReauthURL, log: cfg.Log, now: time.Now,
	}
}

// RandomToken returns 32 random bytes as unpadded base64url.
func RandomToken() (string, error) {
	b := make([]byte, 32)
	if _, err := rand.Read(b); err != nil {
		return "", err
	}
	return base64.RawURLEncoding.EncodeToString(b), nil
}

// Digest returns the SHA-256 digest of a bearer value.
func Digest(v string) []byte {
	sum := sha256.Sum256([]byte(v))
	return sum[:]
}

// NewSessionInput describes a session created after a console login.
type NewSessionInput struct {
	UserID       string
	Subject      string
	MFA          bool
	AccessExpiry time.Time
	Tokens       SessionTokens
}

// Create stores a session and returns the cookie value. A session without
// MFA evidence stores no tokens: it can only report mfa_required and log out.
func (s *Sessions) Create(ctx context.Context, in NewSessionInput) (cookie string, sess store.Session, err error) {
	cookie, err = RandomToken()
	if err != nil {
		return "", store.Session{}, err
	}
	csrf, err := RandomToken()
	if err != nil {
		return "", store.Session{}, err
	}
	if !in.MFA {
		in.Tokens = SessionTokens{}
	}
	sess = store.Session{
		ID: store.NewID(), UserID: in.UserID, Subject: in.Subject, CSRFToken: csrf, MFA: in.MFA,
		AccessExpireTime: in.AccessExpiry, ExpireTime: s.now().Add(SessionLifetime),
	}
	sess.Tokens, err = s.seal(sess, in.Tokens)
	if err != nil {
		return "", store.Session{}, err
	}
	sess, err = store.InsertSession(ctx, s.db.Pool, sess, Digest(cookie))
	if err != nil {
		return "", store.Session{}, err
	}
	return cookie, sess, nil
}

func (s *Sessions) seal(sess store.Session, t SessionTokens) (crypto.Sealed, error) {
	plain, err := json.Marshal(t)
	if err != nil {
		return crypto.Sealed{}, err
	}
	defer clear(plain)
	return s.keys.Seal(plain, crypto.SessionAAD(sess.ID.String()))
}

func (s *Sessions) open(sess store.Session) (SessionTokens, error) {
	plain, err := s.keys.Open(sess.Tokens, crypto.SessionAAD(sess.ID.String()))
	if err != nil {
		return SessionTokens{}, err
	}
	defer clear(plain)
	var t SessionTokens
	if err := json.Unmarshal(plain, &t); err != nil {
		return SessionTokens{}, err
	}
	return t, nil
}

// Lookup resolves a cookie value. It refreshes tokens that are about to
// expire. It returns an *apperr.Error for unauthenticated sessions.
func (s *Sessions) Lookup(ctx context.Context, cookie string) (*SessionState, error) {
	if cookie == "" {
		return nil, apperr.Unauthenticated("sign in to continue")
	}
	sess, err := store.SessionByTokenHash(ctx, s.db.Pool, Digest(cookie))
	if errors.Is(err, store.ErrNotFound) {
		return nil, apperr.Unauthenticated("the session expired; sign in again")
	}
	if err != nil {
		return nil, apperr.DependencyUnavailable(err)
	}
	if !sess.MFA {
		return &SessionState{Session: sess}, nil
	}
	if s.now().Add(refreshMargin).Before(sess.AccessExpireTime) {
		tokens, err := s.open(sess)
		if err != nil {
			return nil, apperr.Internal(fmt.Errorf("open session tokens: %w", err))
		}
		return &SessionState{Session: sess, Tokens: tokens}, nil
	}
	return s.Refresh(ctx, sess.ID)
}

// Stored resolves a cookie value to its stored session without opening or
// refreshing its tokens. Logout uses it so that ending a session never
// depends on Logto. It returns an *apperr.Error for unauthenticated
// sessions.
func (s *Sessions) Stored(ctx context.Context, cookie string) (store.Session, error) {
	if cookie == "" {
		return store.Session{}, apperr.Unauthenticated("sign in to continue")
	}
	sess, err := store.SessionByTokenHash(ctx, s.db.Pool, Digest(cookie))
	if errors.Is(err, store.ErrNotFound) {
		return store.Session{}, apperr.Unauthenticated("the session expired; sign in again")
	}
	if err != nil {
		return store.Session{}, apperr.DependencyUnavailable(err)
	}
	return sess, nil
}

// Refresh renews the API and Delcore tokens of a session with its refresh
// token and re-checks MFA evidence. A refreshed token without evidence
// downgrades the session so the user must reauthenticate.
func (s *Sessions) Refresh(ctx context.Context, id uuid.UUID) (*SessionState, error) {
	var out *SessionState
	// refreshErr is a refresh failure returned after the transaction commits,
	// because a rotated refresh token or a session deletion must be stored
	// despite the failure.
	var refreshErr error
	// detached is set once the upstream refresh starts.
	detached := false
	err := s.db.InTxDetached(ctx, func(tx pgx.Tx, detach func() context.Context) error {
		// The lock wait and the checks stay under the request context.
		sess, err := store.LockSession(ctx, tx, id)
		if errors.Is(err, store.ErrNotFound) {
			return apperr.Unauthenticated("the session expired; sign in again")
		}
		if err != nil {
			return apperr.DependencyUnavailable(err)
		}
		if !sess.MFA {
			out = &SessionState{Session: sess}
			return nil
		}
		tokens, err := s.open(sess)
		if err != nil {
			return apperr.Internal(fmt.Errorf("open session tokens: %w", err))
		}
		// Another request may have refreshed while this one waited for the lock.
		if s.now().Add(refreshMargin).Before(sess.AccessExpireTime) && ctx.Value(forceRefreshKey{}) == nil {
			out = &SessionState{Session: sess, Tokens: tokens}
			return nil
		}
		// From here on Logto may rotate the refresh token, which makes the
		// stored one unusable. Run the upstream calls, the writes and the
		// commit detached from the request, so a canceled request cannot
		// lose the rotated token. The work is bounded by the store's detach
		// bound.
		ctx := detach()
		detached = true
		refreshed, mfa, expiry, err := s.refreshTokens(ctx, sess.Subject, tokens)
		if err != nil {
			if errors.Is(err, ErrInvalidGrant) {
				// Commit the deletion before reporting the failure. Returning the
				// error from this callback would roll the deletion back and leave
				// the session usable until its access tokens expire.
				if err := store.DeleteSession(ctx, tx, sess.ID); err != nil {
					return apperr.DependencyUnavailable(err)
				}
				s.log.InfoContext(ctx, "console session ended because Logto rejected its refresh token", "session_id", sess.ID)
				refreshErr = apperr.Unauthenticated("the session expired; sign in again")
				return nil
			}
			var ae *apperr.Error
			if !errors.As(err, &ae) {
				ae = apperr.DependencyUnavailable(err)
			}
			if refreshed.RefreshToken == "" || refreshed.RefreshToken == tokens.RefreshToken {
				return ae
			}
			// Logto rotated the refresh token before a later step failed, so
			// the stored one is no longer valid. Store the replacement and keep
			// the current access tokens and expiry, so that the next request
			// retries the refresh instead of forcing a new sign-in.
			tokens.RefreshToken = refreshed.RefreshToken
			sealed, err := s.seal(sess, tokens)
			if err != nil {
				return apperr.Internal(err)
			}
			if err := store.UpdateSessionTokens(ctx, tx, sess.ID, sess.MFA, sess.AccessExpireTime, sealed); err != nil {
				return apperr.DependencyUnavailable(err)
			}
			s.log.WarnContext(ctx, "console session refresh failed after Logto rotated the refresh token; stored the rotated token", "session_id", sess.ID, "error", ae)
			refreshErr = ae
			return nil
		}
		if !mfa {
			refreshed = SessionTokens{}
		}
		sealed, err := s.seal(sess, refreshed)
		if err != nil {
			return apperr.Internal(err)
		}
		if err := store.UpdateSessionTokens(ctx, tx, sess.ID, mfa, expiry, sealed); err != nil {
			return apperr.DependencyUnavailable(err)
		}
		sess.MFA = mfa
		sess.AccessExpireTime = expiry
		sess.Tokens = sealed
		out = &SessionState{Session: sess, Tokens: refreshed}
		if !mfa {
			s.log.InfoContext(ctx, "console session lost MFA evidence on refresh", "session_id", sess.ID)
		}
		return nil
	})
	if detached && ctx.Err() != nil {
		// The refresh ran to completion although the request was canceled, so
		// a rotated refresh token was stored as for a live request. Log
		// without token material.
		s.log.WarnContext(ctx, "console session refresh finished after the request was canceled",
			"session_id", id, "request_cancelled", true, "ok", err == nil && refreshErr == nil)
	}
	if err != nil {
		return nil, err
	}
	if refreshErr != nil {
		return nil, refreshErr
	}
	return out, nil
}

type forceRefreshKey struct{}

// ForceRefresh makes Refresh renew tokens even when they are still valid.
func ForceRefresh(ctx context.Context) context.Context {
	return context.WithValue(ctx, forceRefreshKey{}, true)
}

// refreshTokens obtains a new API token, then a Delcore token, carrying
// forward a rotated refresh token. When a step fails after Logto rotated the
// refresh token, the returned tokens carry only that rotated refresh token so
// the caller can store it.
func (s *Sessions) refreshTokens(ctx context.Context, subject string, t SessionTokens) (SessionTokens, bool, time.Time, error) {
	if t.RefreshToken == "" {
		return SessionTokens{}, false, time.Time{}, ErrInvalidGrant
	}
	api, err := s.logto.RefreshForResource(ctx, t.RefreshToken, s.apiResource)
	if err != nil {
		return SessionTokens{}, false, time.Time{}, err
	}
	refresh := t.RefreshToken
	if api.RefreshToken != "" {
		refresh = api.RefreshToken
	}
	apiTok, err := s.logto.VerifyAccessToken(ctx, api.AccessToken, s.apiResource)
	if err != nil {
		return SessionTokens{RefreshToken: refresh}, false, time.Time{}, fmt.Errorf("%w: refreshed token: %v", ErrLogtoUnavailable, err)
	}
	if apiTok.Subject != subject {
		return SessionTokens{}, false, time.Time{}, ErrInvalidGrant
	}
	dc, err := s.logto.RefreshForResource(ctx, refresh, s.delcoreRes)
	if err != nil {
		return SessionTokens{RefreshToken: refresh}, false, time.Time{}, err
	}
	if dc.RefreshToken != "" {
		refresh = dc.RefreshToken
	}
	out := SessionTokens{
		APIAccessToken:     api.AccessToken,
		DelcoreAccessToken: dc.AccessToken,
		DelcoreExpiry:      s.now().Add(time.Duration(dc.ExpiresIn) * time.Second),
		RefreshToken:       refresh,
	}
	return out, MFAEvidence(apiTok.Claims, s.mfaClaim), apiTok.Expiry, nil
}

// Delete removes a session.
func (s *Sessions) Delete(ctx context.Context, id uuid.UUID) error {
	return store.DeleteSession(ctx, s.db.Pool, id)
}

// CheckCSRF compares the presented CSRF header with the session token in
// constant time.
func CheckCSRF(presented, expected string) bool {
	if presented == "" || expected == "" {
		return false
	}
	return subtle.ConstantTimeCompare([]byte(presented), []byte(expected)) == 1
}

// CheckOrigin admits a request whose Origin header is absent or equals the
// console origin.
func CheckOrigin(origin, webOrigin string) bool {
	return origin == "" || origin == webOrigin
}

// principalFromSession builds a console principal.
func (s *Sessions) principalFromSession(st *SessionState) *Principal {
	sessionID := st.Session.ID
	p := NewUserPrincipal(mcpav1.ActorKind_ACTOR_KIND_CONSOLE, st.Session.UserID, st.Session.Subject, func(ctx context.Context) (string, error) {
		// Tokens were refreshed on lookup; refresh again only when the Delcore
		// token itself is about to expire.
		if s.now().Add(refreshMargin).Before(st.Tokens.DelcoreExpiry) {
			return st.Tokens.DelcoreAccessToken, nil
		}
		fresh, err := s.Refresh(ForceRefresh(ctx), sessionID)
		if err != nil {
			return "", err
		}
		if !fresh.Session.MFA {
			// The refresh stored the downgrade, so later requests are rejected
			// by cookie authentication. Report the same MFA error to this
			// request instead of an unauthenticated one, so the console shows
			// the reauthentication prompt and no data is served.
			return "", apperr.MFARequired(s.reauthURL)
		}
		return fresh.Tokens.DelcoreAccessToken, nil
	})
	p.SessionID = sessionID
	return p
}
