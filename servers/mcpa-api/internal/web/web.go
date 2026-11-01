// SPDX-License-Identifier: FSL-1.1-ALv2

// Package web serves the console BFF endpoints under /auth/.
//
// Login uses the Logto authorization code flow with PKCE (S256), state and
// nonce. The state is bound to the browser with a short-lived cookie and
// return_to must exactly equal a configured console URL at login and again
// at callback. The callback obtains a token for the mcpa API resource and,
// with the refresh token, a token for the Delcore resource; both stay on
// the server. MFA evidence is checked at session creation and refresh.
package web

import (
	"context"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"encoding/json"
	"errors"
	"log/slog"
	"net/http"
	"net/url"
	"slices"
	"time"

	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
)

// Cookie and timing constants of the login flow.
const (
	loginCookie   = "mcpa_login"
	callbackPath  = "/auth/callback"
	loginLifetime = 10 * time.Minute
	loginScopes   = "openid offline_access profile"
)

// UserReader resolves the Delcore user of a Delcore-audience token.
type UserReader interface {
	GetCurrentUser(ctx context.Context, userToken string) (delcore.CurrentUser, error)
}

// Config configures the handler.
type Config struct {
	Store           *store.Store
	Logto           *auth.Logto
	Authenticator   *auth.Authenticator
	Delcore         UserReader
	PublicURL       string
	ReturnURLs      []string
	APIResource     string
	DelcoreResource string
	MFAClaim        string
	InsecureCookies bool
	Log             *slog.Logger
}

// Handler serves /auth/*.
type Handler struct {
	cfg Config
	now func() time.Time
}

// New builds the handler.
func New(cfg Config) *Handler { return &Handler{cfg: cfg, now: time.Now} }

// Register adds the /auth/ routes to mux.
func (h *Handler) Register(mux *http.ServeMux, wrap func(http.Handler) http.Handler) {
	mux.Handle("GET /auth/login", wrap(http.HandlerFunc(h.login)))
	mux.Handle("GET "+callbackPath, wrap(http.HandlerFunc(h.callback)))
	mux.Handle("GET /auth/session", wrap(http.HandlerFunc(h.session)))
	mux.Handle("POST /auth/refresh", wrap(http.HandlerFunc(h.refresh)))
	mux.Handle("POST /auth/logout", wrap(http.HandlerFunc(h.logout)))
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

// WriteError renders an *apperr.Error as the /auth JSON error shape.
func WriteError(w http.ResponseWriter, err error) {
	e := apperr.As(err)
	switch e.Reason {
	case mcpav1.ErrorReason_ERROR_REASON_MFA_REQUIRED:
		writeJSON(w, http.StatusForbidden, map[string]string{"error": "mfa_required", "reauthentication_url": e.ReauthURL})
	case mcpav1.ErrorReason_ERROR_REASON_UNAUTHENTICATED:
		writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "unauthenticated"})
	case mcpav1.ErrorReason_ERROR_REASON_PERMISSION_DENIED:
		writeJSON(w, http.StatusForbidden, map[string]string{"error": "forbidden"})
	case mcpav1.ErrorReason_ERROR_REASON_INVALID_INPUT:
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid_request"})
	case mcpav1.ErrorReason_ERROR_REASON_DEPENDENCY_UNAVAILABLE:
		writeJSON(w, http.StatusServiceUnavailable, map[string]string{"error": "dependency_unavailable"})
	default:
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": "internal"})
	}
}

func (h *Handler) redirectURI() string { return h.cfg.PublicURL + callbackPath }

func pkceChallenge(verifier string) string {
	sum := sha256.Sum256([]byte(verifier))
	return base64.RawURLEncoding.EncodeToString(sum[:])
}

func (h *Handler) setCookie(w http.ResponseWriter, c *http.Cookie) {
	c.HttpOnly = true
	c.Secure = !h.cfg.InsecureCookies
	c.SameSite = http.SameSiteLaxMode
	http.SetCookie(w, c)
}

// login starts a Logto authorization code flow.
func (h *Handler) login(w http.ResponseWriter, r *http.Request) {
	ctx := r.Context()
	returnTo := r.URL.Query().Get("return_to")
	if !slices.Contains(h.cfg.ReturnURLs, returnTo) {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid_return_to"})
		return
	}
	authEndpoint, _, _, err := h.cfg.Logto.Endpoints(ctx)
	if err != nil {
		h.cfg.Log.WarnContext(ctx, "logto discovery failed", "error", err)
		WriteError(w, apperr.DependencyUnavailable(err))
		return
	}
	state, err1 := auth.RandomToken()
	verifier, err2 := auth.RandomToken()
	nonce, err3 := auth.RandomToken()
	browser, err4 := auth.RandomToken()
	if err := errors.Join(err1, err2, err3, err4); err != nil {
		WriteError(w, apperr.Internal(err))
		return
	}
	expiry := h.now().Add(loginLifetime)
	if err := store.InsertLoginFlow(ctx, h.cfg.Store.Pool, auth.Digest(state), store.LoginFlow{
		BrowserHash: auth.Digest(browser), CodeVerifier: verifier, Nonce: nonce, ReturnTo: returnTo, ExpireTime: expiry,
	}); err != nil {
		WriteError(w, apperr.DependencyUnavailable(err))
		return
	}
	h.setCookie(w, &http.Cookie{Name: loginCookie, Value: browser, Path: callbackPath, MaxAge: int(loginLifetime.Seconds())})
	q := url.Values{
		"client_id":             {h.cfg.Logto.ClientID()},
		"redirect_uri":          {h.redirectURI()},
		"response_type":         {"code"},
		"scope":                 {loginScopes},
		"state":                 {state},
		"nonce":                 {nonce},
		"code_challenge":        {pkceChallenge(verifier)},
		"code_challenge_method": {"S256"},
		// Logto issues refresh tokens for offline_access only with consent.
		"prompt":   {"consent"},
		"resource": {h.cfg.APIResource, h.cfg.DelcoreResource},
	}
	w.Header().Set("Cache-Control", "no-store")
	http.Redirect(w, r, authEndpoint+"?"+q.Encode(), http.StatusFound)
}

// callback completes the login and creates the session.
func (h *Handler) callback(w http.ResponseWriter, r *http.Request) {
	ctx := r.Context()
	q := r.URL.Query()
	state := q.Get("state")
	if state == "" {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid_state"})
		return
	}
	flow, err := store.TakeLoginFlow(ctx, h.cfg.Store.Pool, auth.Digest(state))
	if errors.Is(err, store.ErrNotFound) {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid_state"})
		return
	}
	if err != nil {
		WriteError(w, apperr.DependencyUnavailable(err))
		return
	}
	browser, cerr := r.Cookie(loginCookie)
	h.setCookie(w, &http.Cookie{Name: loginCookie, Value: "", Path: callbackPath, MaxAge: -1})
	if cerr != nil || subtle.ConstantTimeCompare(auth.Digest(browser.Value), flow.BrowserHash) != 1 {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid_state"})
		return
	}
	// Re-check the saved return URL against the current configuration.
	if !slices.Contains(h.cfg.ReturnURLs, flow.ReturnTo) {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid_return_to"})
		return
	}
	if q.Get("error") != "" || q.Get("code") == "" {
		h.redirectWithError(w, r, flow.ReturnTo, "login_failed")
		return
	}
	in, err := h.completeLogin(ctx, q.Get("code"), flow)
	if err != nil {
		h.cfg.Log.WarnContext(ctx, "console login failed", "error", err)
		h.redirectWithError(w, r, flow.ReturnTo, "login_failed")
		return
	}
	cookie, sess, err := h.cfg.Authenticator.Sessions().Create(ctx, *in)
	if err != nil {
		h.cfg.Log.ErrorContext(ctx, "create console session failed", "error", err)
		h.redirectWithError(w, r, flow.ReturnTo, "login_failed")
		return
	}
	h.setCookie(w, &http.Cookie{Name: auth.SessionCookie, Value: cookie, Path: "/", Expires: sess.ExpireTime})
	h.cfg.Log.InfoContext(ctx, "console session created", "session_id", sess.ID, "mfa", sess.MFA)
	w.Header().Set("Cache-Control", "no-store")
	http.Redirect(w, r, flow.ReturnTo, http.StatusFound)
}

func (h *Handler) completeLogin(ctx context.Context, code string, flow store.LoginFlow) (*auth.NewSessionInput, error) {
	logto := h.cfg.Logto
	tr, err := logto.ExchangeCode(ctx, code, flow.CodeVerifier, h.redirectURI(), h.cfg.APIResource)
	if err != nil {
		return nil, err
	}
	if tr.IDToken == "" || tr.RefreshToken == "" {
		return nil, errors.New("token response lacks an ID token or refresh token")
	}
	idTok, err := logto.VerifyIDToken(ctx, tr.IDToken, flow.Nonce)
	if err != nil {
		return nil, err
	}
	apiTok, err := logto.VerifyAccessToken(ctx, tr.AccessToken, h.cfg.APIResource)
	if err != nil {
		return nil, err
	}
	if apiTok.Subject != idTok.Subject {
		return nil, errors.New("access token subject does not match the ID token")
	}
	refresh := tr.RefreshToken
	dc, err := logto.RefreshForResource(ctx, refresh, h.cfg.DelcoreResource)
	if err != nil {
		return nil, err
	}
	if dc.RefreshToken != "" {
		refresh = dc.RefreshToken
	}
	user, err := h.cfg.Delcore.GetCurrentUser(ctx, dc.AccessToken)
	if err != nil {
		return nil, err
	}
	return &auth.NewSessionInput{
		UserID: user.UserID, Subject: idTok.Subject, MFA: auth.MFAEvidence(apiTok.Claims, h.cfg.MFAClaim), AccessExpiry: apiTok.Expiry,
		Tokens: auth.SessionTokens{
			APIAccessToken: tr.AccessToken, DelcoreAccessToken: dc.AccessToken, RefreshToken: refresh,
			DelcoreExpiry: h.now().Add(time.Duration(dc.ExpiresIn) * time.Second),
		},
	}, nil
}

func (h *Handler) redirectWithError(w http.ResponseWriter, r *http.Request, returnTo, code string) {
	w.Header().Set("Cache-Control", "no-store")
	http.Redirect(w, r, returnTo+"?"+url.Values{"auth_error": {code}}.Encode(), http.StatusFound)
}

type sessionBody struct {
	UserID    string `json:"user_id"`
	CSRFToken string `json:"csrf_token"`
	ExpiresAt string `json:"expires_at"`
}

func (h *Handler) writeSession(w http.ResponseWriter, st *auth.SessionState) {
	if !st.Session.MFA {
		WriteError(w, apperr.MFARequired(h.cfg.Authenticator.ReauthURL()))
		return
	}
	writeJSON(w, http.StatusOK, sessionBody{
		UserID: st.Session.UserID, CSRFToken: st.Session.CSRFToken, ExpiresAt: st.Session.ExpireTime.UTC().Format(time.RFC3339),
	})
}

// session reports the current session.
func (h *Handler) session(w http.ResponseWriter, r *http.Request) {
	st, err := h.cfg.Authenticator.SessionFromHeader(r.Context(), r.Header)
	if err != nil {
		WriteError(w, err)
		return
	}
	h.writeSession(w, st)
}

// mutationSession resolves the session of a POST and checks Origin and CSRF.
// It refreshes tokens that are about to expire.
func (h *Handler) mutationSession(r *http.Request) (*auth.SessionState, error) {
	if !auth.CheckOrigin(r.Header.Get("Origin"), h.cfg.Authenticator.WebOrigin()) {
		return nil, apperr.PermissionDenied("origin is not allowed")
	}
	st, err := h.cfg.Authenticator.SessionFromHeader(r.Context(), r.Header)
	if err != nil {
		return nil, err
	}
	if !auth.CheckCSRF(r.Header.Get(auth.CSRFHeader), st.Session.CSRFToken) {
		return nil, apperr.PermissionDenied("missing or invalid CSRF token")
	}
	return st, nil
}

// storedMutationSession is mutationSession without the token refresh, so
// the request does not depend on Logto.
func (h *Handler) storedMutationSession(r *http.Request) (store.Session, error) {
	if !auth.CheckOrigin(r.Header.Get("Origin"), h.cfg.Authenticator.WebOrigin()) {
		return store.Session{}, apperr.PermissionDenied("origin is not allowed")
	}
	sess, err := h.cfg.Authenticator.StoredSessionFromHeader(r.Context(), r.Header)
	if err != nil {
		return store.Session{}, err
	}
	if !auth.CheckCSRF(r.Header.Get(auth.CSRFHeader), sess.CSRFToken) {
		return store.Session{}, apperr.PermissionDenied("missing or invalid CSRF token")
	}
	return sess, nil
}

// refresh renews the session tokens and re-checks MFA evidence.
func (h *Handler) refresh(w http.ResponseWriter, r *http.Request) {
	st, err := h.mutationSession(r)
	if err != nil {
		WriteError(w, err)
		return
	}
	if !st.Session.MFA {
		WriteError(w, apperr.MFARequired(h.cfg.Authenticator.ReauthURL()))
		return
	}
	fresh, err := h.cfg.Authenticator.Sessions().Refresh(auth.ForceRefresh(r.Context()), st.Session.ID)
	if err != nil {
		if apperr.As(err).Reason == mcpav1.ErrorReason_ERROR_REASON_UNAUTHENTICATED {
			h.clearSession(w)
		}
		WriteError(w, err)
		return
	}
	h.writeSession(w, fresh)
}

func (h *Handler) clearSession(w http.ResponseWriter) {
	h.setCookie(w, &http.Cookie{Name: auth.SessionCookie, Value: "", Path: "/", MaxAge: -1})
}

// logout deletes the session. It resolves the stored session without
// refreshing tokens, so signing out works while Logto is unavailable.
func (h *Handler) logout(w http.ResponseWriter, r *http.Request) {
	sess, err := h.storedMutationSession(r)
	if err != nil {
		WriteError(w, err)
		return
	}
	if err := h.cfg.Authenticator.Sessions().Delete(r.Context(), sess.ID); err != nil {
		WriteError(w, apperr.DependencyUnavailable(err))
		return
	}
	h.clearSession(w)
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(http.StatusNoContent)
}
