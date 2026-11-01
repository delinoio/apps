// SPDX-License-Identifier: FSL-1.1-ALv2

package provideroauth

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/tidwall/gjson"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/proxy"
)

// fakeProvider records requests and answers with scripted handlers.
type fakeProvider struct {
	srv *httptest.Server
	mu  sync.Mutex
	// requests holds "METHOD path" with the parsed body of each request.
	requests []recorded
	handlers map[string]http.HandlerFunc
}

type recorded struct {
	method, path string
	header       http.Header
	form         url.Values
	json         gjson.Result
}

func newFakeProvider(t *testing.T) *fakeProvider {
	t.Helper()
	f := &fakeProvider{handlers: map[string]http.HandlerFunc{}}
	f.srv = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		rec := recorded{method: r.Method, path: r.URL.Path, header: r.Header.Clone()}
		if strings.HasPrefix(r.Header.Get("Content-Type"), "application/x-www-form-urlencoded") {
			rec.form, _ = url.ParseQuery(string(body))
		} else {
			rec.json = gjson.ParseBytes(body)
		}
		f.mu.Lock()
		f.requests = append(f.requests, rec)
		h := f.handlers[r.URL.Path]
		f.mu.Unlock()
		if h == nil {
			http.NotFound(w, r)
			return
		}
		r.Body = io.NopCloser(strings.NewReader(string(body)))
		h(w, r)
	}))
	t.Cleanup(f.srv.Close)
	return f
}

func (f *fakeProvider) handle(path string, h http.HandlerFunc) {
	f.mu.Lock()
	f.handlers[path] = h
	f.mu.Unlock()
}

func (f *fakeProvider) last(path string) recorded {
	f.mu.Lock()
	defer f.mu.Unlock()
	for i := len(f.requests) - 1; i >= 0; i-- {
		if f.requests[i].path == path {
			return f.requests[i]
		}
	}
	return recorded{}
}

func (f *fakeProvider) count(path string) int {
	f.mu.Lock()
	defer f.mu.Unlock()
	n := 0
	for _, r := range f.requests {
		if r.path == path {
			n++
		}
	}
	return n
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

// rewriteTransport sends every request to the fake provider, keeping the
// path and query. It stands in for fixed SDK endpoints.
type rewriteTransport struct{ target *url.URL }

func (t rewriteTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	out := req.Clone(req.Context())
	out.URL.Scheme = t.target.Scheme
	out.URL.Host = t.target.Host
	out.Host = t.target.Host
	return http.DefaultTransport.RoundTrip(out)
}

var fixedNow = time.Date(2026, 11, 1, 12, 0, 0, 0, time.UTC)

func testBroker(t *testing.T, f *fakeProvider) *Broker {
	t.Helper()
	base := f.srv.URL
	target, _ := url.Parse(base)
	client := &http.Client{Timeout: 5 * time.Second, Transport: rewriteTransport{target: target}}
	return New(Options{
		Log:              slog.New(slog.NewTextHandler(io.Discard, nil)),
		HTTPClient:       client,
		ClaudeHTTPClient: &http.Client{Timeout: 5 * time.Second},
		Timeout:          5 * time.Second,
		SetupTimeout:     10 * time.Second,
		Now:              func() time.Time { return fixedNow },
		Endpoints: Endpoints{
			ClaudeAuthURL: base + "/claude/authorize", ClaudeTokenURL: base + "/claude/token",
			CodexTokenURL: base + "/codex/token", CodexDeviceUserCodeURL: base + "/codex/usercode",
			CodexDeviceTokenURL: base + "/codex/devicetoken", CodexDeviceVerificationURL: "https://auth.openai.com/codex/device",
			GoogleAuthURL: base + "/google/auth", GoogleTokenURL: base + "/google/token",
			GeminiUserInfoURL: base + "/google/userinfo", GeminiCodeAssist: base,
			AntigravityAuthURL: base + "/google/v2/auth", AntigravityUserInfoURL: base + "/google/v2/userinfo",
			KimiDeviceAuthorizationURL: base + "/kimi/device", KimiTokenURL: base + "/kimi/token",
		},
	})
}

// validate checks that the engine accepts a token payload for provider.
func validate(t *testing.T, provider mcpav1.Provider, tok token) {
	t.Helper()
	raw, err := json.Marshal(tok.payload)
	if err != nil {
		t.Fatal(err)
	}
	err = proxy.ValidateCredential(proxy.Credential{
		ID: uuid.NewString(), ResourceID: uuid.NewString(), Provider: provider,
		Kind: mcpav1.CredentialKind_CREDENTIAL_KIND_OAUTH, Secret: raw,
	})
	if err != nil {
		t.Fatalf("engine rejects the %s payload: %v", provider, err)
	}
}

func TestAuthorizationURLsCarryPKCEStateAndRegisteredRedirects(t *testing.T) {
	b := testBroker(t, newFakeProvider(t))
	cases := []struct {
		provider mcpav1.Provider
		redirect string
		client   string
	}{
		{mcpav1.Provider_PROVIDER_CLAUDE, claudeRedirectURI, claudeClientID},
		{mcpav1.Provider_PROVIDER_GEMINI_CLI, geminiRedirectURI, geminiClientID},
		{mcpav1.Provider_PROVIDER_ANTIGRAVITY, antigravityRedirectURI, antigravityClientID},
	}
	for _, tc := range cases {
		spec := providers[tc.provider]
		if spec.kind != mcpav1.OauthFlowKind_OAUTH_FLOW_KIND_AUTHORIZATION_CODE {
			t.Fatalf("%s kind %s", tc.provider, spec.kind)
		}
		u, err := url.Parse(spec.authURL(b, "state-1", "challenge-1"))
		if err != nil {
			t.Fatal(err)
		}
		q := u.Query()
		if q.Get("state") != "state-1" || q.Get("code_challenge") != "challenge-1" || q.Get("code_challenge_method") != "S256" ||
			q.Get("redirect_uri") != tc.redirect || q.Get("client_id") != tc.client || q.Get("response_type") != "code" {
			t.Fatalf("%s authorization URL %s", tc.provider, u)
		}
	}
	for _, p := range []mcpav1.Provider{mcpav1.Provider_PROVIDER_CODEX, mcpav1.Provider_PROVIDER_KIMI} {
		if kind, _ := FlowKind(p); kind != mcpav1.OauthFlowKind_OAUTH_FLOW_KIND_DEVICE_CODE {
			t.Fatalf("%s kind %s", p, kind)
		}
	}
}

func TestPKCEVerifierMatchesChallenge(t *testing.T) {
	verifier, challenge, err := newPKCE()
	if err != nil {
		t.Fatal(err)
	}
	if len(verifier) != 128 || len(challenge) != 43 {
		t.Fatalf("verifier %d chars, challenge %d chars", len(verifier), len(challenge))
	}
	other, _, _ := newPKCE()
	if other == verifier {
		t.Fatal("verifiers repeat")
	}
}

func TestParseCallback(t *testing.T) {
	cases := []struct {
		raw              string
		code, state, err string
		bad              bool
	}{
		{raw: "http://localhost:54545/callback?code=abc%23st1&state=st1", code: "abc", state: "st1"},
		{raw: "http://localhost:54545/callback?code=abc%23st1", code: "abc", state: "st1"},
		{raw: "http://localhost:8085/oauth2callback?state=s&code=4%2F0A&scope=x", code: "4/0A", state: "s"},
		{raw: "http://localhost:51121/oauth-callback#code=frag&state=s2", code: "frag", state: "s2"},
		{raw: "http://localhost:8085/oauth2callback?error=access_denied&state=s", err: "access_denied", state: "s"},
		{raw: "http://localhost:8085/oauth2callback?state=s", bad: true},
		{raw: "", bad: true},
	}
	for _, tc := range cases {
		cb, err := parseCallback(tc.raw)
		if tc.bad {
			if err == nil {
				t.Fatalf("%q parsed", tc.raw)
			}
			continue
		}
		if err != nil || cb.code != tc.code || cb.state != tc.state || cb.err != tc.err {
			t.Fatalf("%q: %+v %v", tc.raw, cb, err)
		}
	}
}

func TestClaudeExchange(t *testing.T) {
	f := newFakeProvider(t)
	f.handle("/claude/token", func(w http.ResponseWriter, r *http.Request) {
		writeJSON(w, 200, map[string]any{
			"access_token": "sk-ant-oat-access", "refresh_token": "sk-ant-ort-refresh", "token_type": "Bearer", "expires_in": 3600,
			"account": map[string]any{"uuid": "a1", "email_address": "dev@example.com"},
		})
	})
	b := testBroker(t, f)
	tok, err := claudeExchange(context.Background(), b, "the-code", "st1", flowSecret{State: "st1", CodeVerifier: "verifier-1"})
	if err != nil {
		t.Fatal(err)
	}
	req := f.last("/claude/token")
	if req.json.Get("grant_type").String() != "authorization_code" || req.json.Get("code").String() != "the-code" ||
		req.json.Get("code_verifier").String() != "verifier-1" || req.json.Get("state").String() != "st1" ||
		req.json.Get("client_id").String() != claudeClientID || req.json.Get("redirect_uri").String() != claudeRedirectURI {
		t.Fatalf("token request %s", req.json.Raw)
	}
	if tok.label != "dev@example.com" || tok.payload["type"] != "claude" || tok.payload["access_token"] != "sk-ant-oat-access" ||
		tok.payload["refresh_token"] != "sk-ant-ort-refresh" || tok.payload["expired"] != fixedNow.Add(time.Hour).Format(time.RFC3339) {
		t.Fatalf("payload %+v", tok.payload)
	}
	validate(t, mcpav1.Provider_PROVIDER_CLAUDE, tok)

	f.handle("/claude/token", func(w http.ResponseWriter, r *http.Request) {
		writeJSON(w, 400, map[string]any{"error": "invalid_grant", "error_description": "code sk-ant-secret is used"})
	})
	_, err = claudeExchange(context.Background(), b, "used", "st1", flowSecret{State: "st1", CodeVerifier: "v"})
	var se *statusError
	if !errors.As(err, &se) || se.status != 400 || strings.Contains(err.Error(), "sk-ant") {
		t.Fatalf("error %v leaks or is untyped", err)
	}
}

func TestGeminiExchangeSetsUpCodeAssistProject(t *testing.T) {
	f := newFakeProvider(t)
	f.handle("/google/token", func(w http.ResponseWriter, r *http.Request) {
		writeJSON(w, 200, map[string]any{"access_token": "ya29.access", "refresh_token": "1//refresh", "token_type": "Bearer", "expires_in": 3599})
	})
	f.handle("/google/userinfo", func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer ya29.access" {
			w.WriteHeader(401)
			return
		}
		writeJSON(w, 200, map[string]any{"email": "g@example.com"})
	})
	f.handle("/v1internal:loadCodeAssist", func(w http.ResponseWriter, r *http.Request) {
		writeJSON(w, 200, map[string]any{"allowedTiers": []any{map[string]any{"id": "free-tier", "isDefault": true}}})
	})
	var onboards int
	f.handle("/v1internal:onboardUser", func(w http.ResponseWriter, r *http.Request) {
		onboards++
		if onboards == 1 {
			writeJSON(w, 200, map[string]any{"done": false})
			return
		}
		writeJSON(w, 200, map[string]any{"done": true, "response": map[string]any{"cloudaicompanionProject": map[string]any{"id": "proj-123"}}})
	})
	b := testBroker(t, f)
	tok, err := geminiExchange(context.Background(), b, "4/code", "", flowSecret{CodeVerifier: "verifier-g"})
	if err != nil {
		t.Fatal(err)
	}
	form := f.last("/google/token").form
	if form.Get("grant_type") != "authorization_code" || form.Get("code") != "4/code" || form.Get("code_verifier") != "verifier-g" ||
		form.Get("client_id") != geminiClientID || form.Get("client_secret") != geminiClientSecret || form.Get("redirect_uri") != geminiRedirectURI {
		t.Fatalf("token form %v", form)
	}
	onboard := f.last("/v1internal:onboardUser")
	if onboard.json.Get("tierId").String() != "free-tier" || onboard.json.Get("cloudaicompanionProject").String() != "proj-123" {
		t.Fatalf("final onboard %s", onboard.json.Raw)
	}
	if tok.payload["project_id"] != "proj-123" || tok.payload["email"] != "g@example.com" || tok.payload["type"] != "gemini" {
		t.Fatalf("payload %+v", tok.payload)
	}
	inner, _ := tok.payload["token"].(map[string]any)
	if inner["access_token"] != "ya29.access" || inner["refresh_token"] != "1//refresh" || inner["client_id"] != geminiClientID || inner["token_uri"] != googleTokenURL {
		t.Fatalf("token object %+v", inner)
	}
	validate(t, mcpav1.Provider_PROVIDER_GEMINI_CLI, tok)
}

func TestAntigravityExchangeFetchesProject(t *testing.T) {
	f := newFakeProvider(t)
	f.handle("/google/token", func(w http.ResponseWriter, r *http.Request) {
		writeJSON(w, 200, map[string]any{"access_token": "ya29.ag", "refresh_token": "1//ag", "token_type": "Bearer", "expires_in": 3599})
	})
	f.handle("/google/v2/userinfo", func(w http.ResponseWriter, r *http.Request) {
		writeJSON(w, 200, map[string]any{"email": "ag@example.com"})
	})
	// sdkauth.FetchAntigravityProjectID calls the fixed Code Assist URL; the
	// broker's client routes it here.
	f.handle("/v1internal:loadCodeAssist", func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer ya29.ag" {
			w.WriteHeader(401)
			return
		}
		writeJSON(w, 200, map[string]any{"cloudaicompanionProject": "ag-project"})
	})
	b := testBroker(t, f)
	tok, err := antigravityExchange(context.Background(), b, "code-ag", "", flowSecret{CodeVerifier: "verifier-ag"})
	if err != nil {
		t.Fatal(err)
	}
	form := f.last("/google/token").form
	if form.Get("client_id") != antigravityClientID || form.Get("redirect_uri") != antigravityRedirectURI || form.Get("code_verifier") != "verifier-ag" {
		t.Fatalf("token form %v", form)
	}
	if tok.payload["project_id"] != "ag-project" || tok.payload["email"] != "ag@example.com" || tok.payload["type"] != "antigravity" ||
		tok.payload["access_token"] != "ya29.ag" {
		t.Fatalf("payload %+v", tok.payload)
	}
	validate(t, mcpav1.Provider_PROVIDER_ANTIGRAVITY, tok)
}

func fakeIDToken(claims map[string]any) string {
	raw, _ := json.Marshal(claims)
	return "eyJhbGciOiJub25lIn0." + base64.RawURLEncoding.EncodeToString(raw) + ".sig"
}

func TestCodexDeviceFlow(t *testing.T) {
	f := newFakeProvider(t)
	f.handle("/codex/usercode", func(w http.ResponseWriter, r *http.Request) {
		writeJSON(w, 200, map[string]any{"device_auth_id": "dev-auth-1", "user_code": "ABCD-EFGH", "interval": "7"})
	})
	approved := false
	f.handle("/codex/devicetoken", func(w http.ResponseWriter, r *http.Request) {
		if !approved {
			w.WriteHeader(http.StatusForbidden)
			return
		}
		writeJSON(w, 200, map[string]any{"authorization_code": "auth-code-1", "code_verifier": "server-verifier", "code_challenge": "c"})
	})
	f.handle("/codex/token", func(w http.ResponseWriter, r *http.Request) {
		writeJSON(w, 200, map[string]any{
			"access_token": "codex-access", "refresh_token": "codex-refresh", "expires_in": 864000,
			"id_token": fakeIDToken(map[string]any{
				"email":                       "c@example.com",
				"https://api.openai.com/auth": map[string]any{"chatgpt_account_id": "acct-9", "chatgpt_plan_type": "plus"},
			}),
		})
	})
	b := testBroker(t, f)
	start, err := codexStartDevice(context.Background(), b)
	if err != nil {
		t.Fatal(err)
	}
	if start.userCode != "ABCD-EFGH" || start.secret.DeviceCode != "dev-auth-1" || start.secret.IntervalSeconds != 7 ||
		start.verificationURI != "https://auth.openai.com/codex/device" || start.lifetime != 15*time.Minute {
		t.Fatalf("start %+v", start)
	}
	if f.last("/codex/usercode").json.Get("client_id").String() != codexClientID {
		t.Fatal("user code request lacks the client ID")
	}
	sec := start.secret
	tok, err := codexPoll(context.Background(), b, &sec)
	if err != nil || tok != nil {
		t.Fatalf("pending poll: %v %v", tok, err)
	}
	approved = true
	tok, err = codexPoll(context.Background(), b, &sec)
	if err != nil || tok == nil {
		t.Fatalf("approved poll: %v %v", tok, err)
	}
	poll := f.last("/codex/devicetoken").json
	if poll.Get("device_auth_id").String() != "dev-auth-1" || poll.Get("user_code").String() != "ABCD-EFGH" {
		t.Fatalf("poll body %s", poll.Raw)
	}
	form := f.last("/codex/token").form
	if form.Get("code") != "auth-code-1" || form.Get("code_verifier") != "server-verifier" || form.Get("redirect_uri") != codexDeviceTokenExchangeURI {
		t.Fatalf("exchange form %v", form)
	}
	if tok.label != "c@example.com" || tok.payload["account_id"] != "acct-9" || tok.payload["type"] != "codex" {
		t.Fatalf("payload %+v", tok.payload)
	}
	validate(t, mcpav1.Provider_PROVIDER_CODEX, *tok)

	f.handle("/codex/devicetoken", func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(500) })
	if _, err := codexPoll(context.Background(), b, &sec); err == nil {
		t.Fatal("a provider failure did not fail the poll")
	}
}

func TestKimiDeviceFlow(t *testing.T) {
	f := newFakeProvider(t)
	f.handle("/kimi/device", func(w http.ResponseWriter, r *http.Request) {
		writeJSON(w, 200, map[string]any{
			"device_code": "kimi-device", "user_code": "KIMI-1", "verification_uri": "https://www.kimi.com/device",
			"verification_uri_complete": "https://www.kimi.com/device?code=KIMI-1", "expires_in": 600, "interval": 3,
		})
	})
	answers := []map[string]any{{"error": "authorization_pending"}, {"error": "slow_down"},
		{"access_token": "kimi-access", "refresh_token": "kimi-refresh", "token_type": "Bearer", "expires_in": 3600, "scope": "kimi-code"}}
	f.handle("/kimi/token", func(w http.ResponseWriter, r *http.Request) {
		next := answers[0]
		answers = answers[1:]
		writeJSON(w, 200, next)
	})
	b := testBroker(t, f)
	start, err := kimiStartDevice(context.Background(), b)
	if err != nil {
		t.Fatal(err)
	}
	if start.userCode != "KIMI-1" || start.verificationURI != "https://www.kimi.com/device?code=KIMI-1" ||
		start.secret.IntervalSeconds != 5 || start.lifetime != 10*time.Minute || start.secret.DeviceID == "" {
		t.Fatalf("start %+v", start)
	}
	device := f.last("/kimi/device")
	if device.form.Get("client_id") != kimiClientID || device.header.Get("X-Msh-Device-Id") != start.secret.DeviceID {
		t.Fatalf("device request %v %v", device.form, device.header)
	}
	sec := start.secret
	if tok, err := kimiPoll(context.Background(), b, &sec); tok != nil || err != nil {
		t.Fatalf("pending: %v %v", tok, err)
	}
	if tok, err := kimiPoll(context.Background(), b, &sec); tok != nil || err != nil || sec.IntervalSeconds != 10 {
		t.Fatalf("slow_down: %v %v interval %d", tok, err, sec.IntervalSeconds)
	}
	tok, err := kimiPoll(context.Background(), b, &sec)
	if err != nil || tok == nil {
		t.Fatalf("approved: %v %v", tok, err)
	}
	form := f.last("/kimi/token").form
	if form.Get("grant_type") != kimiDeviceGrantType || form.Get("device_code") != "kimi-device" {
		t.Fatalf("token form %v", form)
	}
	if tok.payload["access_token"] != "kimi-access" || tok.payload["device_id"] != sec.DeviceID || tok.payload["type"] != "kimi" {
		t.Fatalf("payload %+v", tok.payload)
	}
	validate(t, mcpav1.Provider_PROVIDER_KIMI, *tok)

	f.handle("/kimi/token", func(w http.ResponseWriter, r *http.Request) {
		writeJSON(w, 400, map[string]any{"error": "access_denied"})
	})
	_, err = kimiPoll(context.Background(), b, &sec)
	var fe *flowError
	if !errors.As(err, &fe) || fe.state != mcpav1.OauthFlowState_OAUTH_FLOW_STATE_FAILED {
		t.Fatalf("denied: %v", err)
	}
	f.handle("/kimi/token", func(w http.ResponseWriter, r *http.Request) {
		writeJSON(w, 400, map[string]any{"error": "expired_token"})
	})
	if _, err = kimiPoll(context.Background(), b, &sec); !errors.As(err, &fe) || fe.state != mcpav1.OauthFlowState_OAUTH_FLOW_STATE_EXPIRED {
		t.Fatalf("expired: %v", err)
	}
}

func TestBrowserTLSClientRequiresHTTPS(t *testing.T) {
	c := NewBrowserTLSClient(time.Second)
	if _, err := c.Get("http://127.0.0.1:1/"); err == nil {
		t.Fatal("plain HTTP was accepted")
	}
}
