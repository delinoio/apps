// SPDX-License-Identifier: FSL-1.1-ALv2

package oauth

import (
	"context"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"mime"
	"net/http"
	"net/url"
	"slices"
	"time"

	"github.com/google/uuid"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

// OAuth error codes (RFC 6749, RFC 7009 and RFC 8707).
type errorCode string

const (
	errInvalidRequest          errorCode = "invalid_request"
	errInvalidClient           errorCode = "invalid_client"
	errInvalidGrant            errorCode = "invalid_grant"
	errUnauthorizedClient      errorCode = "unauthorized_client"
	errUnsupportedGrantType    errorCode = "unsupported_grant_type"
	errUnsupportedResponseType errorCode = "unsupported_response_type"
	errInvalidTarget           errorCode = "invalid_target"
	errAccessDenied            errorCode = "access_denied"
	errServerError             errorCode = "server_error"
	errTemporarilyUnavailable  errorCode = "temporarily_unavailable"
)

// oauthError is an error response of the token and revocation endpoints.
type oauthError struct {
	code        errorCode
	description string
	uri         string
}

func (e *oauthError) Error() string { return string(e.code) + ": " + e.description }

func newOAuthError(code errorCode, description string) *oauthError {
	return &oauthError{code: code, description: description}
}

func (e *oauthError) status() int {
	switch e.code {
	case errInvalidClient:
		return http.StatusUnauthorized
	case errServerError:
		return http.StatusInternalServerError
	case errTemporarilyUnavailable:
		return http.StatusServiceUnavailable
	default:
		return http.StatusBadRequest
	}
}

func writeOAuthError(w http.ResponseWriter, e *oauthError) {
	body := map[string]string{"error": string(e.code)}
	if e.description != "" {
		body["error_description"] = e.description
	}
	if e.uri != "" {
		body["error_uri"] = e.uri
	}
	if e.code == errInvalidClient {
		w.Header().Set("WWW-Authenticate", `Basic realm="mcpa-oauth"`)
	}
	writeJSON(w, e.status(), body)
}

// pkceChallenge returns the S256 challenge of a verifier.
func pkceChallenge(verifier string) string {
	sum := sha256.Sum256([]byte(verifier))
	return base64.RawURLEncoding.EncodeToString(sum[:])
}

// maxClientStateLength is the longest client state the broker accepts.
const maxClientStateLength = 512

// validClientState reports whether state is 1 to 512 bytes, all printable
// ASCII (%x20-7E). The value is stored as text and echoed in redirects, so
// NUL bytes, control characters and invalid UTF-8 are refused up front
// instead of failing the insert later.
func validClientState(state string) bool {
	if len(state) == 0 || len(state) > maxClientStateLength {
		return false
	}
	for i := 0; i < len(state); i++ {
		if c := state[i]; c < 0x20 || c > 0x7e {
			return false
		}
	}
	return true
}

// validPKCEValue reports whether v is a 43 to 128 character PKCE verifier
// (RFC 7636 section 4.1). S256 challenges are always 43 characters.
func validPKCEValue(v string) bool {
	if len(v) < 43 || len(v) > 128 {
		return false
	}
	for i := 0; i < len(v); i++ {
		c := v[i]
		if !(c >= 'A' && c <= 'Z' || c >= 'a' && c <= 'z' || c >= '0' && c <= '9' || c == '-' || c == '.' || c == '_' || c == '~') {
			return false
		}
	}
	return true
}

// VerifyPKCE checks a verifier against an S256 challenge in constant time.
func VerifyPKCE(verifier, challenge string) bool {
	if !validPKCEValue(verifier) || len(challenge) != 43 {
		return false
	}
	return subtle.ConstantTimeCompare([]byte(pkceChallenge(verifier)), []byte(challenge)) == 1
}

// singleValues returns the named parameters and reports whether any of them
// was repeated, which RFC 6749 forbids.
func singleValues(v url.Values, names ...string) (map[string]string, bool) {
	out := make(map[string]string, len(names))
	for _, n := range names {
		if len(v[n]) > 1 {
			return nil, false
		}
		out[n] = v.Get(n)
	}
	return out, true
}

// metadataHandler serves public JSON metadata. It allows any origin without
// credentials so browser-based clients can discover the broker; the
// documents contain no secrets.
func metadataHandler(doc func() map[string]any) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		h := w.Header()
		h.Set("Access-Control-Allow-Origin", "*")
		switch r.Method {
		case http.MethodGet, http.MethodHead:
		case http.MethodOptions:
			h.Set("Access-Control-Allow-Methods", "GET, OPTIONS")
			h.Set("Access-Control-Allow-Headers", "Content-Type, MCP-Protocol-Version")
			w.WriteHeader(http.StatusNoContent)
			return
		default:
			h.Set("Allow", "GET, HEAD, OPTIONS")
			writeJSON(w, http.StatusMethodNotAllowed, map[string]string{"error": "method_not_allowed"})
			return
		}
		h.Set("Content-Type", "application/json")
		h.Set("Cache-Control", "public, max-age=300")
		w.WriteHeader(http.StatusOK)
		if r.Method == http.MethodGet {
			_ = json.NewEncoder(w).Encode(doc())
		}
	})
}

// protectedResourceMetadata is the RFC 9728 document of /mcp.
func (b *Broker) protectedResourceMetadata() map[string]any {
	return map[string]any{
		"resource":                 b.cfg.MCPResource,
		"authorization_servers":    []string{b.cfg.PublicURL},
		"bearer_methods_supported": []string{"header"},
		"resource_name":            "Managed CPA",
	}
}

// authorizationServerMetadata is the RFC 8414 document of the broker.
func (b *Broker) authorizationServerMetadata() map[string]any {
	return map[string]any{
		"issuer":                                         b.cfg.PublicURL,
		"authorization_endpoint":                         b.cfg.PublicURL + AuthorizePath,
		"token_endpoint":                                 b.cfg.PublicURL + TokenPath,
		"revocation_endpoint":                            b.cfg.PublicURL + RevokePath,
		"response_types_supported":                       []string{"code"},
		"response_modes_supported":                       []string{"query"},
		"grant_types_supported":                          []string{"authorization_code", "refresh_token"},
		"code_challenge_methods_supported":               []string{"S256"},
		"token_endpoint_auth_methods_supported":          []string{"none"},
		"revocation_endpoint_auth_methods_supported":     []string{"none"},
		"authorization_response_iss_parameter_supported": true,
		"client_id_metadata_document_supported":          false,
	}
}

func (b *Broker) callbackURI() string { return b.cfg.PublicURL + CallbackPath }

func (b *Broker) setCookie(w http.ResponseWriter, c *http.Cookie) {
	c.HttpOnly = true
	c.Secure = !b.cfg.InsecureCookies
	c.SameSite = http.SameSiteLaxMode
	http.SetCookie(w, c)
}

// redirectToClient sends the browser to a registered redirect URI with
// params added to its query.
func (b *Broker) redirectToClient(w http.ResponseWriter, r *http.Request, redirectURI string, params url.Values) {
	u, err := url.Parse(redirectURI)
	if err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid_redirect_uri"})
		return
	}
	q := u.Query()
	for k, vs := range params {
		q[k] = vs
	}
	q.Set("iss", b.cfg.PublicURL)
	u.RawQuery = q.Encode()
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("Referrer-Policy", "no-referrer")
	http.Redirect(w, r, u.String(), http.StatusFound)
}

func (b *Broker) redirectError(w http.ResponseWriter, r *http.Request, redirectURI, state string, e *oauthError) {
	params := url.Values{"error": {string(e.code)}}
	if e.description != "" {
		params.Set("error_description", e.description)
	}
	if e.uri != "" {
		params.Set("error_uri", e.uri)
	}
	if state != "" {
		params.Set("state", state)
	}
	b.redirectToClient(w, r, redirectURI, params)
}

// registeredRedirect reports whether redirectURI exactly equals a registered
// redirect URI of clientID.
func (b *Broker) registeredRedirect(clientID, redirectURI string) bool {
	c, ok := b.cfg.Clients[clientID]
	return ok && redirectURI != "" && slices.Contains(c.RedirectURIs, redirectURI)
}

// authorize validates an MCP client authorization request and starts the
// upstream Logto login. Requests with an unknown client or an unregistered
// redirect URI are answered directly and never redirected.
func (b *Broker) authorize(w http.ResponseWriter, r *http.Request) {
	ctx := r.Context()
	p, ok := singleValues(r.URL.Query(), "client_id", "redirect_uri", "response_type", "state",
		"code_challenge", "code_challenge_method", "resource")
	if !ok {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": string(errInvalidRequest), "error_description": "a parameter is repeated"})
		return
	}
	clientID, redirectURI, state := p["client_id"], p["redirect_uri"], p["state"]
	if _, ok := b.cfg.Clients[clientID]; !ok {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": string(errInvalidClient), "error_description": "unknown client_id"})
		return
	}
	if !b.registeredRedirect(clientID, redirectURI) {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": string(errInvalidRequest), "error_description": "redirect_uri is not registered for this client"})
		return
	}
	// A non-empty state that cannot be stored or echoed safely is refused
	// directly: it is never redirected back, so it is not reflected to the
	// registered redirect URI. An empty state keeps its OAuth error redirect
	// below.
	if state != "" && !validClientState(state) {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": string(errInvalidRequest), "error_description": "state is invalid"})
		return
	}
	switch {
	case p["response_type"] != "code":
		b.redirectError(w, r, redirectURI, state, newOAuthError(errUnsupportedResponseType, "response_type must be code"))
		return
	case state == "":
		b.redirectError(w, r, redirectURI, state, newOAuthError(errInvalidRequest, "state is required"))
		return
	case p["code_challenge_method"] != "S256" || len(p["code_challenge"]) != 43 || !validPKCEValue(p["code_challenge"]):
		b.redirectError(w, r, redirectURI, state, newOAuthError(errInvalidRequest, "an S256 code_challenge is required"))
		return
	case p["resource"] != "" && p["resource"] != b.cfg.MCPResource:
		b.redirectError(w, r, redirectURI, state, newOAuthError(errInvalidTarget, "resource must be the Managed CPA MCP resource"))
		return
	}
	authEndpoint, _, _, err := b.cfg.Logto.Endpoints(ctx)
	if err != nil {
		b.cfg.Log.WarnContext(ctx, "logto discovery failed", "error", err)
		b.redirectError(w, r, redirectURI, state, newOAuthError(errTemporarilyUnavailable, "the identity provider is unavailable; retry later"))
		return
	}
	upstreamState, err1 := auth.RandomToken()
	verifier, err2 := auth.RandomToken()
	nonce, err3 := auth.RandomToken()
	browser, err4 := auth.RandomToken()
	if err := errors.Join(err1, err2, err3, err4); err != nil {
		b.redirectError(w, r, redirectURI, state, newOAuthError(errServerError, ""))
		return
	}
	if err := b.cfg.Store.InsertAuthRequest(ctx, store.MCPAuthRequest{
		StateHash: auth.Digest(upstreamState), BrowserHash: auth.Digest(browser), ClientID: clientID, RedirectURI: redirectURI,
		ClientState: state, CodeChallenge: p["code_challenge"], CodeVerifier: verifier, Nonce: nonce,
		ExpireTime: b.now().Add(RequestLifetime),
	}); err != nil {
		b.cfg.Log.ErrorContext(ctx, "store mcp authorization request failed", "error", err)
		b.redirectError(w, r, redirectURI, state, newOAuthError(errTemporarilyUnavailable, "retry later"))
		return
	}
	b.setCookie(w, &http.Cookie{Name: browserCookie, Value: browser, Path: CallbackPath, MaxAge: int(RequestLifetime.Seconds())})
	q := url.Values{
		"client_id":             {b.cfg.Logto.ClientID()},
		"redirect_uri":          {b.callbackURI()},
		"response_type":         {"code"},
		"scope":                 {loginScopes},
		"state":                 {upstreamState},
		"nonce":                 {nonce},
		"code_challenge":        {pkceChallenge(verifier)},
		"code_challenge_method": {"S256"},
		// Logto issues refresh tokens for offline_access only with consent.
		"prompt":   {"consent"},
		"resource": {b.cfg.MCPResource, b.cfg.DelcoreResource},
	}
	b.cfg.Log.InfoContext(ctx, "mcp authorization started", "client_id", clientID)
	w.Header().Set("Cache-Control", "no-store")
	http.Redirect(w, r, authEndpoint+"?"+q.Encode(), http.StatusFound)
}

// callback completes the upstream login, creates the grant and redirects to
// the client with a one-use code.
func (b *Broker) callback(w http.ResponseWriter, r *http.Request) {
	ctx := r.Context()
	q := r.URL.Query()
	upstreamState := q.Get("state")
	if upstreamState == "" {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid_state"})
		return
	}
	req, err := b.cfg.Store.TakeAuthRequest(ctx, auth.Digest(upstreamState))
	if errors.Is(err, store.ErrNotFound) {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid_state"})
		return
	}
	if err != nil {
		b.cfg.Log.ErrorContext(ctx, "read mcp authorization request failed", "error", err)
		writeJSON(w, http.StatusServiceUnavailable, map[string]string{"error": "temporarily_unavailable"})
		return
	}
	browser, cerr := r.Cookie(browserCookie)
	b.setCookie(w, &http.Cookie{Name: browserCookie, Value: "", Path: CallbackPath, MaxAge: -1})
	if cerr != nil || subtle.ConstantTimeCompare(auth.Digest(browser.Value), req.BrowserHash) != 1 {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid_state"})
		return
	}
	// Revalidate the registration against the current configuration.
	if !b.registeredRedirect(req.ClientID, req.RedirectURI) {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": string(errInvalidClient)})
		return
	}
	if upstreamErr := q.Get("error"); upstreamErr != "" || q.Get("code") == "" {
		code := errServerError
		if upstreamErr == "access_denied" {
			code = errAccessDenied
		}
		b.cfg.Log.InfoContext(ctx, "mcp authorization ended without a code", "client_id", req.ClientID, "upstream_error", boundedOAuthError(upstreamErr))
		b.redirectError(w, r, req.RedirectURI, req.ClientState, newOAuthError(code, "the sign-in was not completed"))
		return
	}
	grant, err := b.completeLogin(ctx, q.Get("code"), req)
	if err != nil {
		e := newOAuthError(errServerError, "the sign-in could not be completed")
		switch {
		case isMFA(err):
			e = &oauthError{code: errAccessDenied, description: "multi-factor authentication is required; enroll or sign in again", uri: b.cfg.ReauthURL}
		case errors.Is(err, auth.ErrLogtoUnavailable) || apperr.As(err).Reason == mcpav1.ErrorReason_ERROR_REASON_DEPENDENCY_UNAVAILABLE:
			e = newOAuthError(errTemporarilyUnavailable, "a dependency is unavailable; retry later")
		}
		b.cfg.Log.WarnContext(ctx, "mcp authorization failed", "client_id", req.ClientID, "oauth_error", string(e.code), "error", err)
		b.redirectError(w, r, req.RedirectURI, req.ClientState, e)
		return
	}
	code, err := auth.RandomToken()
	if err != nil {
		b.redirectError(w, r, req.RedirectURI, req.ClientState, newOAuthError(errServerError, ""))
		return
	}
	// Refresh the group snapshots before the code exists so that a slow
	// Delcore cannot spend the code's lifetime before the client receives it.
	// The refresh is bounded as a whole and never fails the login.
	b.refreshSnapshots(ctx, grant)
	if err := b.cfg.Store.InsertGrant(ctx, grant.grant, store.MCPCode{
		CodeHash: auth.Digest(code), ClientID: req.ClientID, RedirectURI: req.RedirectURI,
		CodeChallenge: req.CodeChallenge, ExpireTime: b.now().Add(CodeLifetime),
	}); err != nil {
		b.cfg.Log.ErrorContext(ctx, "store mcp grant failed", "error", err)
		b.revokeUpstream(ctx, grant.refreshToken)
		b.redirectError(w, r, req.RedirectURI, req.ClientState, newOAuthError(errTemporarilyUnavailable, "retry later"))
		return
	}
	b.cfg.Log.InfoContext(ctx, "mcp grant created", "grant_id", grant.grant.ID, "client_id", req.ClientID)
	b.redirectToClient(w, r, req.RedirectURI, url.Values{"code": {code}, "state": {req.ClientState}})
}

// boundedOAuthError keeps upstream error codes from becoming free-form log
// content.
func boundedOAuthError(code string) string {
	switch code {
	case "", "access_denied", "invalid_request", "invalid_scope", "server_error", "temporarily_unavailable",
		"login_required", "consent_required", "interaction_required", "invalid_target":
		return code
	default:
		return "other"
	}
}

type completedLogin struct {
	grant        store.MCPGrant
	refreshToken string
	delcoreToken string
	tenantIDs    []string
}

// completeLogin exchanges the upstream code, verifies the MCP-audience token
// and its MFA evidence, resolves the Delcore user with a Delcore-audience
// token and seals the grant tokens.
func (b *Broker) completeLogin(ctx context.Context, code string, req store.MCPAuthRequest) (*completedLogin, error) {
	logto := b.cfg.Logto
	tr, err := logto.ExchangeCode(ctx, code, req.CodeVerifier, b.callbackURI(), b.cfg.MCPResource)
	if err != nil {
		return nil, err
	}
	if tr.IDToken == "" || tr.RefreshToken == "" {
		return nil, errors.New("token response lacks an ID token or refresh token")
	}
	idTok, err := logto.VerifyIDToken(ctx, tr.IDToken, req.Nonce)
	if err != nil {
		return nil, err
	}
	mcpTok, err := logto.VerifyAccessToken(ctx, tr.AccessToken, b.cfg.MCPResource)
	if err != nil {
		return nil, err
	}
	if mcpTok.Subject != idTok.Subject {
		return nil, errors.New("access token subject does not match the ID token")
	}
	refresh := tr.RefreshToken
	if !auth.MFAEvidence(mcpTok.Claims, b.cfg.MFAClaim) {
		b.revokeUpstream(ctx, refresh)
		return nil, b.mfaRequired()
	}
	dc, err := logto.RefreshForResource(ctx, refresh, b.cfg.DelcoreResource)
	if err != nil {
		return nil, err
	}
	if dc.RefreshToken != "" {
		refresh = dc.RefreshToken
	}
	user, err := b.cfg.Delcore.GetCurrentUser(ctx, dc.AccessToken)
	if err != nil {
		// No grant will hold the upstream refresh token.
		b.revokeUpstream(ctx, refresh)
		return nil, apperr.DependencyUnavailable(fmt.Errorf("resolve Delcore user: %w", err))
	}
	id := store.NewID()
	sealed, err := b.seal(id, grantTokens{
		RefreshToken: refresh, DelcoreAccessToken: dc.AccessToken,
		DelcoreExpiry: b.now().Add(time.Duration(dc.ExpiresIn) * time.Second),
	})
	if err != nil {
		return nil, err
	}
	out := &completedLogin{
		grant: store.MCPGrant{
			ID: id, ClientID: req.ClientID, UserID: user.UserID, Subject: idTok.Subject,
			ExpireTime: b.now().Add(GrantIdleLifetime), Tokens: sealed,
		},
		refreshToken: refresh,
		delcoreToken: dc.AccessToken,
	}
	for _, t := range user.Tenants {
		out.tenantIDs = append(out.tenantIDs, t.GetId())
	}
	return out, nil
}

// refreshSnapshots refreshes the group-membership snapshots of the user's
// tenants, as console sessions do when they operate in a tenant. Snapshots
// newer than the refresh TTL are kept; failures keep the previous snapshot.
// The refresh as a whole is bounded by the dependency timeout, and tenants
// not reached in time are skipped.
func (b *Broker) refreshSnapshots(ctx context.Context, l *completedLogin) {
	if b.cfg.Groups == nil {
		return
	}
	ctx, cancel := context.WithTimeout(ctx, b.cfg.DependencyTimeout)
	defer cancel()
	token := l.delcoreToken
	p := auth.NewUserPrincipal(mcpav1.ActorKind_ACTOR_KIND_MCP, l.grant.UserID, l.grant.Subject,
		func(context.Context) (string, error) { return token, nil })
	for i, tenantID := range l.tenantIDs {
		if i >= maxSnapshotTenants {
			break
		}
		if ctx.Err() != nil {
			b.cfg.Log.WarnContext(ctx, "mcp group snapshot refresh skipped the remaining tenants", "refreshed", i, "tenants", len(l.tenantIDs))
			break
		}
		b.cfg.Groups.RefreshGroups(ctx, p, tenantID)
	}
}

// clientID returns the authenticating public client. A client may send its
// ID in the form or as the HTTP Basic user name with an empty password.
func (b *Broker) clientID(r *http.Request) (string, *oauthError) {
	form := r.PostForm
	if len(form["client_id"]) > 1 {
		return "", newOAuthError(errInvalidRequest, "client_id is repeated")
	}
	id := form.Get("client_id")
	if user, pass, ok := r.BasicAuth(); ok {
		if pass != "" || (id != "" && id != user) {
			return "", newOAuthError(errInvalidClient, "clients authenticate with client_id only")
		}
		id = user
	}
	if _, ok := b.cfg.Clients[id]; !ok || id == "" {
		return "", newOAuthError(errInvalidClient, "unknown client_id")
	}
	return id, nil
}

const maxFormBytes = 64 << 10

func parseForm(w http.ResponseWriter, r *http.Request) *oauthError {
	r.Body = http.MaxBytesReader(w, r.Body, maxFormBytes)
	if ct := r.Header.Get("Content-Type"); ct == "" || !isFormContentType(ct) {
		return newOAuthError(errInvalidRequest, "the body must be application/x-www-form-urlencoded")
	}
	if err := r.ParseForm(); err != nil {
		return newOAuthError(errInvalidRequest, "the form could not be parsed")
	}
	return nil
}

func isFormContentType(ct string) bool {
	mt, _, err := mime.ParseMediaType(ct)
	return err == nil && mt == "application/x-www-form-urlencoded"
}

type tokenResponse struct {
	AccessToken  string `json:"access_token"`
	TokenType    string `json:"token_type"`
	ExpiresIn    int64  `json:"expires_in"`
	RefreshToken string `json:"refresh_token"`
}

// token serves the authorization_code and refresh_token grants.
func (b *Broker) token(w http.ResponseWriter, r *http.Request) {
	ctx := r.Context()
	if e := parseForm(w, r); e != nil {
		writeOAuthError(w, e)
		return
	}
	clientID, e := b.clientID(r)
	if e != nil {
		writeOAuthError(w, e)
		return
	}
	p, ok := singleValues(r.PostForm, "grant_type", "code", "redirect_uri", "code_verifier", "refresh_token", "resource")
	if !ok {
		writeOAuthError(w, newOAuthError(errInvalidRequest, "a parameter is repeated"))
		return
	}
	if p["resource"] != "" && p["resource"] != b.cfg.MCPResource {
		writeOAuthError(w, newOAuthError(errInvalidTarget, "resource must be the Managed CPA MCP resource"))
		return
	}
	var resp *tokenResponse
	switch p["grant_type"] {
	case "authorization_code":
		resp, e = b.exchangeCode(ctx, clientID, p["code"], p["redirect_uri"], p["code_verifier"])
	case "refresh_token":
		resp, e = b.exchangeRefresh(ctx, clientID, p["refresh_token"])
	case "":
		e = newOAuthError(errInvalidRequest, "grant_type is required")
	default:
		e = newOAuthError(errUnsupportedGrantType, "grant_type must be authorization_code or refresh_token")
	}
	if e != nil {
		b.cfg.Log.InfoContext(ctx, "mcp token request rejected", "client_id", clientID, "grant_type", boundedGrantType(p["grant_type"]), "oauth_error", string(e.code))
		writeOAuthError(w, e)
		return
	}
	writeJSON(w, http.StatusOK, resp)
}

func boundedGrantType(v string) string {
	switch v {
	case "authorization_code", "refresh_token", "":
		return v
	default:
		return "other"
	}
}

func (b *Broker) exchangeCode(ctx context.Context, clientID, code, redirectURI, verifier string) (*tokenResponse, *oauthError) {
	if code == "" || redirectURI == "" || verifier == "" {
		return nil, newOAuthError(errInvalidRequest, "code, redirect_uri and code_verifier are required")
	}
	// The code is consumed by issue together with the tokens it issues, so an
	// exchange that fails before issuing tokens can be retried with it.
	c, err := b.cfg.Store.LookupCode(ctx, auth.Digest(code))
	switch {
	case errors.Is(err, store.ErrCodeReused):
		// A replayed code revokes everything issued from it (RFC 6749 4.1.2).
		_ = b.revokeGrant(ctx, c.GrantID, "authorization code reused")
		return nil, newOAuthError(errInvalidGrant, "the authorization code is invalid")
	case errors.Is(err, store.ErrNotFound):
		return nil, newOAuthError(errInvalidGrant, "the authorization code is invalid or expired")
	case err != nil:
		b.cfg.Log.ErrorContext(ctx, "redeem mcp authorization code failed", "error", err)
		return nil, newOAuthError(errTemporarilyUnavailable, "retry later")
	}
	if c.ClientID != clientID || c.RedirectURI != redirectURI || !VerifyPKCE(verifier, c.CodeChallenge) {
		// Revoking the grant removes the code, so it can never issue tokens.
		_ = b.revokeGrant(ctx, c.GrantID, "authorization code binding mismatch")
		return nil, newOAuthError(errInvalidGrant, "the authorization code does not match this request")
	}
	return b.issue(ctx, c.GrantID, clientID, store.MCPGrantUse{Code: c.CodeHash})
}

func (b *Broker) exchangeRefresh(ctx context.Context, clientID, handle string) (*tokenResponse, *oauthError) {
	if handle == "" {
		return nil, newOAuthError(errInvalidRequest, "refresh_token is required")
	}
	hash := auth.Digest(handle)
	grantID, retired, err := b.cfg.Store.RefreshHandleGrant(ctx, hash)
	switch {
	case errors.Is(err, store.ErrNotFound):
		return nil, newOAuthError(errInvalidGrant, "the refresh token is invalid")
	case err != nil:
		b.cfg.Log.ErrorContext(ctx, "read mcp refresh handle failed", "error", err)
		return nil, newOAuthError(errTemporarilyUnavailable, "retry later")
	case retired:
		_ = b.revokeGrant(ctx, grantID, "refresh token reused")
		return nil, newOAuthError(errInvalidGrant, "the refresh token is invalid")
	}
	return b.issue(ctx, grantID, clientID, store.MCPGrantUse{Handle: hash})
}

var errClientMismatch = errors.New("grant belongs to another client")

// issue obtains a new MCP-audience token with the grant's upstream refresh
// token, re-checks MFA evidence, records the token and rotates the refresh
// handle. use names the presented authorization code or refresh handle,
// which is consumed only when tokens are issued. A rejected upstream
// refresh, a changed subject or a token without MFA evidence revokes the
// grant. When issuance fails for a recoverable reason, the presented code or
// handle stays usable; if Logto rotated the upstream refresh token, the
// replacement is stored without issuing a token, so a retry does not present
// the consumed upstream token.
func (b *Broker) issue(ctx context.Context, grantID uuid.UUID, clientID string, use store.MCPGrantUse) (*tokenResponse, *oauthError) {
	var resp *tokenResponse
	var lostRefresh string
	keptRotation := false
	// ran is set once the grant is locked and the upstream refresh starts.
	ran := false
	err := b.cfg.Store.UpdateGrant(ctx, grantID, use, func(wctx context.Context, g store.MCPGrant) (store.MCPGrantChange, error) {
		ran = true
		if g.ClientID != clientID {
			return store.MCPGrantChange{}, errClientMismatch
		}
		t, err := b.open(g)
		if err != nil {
			return store.MCPGrantChange{}, fmt.Errorf("open grant tokens: %w", err)
		}
		tr, err := b.cfg.Logto.RefreshForResource(wctx, t.RefreshToken, b.cfg.MCPResource)
		if errors.Is(err, auth.ErrInvalidGrant) {
			return store.MCPGrantChange{Delete: true}, auth.ErrInvalidGrant
		}
		if err != nil {
			return store.MCPGrantChange{}, err
		}
		rotated := tr.RefreshToken != "" && tr.RefreshToken != t.RefreshToken
		if rotated {
			t.RefreshToken = tr.RefreshToken
		}
		sealed, err := b.seal(g.ID, t)
		if err != nil {
			return store.MCPGrantChange{}, err
		}
		// keep stores only the rotated upstream refresh token. It is returned
		// with recoverable failures below; it issues no handle or token.
		var keep store.MCPGrantChange
		if rotated {
			keep.Tokens = &sealed
		}
		tok, err := b.cfg.Logto.VerifyAccessToken(wctx, tr.AccessToken, b.cfg.MCPResource)
		if err != nil {
			keptRotation = rotated
			return keep, fmt.Errorf("%w: refreshed token: %v", auth.ErrLogtoUnavailable, err)
		}
		if tok.Subject != g.Subject {
			lostRefresh = t.RefreshToken
			return store.MCPGrantChange{Delete: true}, auth.ErrInvalidGrant
		}
		if !auth.MFAEvidence(tok.Claims, b.cfg.MFAClaim) {
			lostRefresh = t.RefreshToken
			return store.MCPGrantChange{Delete: true}, b.mfaRequired()
		}
		// A client that is gone cannot receive the new handle, so consuming
		// the presented one would leave the grant without a usable handle.
		// Keep only the rotated upstream refresh token; the client retries
		// with the same handle.
		if err := ctx.Err(); err != nil {
			keptRotation = rotated
			return keep, err
		}
		handle, err := auth.RandomToken()
		if err != nil {
			keptRotation = rotated
			return keep, err
		}
		expiresIn := int64(tok.Expiry.Sub(b.now()).Seconds())
		if tr.ExpiresIn > 0 && tr.ExpiresIn < expiresIn {
			expiresIn = tr.ExpiresIn
		}
		resp = &tokenResponse{AccessToken: tr.AccessToken, TokenType: "Bearer", ExpiresIn: max(expiresIn, 0), RefreshToken: handle}
		return store.MCPGrantChange{
			Tokens: &sealed, ExpireTime: b.now().Add(GrantIdleLifetime), NewHandle: auth.Digest(handle),
			AccessToken: tok.Digest, AccessExpire: tok.Expiry,
		}, nil
	})
	if lostRefresh != "" {
		b.revokeUpstream(ctx, lostRefresh)
	}
	if ran && ctx.Err() != nil {
		// The upstream refresh ran detached from the canceled request, so a
		// rotated upstream refresh token was stored (or the grant deleted) as
		// for a live request. Log without token material.
		b.cfg.Log.WarnContext(ctx, "mcp token issuance finished after the request was canceled",
			"grant_id", grantID, "client_id", clientID, "request_cancelled", true,
			"upstream_refresh_kept", keptRotation, "issued", err == nil)
	}
	switch {
	case err == nil:
		b.cfg.Log.InfoContext(ctx, "mcp tokens issued", "grant_id", grantID, "client_id", clientID, "rotated", use.Handle != nil)
		return resp, nil
	case errors.Is(err, store.ErrHandleRetired):
		_ = b.revokeGrant(ctx, grantID, "refresh token reused")
		return nil, newOAuthError(errInvalidGrant, "the refresh token is invalid")
	case errors.Is(err, store.ErrCodeReused):
		// A concurrent exchange of the code issued tokens first; this is a
		// replay (RFC 6749 4.1.2).
		_ = b.revokeGrant(ctx, grantID, "authorization code reused")
		return nil, newOAuthError(errInvalidGrant, "the authorization code is invalid")
	case errors.Is(err, store.ErrNotFound), errors.Is(err, errClientMismatch):
		return nil, newOAuthError(errInvalidGrant, "the grant is invalid or expired")
	case errors.Is(err, auth.ErrInvalidGrant):
		b.cfg.Log.InfoContext(ctx, "mcp grant revoked after upstream refresh rejection", "grant_id", grantID)
		return nil, newOAuthError(errInvalidGrant, "the grant expired; sign in again")
	case isMFA(err):
		b.cfg.Log.InfoContext(ctx, "mcp grant revoked: refreshed token lacks MFA evidence", "grant_id", grantID)
		return nil, &oauthError{code: errInvalidGrant, description: "multi-factor authentication is required; sign in again", uri: b.cfg.ReauthURL}
	case errors.Is(err, auth.ErrLogtoUnavailable):
		// A failed commit returns its own error, so the rotated token was
		// stored when keptRotation is set.
		b.cfg.Log.WarnContext(ctx, "mcp token issuance failed", "grant_id", grantID, "upstream_refresh_kept", keptRotation, "error", err)
		return nil, newOAuthError(errTemporarilyUnavailable, "the identity provider is unavailable; retry later")
	case errors.Is(err, context.Canceled):
		// The client left before issuance finished; nothing was consumed
		// except a rotated upstream refresh token, which is stored.
		b.cfg.Log.WarnContext(ctx, "mcp token issuance abandoned: request canceled", "grant_id", grantID, "upstream_refresh_kept", keptRotation)
		return nil, newOAuthError(errTemporarilyUnavailable, "retry later")
	case errors.Is(err, store.ErrStorage), errors.Is(err, context.DeadlineExceeded):
		// The database failed or the request ran out of time. Nothing was
		// consumed: the code or handle stays usable and the client retries.
		b.cfg.Log.WarnContext(ctx, "mcp token issuance failed: storage unavailable", "grant_id", grantID, "error", err)
		return nil, newOAuthError(errTemporarilyUnavailable, "retry later")
	default:
		b.cfg.Log.ErrorContext(ctx, "mcp token issuance failed", "grant_id", grantID, "error", err)
		return nil, newOAuthError(errServerError, "")
	}
}

// revoke implements RFC 7009 for refresh handles and access tokens. Either
// revokes the whole grant and its upstream refresh token. Unknown tokens are
// answered with 200.
func (b *Broker) revoke(w http.ResponseWriter, r *http.Request) {
	ctx := r.Context()
	if e := parseForm(w, r); e != nil {
		writeOAuthError(w, e)
		return
	}
	clientID, e := b.clientID(r)
	if e != nil {
		writeOAuthError(w, e)
		return
	}
	p, ok := singleValues(r.PostForm, "token", "token_type_hint")
	if !ok || p["token"] == "" {
		writeOAuthError(w, newOAuthError(errInvalidRequest, "token is required"))
		return
	}
	grantID, err := b.grantOfToken(ctx, p["token"], p["token_type_hint"])
	if errors.Is(err, store.ErrNotFound) {
		w.Header().Set("Cache-Control", "no-store")
		w.WriteHeader(http.StatusOK)
		return
	}
	if err != nil {
		b.cfg.Log.ErrorContext(ctx, "mcp revocation lookup failed", "error", err)
		writeOAuthError(w, newOAuthError(errTemporarilyUnavailable, "retry later"))
		return
	}
	g, err := b.cfg.Store.GetGrant(ctx, grantID)
	if err == nil && g.ClientID != clientID {
		writeOAuthError(w, newOAuthError(errUnauthorizedClient, "the token was issued to another client"))
		return
	}
	if err != nil && !errors.Is(err, store.ErrNotFound) {
		writeOAuthError(w, newOAuthError(errTemporarilyUnavailable, "retry later"))
		return
	}
	if err := b.revokeGrant(ctx, grantID, "client revocation"); err != nil {
		// RFC 7009 section 2.2.1: on 503 the client assumes the token still
		// exists and retries later.
		writeOAuthError(w, newOAuthError(errTemporarilyUnavailable, "the token was not revoked; retry later"))
		return
	}
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(http.StatusOK)
}

func (b *Broker) grantOfToken(ctx context.Context, token, hint string) (uuid.UUID, error) {
	hash := auth.Digest(token)
	byHandle := func() (uuid.UUID, error) {
		id, _, err := b.cfg.Store.RefreshHandleGrant(ctx, hash)
		return id, err
	}
	byAccess := func() (uuid.UUID, error) { return b.cfg.Store.GrantIDByAccessToken(ctx, hash) }
	order := []func() (uuid.UUID, error){byHandle, byAccess}
	if hint == "access_token" {
		order = []func() (uuid.UUID, error){byAccess, byHandle}
	}
	for _, f := range order {
		id, err := f()
		if !errors.Is(err, store.ErrNotFound) {
			return id, err
		}
	}
	return uuid.Nil, store.ErrNotFound
}
