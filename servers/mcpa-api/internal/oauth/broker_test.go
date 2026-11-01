// SPDX-License-Identifier: FSL-1.1-ALv2

package oauth_test

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"net/url"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"connectrpc.com/connect"
	"golang.org/x/oauth2"

	delcorev1 "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/protos/delcore/v1/delcorev1connect"
	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth/authtest"
	"github.com/delinoio/apps/servers/mcpa-api/internal/config"
	"github.com/delinoio/apps/servers/mcpa-api/internal/crypto"
	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
	"github.com/delinoio/apps/servers/mcpa-api/internal/oauth"
)

const (
	publicURL       = "https://api.example.test"
	webOrigin       = "https://console.example.test"
	mcpResource     = "https://api.example.test/mcp"
	delcoreResource = "https://delcore.example.test"
	reauthURL       = "https://login.example.test/mfa"
	logtoClientID   = "logto-app"
	clientID        = "claude"
	otherClientID   = "other"
	redirectURI     = "http://127.0.0.1:33418/callback"
	otherRedirect   = "https://other.example.test/cb"
	subject         = "logto-sub"
	delcoreUserID   = "0190f000-0000-7000-8000-000000000001"
)

// fakeDelcore records the Authorization header of every user-token call.
type fakeDelcore struct {
	delcorev1connect.UnimplementedDelcoreServiceHandler
	mu      sync.Mutex
	headers []string
	down    bool
}

func (f *fakeDelcore) GetCurrentUser(_ context.Context, req *connect.Request[delcorev1.GetCurrentUserRequest]) (*connect.Response[delcorev1.GetCurrentUserResponse], error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.headers = append(f.headers, req.Header().Get("Authorization"))
	if f.down {
		return nil, connect.NewError(connect.CodeUnavailable, errors.New("down"))
	}
	if req.Msg.GetPage().GetAfterId() == "" {
		return connect.NewResponse(&delcorev1.GetCurrentUserResponse{UserId: delcoreUserID, Tenants: []*delcorev1.Tenant{{Id: "tenant-1"}}, NextAfterId: "p2"}), nil
	}
	return connect.NewResponse(&delcorev1.GetCurrentUserResponse{UserId: delcoreUserID, Tenants: []*delcorev1.Tenant{{Id: "tenant-2"}}}), nil
}

func (f *fakeDelcore) authHeaders() []string {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]string(nil), f.headers...)
}

type fakeGroups struct {
	mu      sync.Mutex
	tenants []string
	tokens  []string
}

func (g *fakeGroups) RefreshGroups(ctx context.Context, p *auth.Principal, tenantID string) {
	token, _ := p.DelcoreToken(ctx)
	g.mu.Lock()
	defer g.mu.Unlock()
	g.tenants = append(g.tenants, tenantID)
	g.tokens = append(g.tokens, token)
}

// upstream scripts the Logto token endpoint: one refresh token is current
// at a time and every refresh rotates it, like Logto with rotation enabled.
type upstream struct {
	t                 *testing.T
	p                 *authtest.Provider
	mu                sync.Mutex
	nonce             string
	upstreamChallenge string
	current           string
	n                 int
	mfa               bool
	subject           string
	issued            []string
}

func (u *upstream) mcpToken() string {
	u.n++
	claims := map[string]any{"sub": u.subject, "aud": mcpResource, "client_id": logtoClientID, "jti": fmt.Sprintf("jti-%d", u.n)}
	if u.mfa {
		claims["mcpa_mfa"] = true
	}
	tok := u.p.Sign(u.t, claims)
	u.issued = append(u.issued, tok)
	return tok
}

func (u *upstream) rotate() string {
	u.n++
	u.current = fmt.Sprintf("rt-%d", u.n)
	return u.current
}

func (u *upstream) handle(form url.Values) (int, any) {
	u.mu.Lock()
	defer u.mu.Unlock()
	invalid := map[string]string{"error": "invalid_grant"}
	switch form.Get("grant_type") {
	case "authorization_code":
		sum := sha256.Sum256([]byte(form.Get("code_verifier")))
		if form.Get("code") != "logto-code" || base64.RawURLEncoding.EncodeToString(sum[:]) != u.upstreamChallenge ||
			form.Get("resource") != mcpResource || form.Get("redirect_uri") != publicURL+oauth.CallbackPath {
			return http.StatusBadRequest, invalid
		}
		id := u.p.Sign(u.t, map[string]any{"sub": u.subject, "aud": logtoClientID, "nonce": u.nonce})
		return http.StatusOK, map[string]any{"access_token": u.mcpToken(), "id_token": id, "refresh_token": u.rotate(), "expires_in": 3600, "token_type": "Bearer"}
	case "refresh_token":
		if form.Get("refresh_token") != u.current || u.current == "" {
			return http.StatusBadRequest, invalid
		}
		switch form.Get("resource") {
		case delcoreResource:
			u.n++
			return http.StatusOK, map[string]any{"access_token": fmt.Sprintf("dc-token-%d", u.n), "refresh_token": u.rotate(), "expires_in": 3600}
		case mcpResource:
			return http.StatusOK, map[string]any{"access_token": u.mcpToken(), "refresh_token": u.rotate(), "expires_in": 3600}
		}
	}
	return http.StatusBadRequest, map[string]string{"error": "invalid_request"}
}

func (u *upstream) set(fn func(*upstream)) {
	u.mu.Lock()
	defer u.mu.Unlock()
	fn(u)
}

type env struct {
	t        *testing.T
	logto    *authtest.Provider
	up       *upstream
	store    *fakeStore
	broker   *oauth.Broker
	h        http.Handler
	dc       *fakeDelcore
	groups   *fakeGroups
	clockMu  sync.Mutex
	clock    time.Time
	verifier string
}

func (e *env) now() time.Time {
	e.clockMu.Lock()
	defer e.clockMu.Unlock()
	return e.clock
}

func (e *env) advance(d time.Duration) {
	e.clockMu.Lock()
	defer e.clockMu.Unlock()
	e.clock = e.clock.Add(d)
}

func newEnv(t *testing.T) *env {
	t.Helper()
	e := &env{t: t, clock: time.Now(), dc: &fakeDelcore{}, groups: &fakeGroups{}}
	e.logto = authtest.New(t)
	e.up = &upstream{t: t, p: e.logto, mfa: true, subject: subject}
	e.logto.SetTokenHandler(e.up.handle)
	e.store = newFakeStore(e.now)

	_, dcHandler := delcorev1connect.NewDelcoreServiceHandler(e.dc)
	dcServer := httptest.NewServer(dcHandler)
	t.Cleanup(dcServer.Close)
	dc, err := delcore.New(delcore.Config{
		BaseURL: dcServer.URL, Resource: delcoreResource, CacheTTL: time.Minute,
		TokenSource: oauth2.StaticTokenSource(&oauth2.Token{AccessToken: "m2m"}),
	})
	if err != nil {
		t.Fatal(err)
	}
	key := make([]byte, 32)
	_, _ = rand.Read(key)
	keys, err := crypto.NewKeyring(map[string][]byte{"k1": key}, "k1")
	if err != nil {
		t.Fatal(err)
	}
	e.broker = oauth.New(oauth.Config{
		Store:   e.store,
		Keys:    keys,
		Logto:   auth.NewLogto(e.logto.Issuer(), logtoClientID, "logto-secret", nil),
		Delcore: dc,
		Groups:  e.groups,
		Clients: map[string]config.OAuthClient{
			clientID:      {Name: "Claude", RedirectURIs: []string{redirectURI}},
			otherClientID: {Name: "Other", RedirectURIs: []string{otherRedirect}},
		},
		PublicURL: publicURL, WebOrigin: webOrigin, MCPResource: mcpResource, DelcoreResource: delcoreResource,
		MFAClaim: config.DefaultMFAClaim, ReauthURL: reauthURL,
		Log: slog.New(slog.NewTextHandler(io.Discard, nil)),
	})
	e.broker.SetClock(e.now)
	mux := http.NewServeMux()
	for _, m := range e.broker.Mounts() {
		mux.Handle(m.Pattern, m.Handler)
	}
	e.h = mux
	e.verifier = "client-verifier-" + strings.Repeat("a", 40)
	return e
}

func challenge(verifier string) string {
	sum := sha256.Sum256([]byte(verifier))
	return base64.RawURLEncoding.EncodeToString(sum[:])
}

func (e *env) do(req *http.Request) *httptest.ResponseRecorder {
	rec := httptest.NewRecorder()
	e.h.ServeHTTP(rec, req)
	return rec
}

func (e *env) get(path string, cookies ...*http.Cookie) *httptest.ResponseRecorder {
	req := httptest.NewRequest(http.MethodGet, path, nil)
	for _, c := range cookies {
		req.AddCookie(c)
	}
	return e.do(req)
}

func (e *env) post(path string, form url.Values, header ...string) *httptest.ResponseRecorder {
	req := httptest.NewRequest(http.MethodPost, path, strings.NewReader(form.Encode()))
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	for i := 0; i+1 < len(header); i += 2 {
		req.Header.Set(header[i], header[i+1])
	}
	return e.do(req)
}

func authorizeQuery(overrides map[string]string) string {
	q := url.Values{
		"client_id": {clientID}, "redirect_uri": {redirectURI}, "response_type": {"code"}, "state": {"client-state"},
		"code_challenge": {challenge("client-verifier-" + strings.Repeat("a", 40))}, "code_challenge_method": {"S256"},
		"resource": {mcpResource},
	}
	for k, v := range overrides {
		if v == "" {
			q.Del(k)
		} else {
			q.Set(k, v)
		}
	}
	return oauth.AuthorizePath + "?" + q.Encode()
}

func location(t *testing.T, rec *httptest.ResponseRecorder) *url.URL {
	t.Helper()
	if rec.Code != http.StatusFound {
		t.Fatalf("status %d, body %s", rec.Code, rec.Body.String())
	}
	u, err := url.Parse(rec.Header().Get("Location"))
	if err != nil {
		t.Fatal(err)
	}
	return u
}

// startLogin runs /oauth/authorize and returns the upstream state and the
// browser binding cookie.
func (e *env) startLogin() (string, *http.Cookie) {
	e.t.Helper()
	rec := e.get(authorizeQuery(nil))
	up := location(e.t, rec)
	if !strings.HasPrefix(up.String(), e.logto.Issuer()+"/auth?") {
		e.t.Fatalf("redirect to %s", up)
	}
	q := up.Query()
	if q.Get("client_id") != logtoClientID || q.Get("redirect_uri") != publicURL+oauth.CallbackPath || q.Get("code_challenge_method") != "S256" ||
		!slices.Equal(q["resource"], []string{mcpResource, delcoreResource}) || !strings.Contains(q.Get("scope"), "offline_access") {
		e.t.Fatalf("upstream query %v", q)
	}
	e.up.set(func(u *upstream) { u.nonce = q.Get("nonce"); u.upstreamChallenge = q.Get("code_challenge") })
	var cookie *http.Cookie
	for _, c := range rec.Result().Cookies() {
		if c.Name == "mcpa_mcp_oauth" {
			cookie = c
		}
	}
	if cookie == nil || !cookie.HttpOnly || !cookie.Secure || cookie.Path != oauth.CallbackPath {
		e.t.Fatalf("browser cookie %+v", cookie)
	}
	return q.Get("state"), cookie
}

// login completes the browser part and returns the client redirect.
func (e *env) login() *url.URL {
	e.t.Helper()
	state, cookie := e.startLogin()
	rec := e.get(oauth.CallbackPath+"?"+url.Values{"code": {"logto-code"}, "state": {state}}.Encode(), cookie)
	return location(e.t, rec)
}

func (e *env) code() string {
	e.t.Helper()
	u := e.login()
	q := u.Query()
	if u.Scheme+"://"+u.Host+u.Path != redirectURI || q.Get("state") != "client-state" || q.Get("iss") != publicURL || q.Get("code") == "" {
		e.t.Fatalf("client redirect %s", u)
	}
	return q.Get("code")
}

type tokenBody struct {
	AccessToken      string `json:"access_token"`
	TokenType        string `json:"token_type"`
	ExpiresIn        int64  `json:"expires_in"`
	RefreshToken     string `json:"refresh_token"`
	Error            string `json:"error"`
	ErrorDescription string `json:"error_description"`
	ErrorURI         string `json:"error_uri"`
}

func decodeToken(t *testing.T, rec *httptest.ResponseRecorder) tokenBody {
	t.Helper()
	var b tokenBody
	if err := json.Unmarshal(rec.Body.Bytes(), &b); err != nil {
		t.Fatalf("body %q: %v", rec.Body.String(), err)
	}
	if rec.Header().Get("Cache-Control") != "no-store" {
		t.Fatalf("token response is cacheable")
	}
	return b
}

func (e *env) exchange(code string, overrides map[string]string) (int, tokenBody) {
	e.t.Helper()
	form := url.Values{"grant_type": {"authorization_code"}, "code": {code}, "redirect_uri": {redirectURI}, "code_verifier": {e.verifier}, "client_id": {clientID}}
	for k, v := range overrides {
		form.Set(k, v)
	}
	rec := e.post(oauth.TokenPath, form)
	return rec.Code, decodeToken(e.t, rec)
}

func (e *env) refresh(handle, client string) (int, tokenBody) {
	e.t.Helper()
	rec := e.post(oauth.TokenPath, url.Values{"grant_type": {"refresh_token"}, "refresh_token": {handle}, "client_id": {client}})
	return rec.Code, decodeToken(e.t, rec)
}

// resolve verifies an MCP token like the authenticator and resolves it.
func (e *env) resolve(token string) (*auth.MCPGrant, error) {
	e.t.Helper()
	l := auth.NewLogto(e.logto.Issuer(), logtoClientID, "", nil)
	v, err := l.VerifyAccessToken(context.Background(), token, mcpResource)
	if err != nil {
		e.t.Fatal(err)
	}
	return e.broker.ResolveMCPToken(context.Background(), v)
}

func TestMetadataDocuments(t *testing.T) {
	e := newEnv(t)
	for _, path := range []string{oauth.ProtectedResourcePath, oauth.ProtectedResourcePath + "/mcp"} {
		rec := e.get(path)
		var doc map[string]any
		if rec.Code != http.StatusOK || json.Unmarshal(rec.Body.Bytes(), &doc) != nil {
			t.Fatalf("%s: %d %s", path, rec.Code, rec.Body.String())
		}
		if doc["resource"] != mcpResource || fmt.Sprint(doc["authorization_servers"]) != "["+publicURL+"]" {
			t.Fatalf("%s: %v", path, doc)
		}
		if rec.Header().Get("Access-Control-Allow-Origin") != "*" || rec.Header().Get("Access-Control-Allow-Credentials") != "" {
			t.Fatalf("%s: CORS %v", path, rec.Header())
		}
	}
	rec := e.get(oauth.AuthorizationServerPath)
	var doc map[string]any
	if err := json.Unmarshal(rec.Body.Bytes(), &doc); err != nil {
		t.Fatal(err)
	}
	want := map[string]string{
		"issuer":                 publicURL,
		"authorization_endpoint": publicURL + "/oauth/authorize",
		"token_endpoint":         publicURL + "/oauth/token",
		"revocation_endpoint":    publicURL + "/oauth/revoke",
	}
	for k, v := range want {
		if doc[k] != v {
			t.Fatalf("%s = %v", k, doc[k])
		}
	}
	for k, v := range map[string]string{
		"code_challenge_methods_supported":      "[S256]",
		"grant_types_supported":                 "[authorization_code refresh_token]",
		"response_types_supported":              "[code]",
		"token_endpoint_auth_methods_supported": "[none]",
	} {
		if fmt.Sprint(doc[k]) != v {
			t.Fatalf("%s = %v", k, doc[k])
		}
	}
	if _, ok := doc["registration_endpoint"]; ok {
		t.Fatal("dynamic client registration is advertised")
	}
	if rec := e.post(oauth.AuthorizationServerPath, url.Values{}); rec.Code != http.StatusMethodNotAllowed {
		t.Fatalf("POST metadata = %d", rec.Code)
	}
}

func TestAuthorizeRejectsUnknownClientAndRedirect(t *testing.T) {
	e := newEnv(t)
	for name, o := range map[string]map[string]string{
		"unknown client":       {"client_id": "nope"},
		"missing client":       {"client_id": ""},
		"other client's URI":   {"redirect_uri": otherRedirect},
		"trailing slash":       {"redirect_uri": redirectURI + "/"},
		"extra query":          {"redirect_uri": redirectURI + "?x=1"},
		"different port":       {"redirect_uri": "http://127.0.0.1:1/callback"},
		"missing redirect_uri": {"redirect_uri": ""},
	} {
		rec := e.get(authorizeQuery(o))
		if rec.Code != http.StatusBadRequest || rec.Header().Get("Location") != "" {
			t.Fatalf("%s: %d %q", name, rec.Code, rec.Header().Get("Location"))
		}
	}
	if rec := e.get(authorizeQuery(nil) + "&client_id=" + clientID); rec.Code != http.StatusBadRequest {
		t.Fatalf("repeated client_id: %d", rec.Code)
	}
}

func TestAuthorizeRequiresS256AndState(t *testing.T) {
	e := newEnv(t)
	cases := map[string]struct {
		o     map[string]string
		error string
		state string
	}{
		"plain method":      {map[string]string{"code_challenge_method": "plain"}, "invalid_request", "client-state"},
		"missing challenge": {map[string]string{"code_challenge": ""}, "invalid_request", "client-state"},
		"short challenge":   {map[string]string{"code_challenge": "abc"}, "invalid_request", "client-state"},
		"missing state":     {map[string]string{"state": ""}, "invalid_request", ""},
		"token response":    {map[string]string{"response_type": "token"}, "unsupported_response_type", "client-state"},
		"other resource":    {map[string]string{"resource": publicURL}, "invalid_target", "client-state"},
	}
	for name, c := range cases {
		u := location(t, e.get(authorizeQuery(c.o)))
		q := u.Query()
		if u.Scheme+"://"+u.Host+u.Path != redirectURI || q.Get("error") != c.error || q.Get("state") != c.state || q.Get("code") != "" {
			t.Fatalf("%s: %s", name, u)
		}
	}
	// Without resource the MCP resource is implied.
	u := location(t, e.get(authorizeQuery(map[string]string{"resource": ""})))
	if !strings.HasPrefix(u.String(), e.logto.Issuer()) {
		t.Fatalf("no resource: %s", u)
	}
}

func TestLoginIssuesMCPTokens(t *testing.T) {
	e := newEnv(t)
	code := e.code()
	status, tok := e.exchange(code, nil)
	if status != http.StatusOK || tok.TokenType != "Bearer" || tok.RefreshToken == "" || tok.ExpiresIn <= 0 || tok.ExpiresIn > 3600 {
		t.Fatalf("exchange %d %+v", status, tok)
	}
	// The access token is the Logto MCP-audience token, issued on refresh.
	e.up.mu.Lock()
	issued := slices.Clone(e.up.issued)
	e.up.mu.Unlock()
	if tok.AccessToken != issued[len(issued)-1] || len(issued) != 2 {
		t.Fatalf("access token is not the last Logto MCP token (%d issued)", len(issued))
	}
	grant, err := e.resolve(tok.AccessToken)
	if err != nil || grant.UserID != delcoreUserID {
		t.Fatalf("resolve = %+v, %v", grant, err)
	}
	dcToken, err := grant.DelcoreToken(context.Background())
	if err != nil || !strings.HasPrefix(dcToken, "dc-token-") {
		t.Fatalf("delcore token %q, %v", dcToken, err)
	}
	// Delcore saw only the Delcore-audience token, for every page.
	headers := e.dc.authHeaders()
	if len(headers) != 2 {
		t.Fatalf("GetCurrentUser calls = %v", headers)
	}
	for _, h := range headers {
		if h != "Bearer "+dcToken {
			t.Fatalf("Delcore received %q", h)
		}
		for _, mcpTok := range issued {
			if strings.Contains(h, mcpTok) {
				t.Fatal("MCP-audience token forwarded to Delcore")
			}
		}
	}
	// The login refreshed the snapshots of every tenant with the Delcore token.
	if !slices.Equal(e.groups.tenants, []string{"tenant-1", "tenant-2"}) || e.groups.tokens[0] != dcToken {
		t.Fatalf("snapshots %v %v", e.groups.tenants, e.groups.tokens)
	}
	// An unknown but valid MCP-audience token resolves to no grant.
	e.up.mu.Lock()
	stray := e.up.mcpToken()
	e.up.mu.Unlock()
	if _, err := e.resolve(stray); apperr.As(err).Reason != mcpav1.ErrorReason_ERROR_REASON_UNAUTHENTICATED {
		t.Fatalf("stray token: %v", err)
	}
}

func TestCallbackRequiresBrowserBindingAndOneUseState(t *testing.T) {
	e := newEnv(t)
	state, cookie := e.startLogin()
	path := oauth.CallbackPath + "?" + url.Values{"code": {"logto-code"}, "state": {state}}.Encode()
	if rec := e.get(path); rec.Code != http.StatusBadRequest || rec.Header().Get("Location") != "" {
		t.Fatalf("no cookie: %d %q", rec.Code, rec.Header().Get("Location"))
	}
	// The state was consumed by the rejected attempt.
	if rec := e.get(path, cookie); rec.Code != http.StatusBadRequest {
		t.Fatalf("replayed state: %d", rec.Code)
	}
	state, cookie = e.startLogin()
	wrong := &http.Cookie{Name: cookie.Name, Value: "forged"}
	if rec := e.get(oauth.CallbackPath+"?"+url.Values{"code": {"logto-code"}, "state": {state}}.Encode(), wrong); rec.Code != http.StatusBadRequest {
		t.Fatalf("forged cookie: %d", rec.Code)
	}
	if e.store.grantCount() != 0 {
		t.Fatal("grant created without browser binding")
	}
}

func TestCallbackCancellationAndExpiry(t *testing.T) {
	e := newEnv(t)
	state, cookie := e.startLogin()
	u := location(t, e.get(oauth.CallbackPath+"?"+url.Values{"error": {"access_denied"}, "state": {state}}.Encode(), cookie))
	if q := u.Query(); q.Get("error") != "access_denied" || q.Get("state") != "client-state" || q.Get("code") != "" {
		t.Fatalf("cancel redirect %s", u)
	}
	state, cookie = e.startLogin()
	e.advance(oauth.RequestLifetime + time.Second)
	if rec := e.get(oauth.CallbackPath+"?"+url.Values{"code": {"logto-code"}, "state": {state}}.Encode(), cookie); rec.Code != http.StatusBadRequest {
		t.Fatalf("expired request: %d", rec.Code)
	}
}

func TestCallbackRequiresMFA(t *testing.T) {
	e := newEnv(t)
	e.up.set(func(u *upstream) { u.mfa = false })
	u := e.login()
	q := u.Query()
	if q.Get("error") != "access_denied" || q.Get("error_uri") != reauthURL || q.Get("code") != "" || q.Get("state") != "client-state" {
		t.Fatalf("redirect %s", u)
	}
	if e.store.grantCount() != 0 {
		t.Fatal("grant created without MFA evidence")
	}
	if len(e.logto.Revoked()) != 1 {
		t.Fatalf("upstream refresh token not revoked: %v", e.logto.Revoked())
	}
	if len(e.dc.authHeaders()) != 0 {
		t.Fatal("Delcore called without MFA evidence")
	}
}

func TestCallbackDelcoreFailureIssuesNoCode(t *testing.T) {
	e := newEnv(t)
	e.dc.mu.Lock()
	e.dc.down = true
	e.dc.mu.Unlock()
	q := e.login().Query()
	if q.Get("error") != "temporarily_unavailable" || q.Get("code") != "" {
		t.Fatalf("redirect %v", q)
	}
	if e.store.grantCount() != 0 {
		t.Fatal("grant created without a Delcore user")
	}
	e.up.mu.Lock()
	current := e.up.current
	e.up.mu.Unlock()
	if !slices.Contains(e.logto.Revoked(), current) {
		t.Fatalf("upstream refresh token %q not revoked: %v", current, e.logto.Revoked())
	}
}

func TestVerifyPKCE(t *testing.T) {
	// RFC 7636 appendix B.
	if !oauth.VerifyPKCE("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk", "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM") {
		t.Fatal("RFC 7636 vector rejected")
	}
	for _, v := range []string{"dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXl", "short", strings.Repeat("a", 129), "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEj+k"} {
		if oauth.VerifyPKCE(v, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM") {
			t.Fatalf("verifier %q accepted", v)
		}
	}
}

func TestTokenExchangeChecksPKCEAndRedirect(t *testing.T) {
	for name, o := range map[string]map[string]string{
		"wrong verifier":  {"code_verifier": "wrong-verifier-" + strings.Repeat("b", 40)},
		"redirect prefix": {"redirect_uri": redirectURI + "/"},
		"other client":    {"client_id": otherClientID},
	} {
		e := newEnv(t)
		code := e.code()
		status, tok := e.exchange(code, o)
		if status != http.StatusBadRequest || tok.Error != "invalid_grant" || tok.AccessToken != "" {
			t.Fatalf("%s: %d %+v", name, status, tok)
		}
		// The spent code's grant is revoked, so a correct retry fails too.
		if status, tok := e.exchange(code, nil); status != http.StatusBadRequest || tok.Error != "invalid_grant" {
			t.Fatalf("%s retry: %d %+v", name, status, tok)
		}
		if e.store.grantCount() != 0 {
			t.Fatalf("%s: grant kept", name)
		}
	}
}

func TestTokenEndpointClientAndForm(t *testing.T) {
	e := newEnv(t)
	code := e.code()
	if status, tok := e.exchange(code, map[string]string{"client_id": "nope"}); status != http.StatusUnauthorized || tok.Error != "invalid_client" {
		t.Fatalf("unknown client: %d %+v", status, tok)
	}
	if status, tok := e.exchange(code, map[string]string{"grant_type": "password"}); status != http.StatusBadRequest || tok.Error != "unsupported_grant_type" {
		t.Fatalf("password grant: %d %+v", status, tok)
	}
	if status, tok := e.exchange(code, map[string]string{"resource": publicURL}); status != http.StatusBadRequest || tok.Error != "invalid_target" {
		t.Fatalf("resource: %d %+v", status, tok)
	}
	rec := e.post(oauth.TokenPath, url.Values{"grant_type": {"authorization_code"}}, "Origin", "https://evil.example.test")
	if rec.Code != http.StatusForbidden {
		t.Fatalf("foreign origin: %d", rec.Code)
	}
	req := httptest.NewRequest(http.MethodPost, oauth.TokenPath, strings.NewReader(`{"grant_type":"authorization_code"}`))
	req.Header.Set("Content-Type", "application/json")
	if rec := e.do(req); rec.Code != http.StatusBadRequest {
		t.Fatalf("JSON body: %d", rec.Code)
	}
	// The rejected requests did not consume the code.
	if status, tok := e.exchange(code, nil); status != http.StatusOK || tok.AccessToken == "" {
		t.Fatalf("exchange after rejections: %d %+v", status, tok)
	}
}

func TestCodeIsOneTimeUse(t *testing.T) {
	e := newEnv(t)
	code := e.code()
	status, tok := e.exchange(code, nil)
	if status != http.StatusOK {
		t.Fatalf("first exchange %d %+v", status, tok)
	}
	status, again := e.exchange(code, nil)
	if status != http.StatusBadRequest || again.Error != "invalid_grant" {
		t.Fatalf("second exchange %d %+v", status, again)
	}
	// Reuse revokes the grant: the first tokens stop working.
	if _, err := e.resolve(tok.AccessToken); apperr.As(err).Reason != mcpav1.ErrorReason_ERROR_REASON_UNAUTHENTICATED {
		t.Fatalf("access token after reuse: %v", err)
	}
	if status, r := e.refresh(tok.RefreshToken, clientID); status != http.StatusBadRequest || r.Error != "invalid_grant" {
		t.Fatalf("refresh after reuse: %d %+v", status, r)
	}
	if len(e.logto.Revoked()) == 0 {
		t.Fatal("upstream refresh token not revoked")
	}
}

func TestCodeExpires(t *testing.T) {
	e := newEnv(t)
	code := e.code()
	e.advance(oauth.CodeLifetime + time.Second)
	if status, tok := e.exchange(code, nil); status != http.StatusBadRequest || tok.Error != "invalid_grant" {
		t.Fatalf("expired code: %d %+v", status, tok)
	}
}

func TestRefreshRotation(t *testing.T) {
	e := newEnv(t)
	_, first := e.exchange(e.code(), nil)
	status, second := e.refresh(first.RefreshToken, clientID)
	if status != http.StatusOK || second.RefreshToken == "" || second.RefreshToken == first.RefreshToken || second.AccessToken == first.AccessToken {
		t.Fatalf("refresh %d %+v", status, second)
	}
	// Both access tokens map to the grant until it is revoked.
	for _, at := range []string{first.AccessToken, second.AccessToken} {
		if g, err := e.resolve(at); err != nil || g.UserID != delcoreUserID {
			t.Fatalf("resolve: %v", err)
		}
	}
	// Another client cannot use the handle.
	if status, r := e.refresh(second.RefreshToken, otherClientID); status != http.StatusBadRequest || r.Error != "invalid_grant" {
		t.Fatalf("other client: %d %+v", status, r)
	}
	// The handle survived the other client's attempt.
	status, third := e.refresh(second.RefreshToken, clientID)
	if status != http.StatusOK {
		t.Fatalf("third refresh %d %+v", status, third)
	}
	// Replaying a retired handle revokes the grant and every token.
	if status, r := e.refresh(first.RefreshToken, clientID); status != http.StatusBadRequest || r.Error != "invalid_grant" {
		t.Fatalf("replay: %d %+v", status, r)
	}
	if status, r := e.refresh(third.RefreshToken, clientID); status != http.StatusBadRequest || r.Error != "invalid_grant" {
		t.Fatalf("after replay: %d %+v", status, r)
	}
	if _, err := e.resolve(third.AccessToken); err == nil {
		t.Fatal("token of a revoked grant resolved")
	}
}

func TestRefreshWithoutMFARevokesGrant(t *testing.T) {
	e := newEnv(t)
	_, tok := e.exchange(e.code(), nil)
	e.up.set(func(u *upstream) { u.mfa = false })
	status, r := e.refresh(tok.RefreshToken, clientID)
	if status != http.StatusBadRequest || r.Error != "invalid_grant" || r.ErrorURI != reauthURL {
		t.Fatalf("refresh without MFA: %d %+v", status, r)
	}
	if e.store.grantCount() != 0 {
		t.Fatal("grant kept after losing MFA evidence")
	}
	if _, err := e.resolve(tok.AccessToken); err == nil {
		t.Fatal("old token still resolves")
	}
}

func TestRefreshRejectedUpstreamRevokesGrant(t *testing.T) {
	e := newEnv(t)
	_, tok := e.exchange(e.code(), nil)
	e.up.set(func(u *upstream) { u.current = "revoked-elsewhere" })
	if status, r := e.refresh(tok.RefreshToken, clientID); status != http.StatusBadRequest || r.Error != "invalid_grant" {
		t.Fatalf("refresh: %d %+v", status, r)
	}
	if e.store.grantCount() != 0 {
		t.Fatal("grant kept after upstream rejection")
	}
}

func TestRevoke(t *testing.T) {
	e := newEnv(t)
	_, tok := e.exchange(e.code(), nil)
	revoke := func(token, client, hint string, header ...string) *httptest.ResponseRecorder {
		form := url.Values{"token": {token}, "client_id": {client}}
		if hint != "" {
			form.Set("token_type_hint", hint)
		}
		return e.post(oauth.RevokePath, form, header...)
	}
	if rec := revoke("unknown", clientID, ""); rec.Code != http.StatusOK {
		t.Fatalf("unknown token: %d", rec.Code)
	}
	if rec := revoke(tok.RefreshToken, otherClientID, ""); rec.Code != http.StatusBadRequest || !strings.Contains(rec.Body.String(), "unauthorized_client") {
		t.Fatalf("other client: %d %s", rec.Code, rec.Body.String())
	}
	if rec := revoke(tok.RefreshToken, "nope", ""); rec.Code != http.StatusUnauthorized {
		t.Fatalf("unknown client: %d", rec.Code)
	}
	if rec := revoke(tok.RefreshToken, clientID, "", "Origin", "https://evil.example.test"); rec.Code != http.StatusForbidden {
		t.Fatalf("foreign origin: %d", rec.Code)
	}
	if e.store.grantCount() != 1 {
		t.Fatal("rejected revocations removed the grant")
	}
	e.up.mu.Lock()
	upstreamRefresh := e.up.current
	e.up.mu.Unlock()
	if rec := revoke(tok.RefreshToken, clientID, "refresh_token"); rec.Code != http.StatusOK {
		t.Fatalf("revoke: %d", rec.Code)
	}
	if e.store.grantCount() != 0 {
		t.Fatal("grant kept after revocation")
	}
	if !slices.Contains(e.logto.Revoked(), upstreamRefresh) {
		t.Fatalf("upstream token %q not revoked: %v", upstreamRefresh, e.logto.Revoked())
	}
	if _, err := e.resolve(tok.AccessToken); err == nil {
		t.Fatal("access token resolves after revocation")
	}
	if status, r := e.refresh(tok.RefreshToken, clientID); status != http.StatusBadRequest || r.Error != "invalid_grant" {
		t.Fatalf("refresh after revoke: %d %+v", status, r)
	}

	// Revoking by access token also ends the grant.
	_, tok = e.exchange(e.code(), nil)
	if rec := revoke(tok.AccessToken, clientID, "access_token"); rec.Code != http.StatusOK || e.store.grantCount() != 0 {
		t.Fatalf("revoke by access token: %d, grants %d", rec.Code, e.store.grantCount())
	}
}

func TestRevokeReportsLocalDeletionFailure(t *testing.T) {
	e := newEnv(t)
	_, tok := e.exchange(e.code(), nil)
	revoke := func() *httptest.ResponseRecorder {
		return e.post(oauth.RevokePath, url.Values{"token": {tok.RefreshToken}, "client_id": {clientID}})
	}
	e.store.setDeleteErr(errors.New("database unavailable"))
	rec := revoke()
	if rec.Code != http.StatusServiceUnavailable || !strings.Contains(rec.Body.String(), "temporarily_unavailable") {
		t.Fatalf("failed deletion: %d %s", rec.Code, rec.Body.String())
	}
	if e.store.grantCount() != 1 || len(e.logto.Revoked()) != 0 {
		t.Fatalf("grants %d, upstream revocations %v after a failed deletion", e.store.grantCount(), e.logto.Revoked())
	}
	// The grant still works, so the retry has something to revoke.
	if _, err := e.resolve(tok.AccessToken); err != nil {
		t.Fatalf("resolve before retry: %v", err)
	}
	e.store.setDeleteErr(nil)
	if rec := revoke(); rec.Code != http.StatusOK || e.store.grantCount() != 0 || len(e.logto.Revoked()) != 1 {
		t.Fatalf("retry: %d, grants %d, upstream revocations %v", rec.Code, e.store.grantCount(), e.logto.Revoked())
	}
}

func TestRevokeSucceedsWhenUpstreamRevocationFails(t *testing.T) {
	e := newEnv(t)
	_, tok := e.exchange(e.code(), nil)
	e.logto.SetRevokeStatus(http.StatusServiceUnavailable)
	rec := e.post(oauth.RevokePath, url.Values{"token": {tok.AccessToken}, "client_id": {clientID}})
	if rec.Code != http.StatusOK || e.store.grantCount() != 0 {
		t.Fatalf("revoke with failing upstream: %d, grants %d", rec.Code, e.store.grantCount())
	}
}

func TestDelcoreTokenRefreshesUnderGrant(t *testing.T) {
	e := newEnv(t)
	_, tok := e.exchange(e.code(), nil)
	grant, err := e.resolve(tok.AccessToken)
	if err != nil {
		t.Fatal(err)
	}
	first, err := grant.DelcoreToken(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	e.advance(2 * time.Hour)
	second, err := grant.DelcoreToken(context.Background())
	if err != nil || second == first || !strings.HasPrefix(second, "dc-token-") {
		t.Fatalf("refreshed delcore token %q (was %q), %v", second, first, err)
	}
	// The rotated upstream refresh token was stored: the MCP refresh works.
	e.advance(-2 * time.Hour)
	if status, r := e.refresh(tok.RefreshToken, clientID); status != http.StatusOK {
		t.Fatalf("refresh after Delcore refresh: %d %+v", status, r)
	}
}

func TestCleanupRemovesExpiredState(t *testing.T) {
	e := newEnv(t)
	_, _ = e.exchange(e.code(), nil)
	e.startLogin()
	e.advance(oauth.GrantIdleLifetime + time.Hour)
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { done <- e.broker.RunCleanup(ctx) }()
	deadline := time.Now().Add(5 * time.Second)
	for e.store.grantCount() != 0 && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	cancel()
	if err := <-done; err != nil {
		t.Fatal(err)
	}
	if e.store.grantCount() != 0 || len(e.store.requests) != 0 {
		t.Fatalf("grants %d, requests %d after cleanup", e.store.grantCount(), len(e.store.requests))
	}
}
