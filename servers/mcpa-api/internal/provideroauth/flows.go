// SPDX-License-Identifier: FSL-1.1-ALv2

package provideroauth

import (
	"bytes"
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"runtime"
	"strconv"
	"strings"
	"time"

	"github.com/google/uuid"
	sdkauth "github.com/router-for-me/CLIProxyAPI/v6/sdk/auth"
	"github.com/tidwall/gjson"
	"golang.org/x/oauth2"
)

// maxProviderBody bounds provider responses.
const maxProviderBody = 1 << 20

func randomToken(n int) (string, error) {
	b := make([]byte, n)
	if _, err := rand.Read(b); err != nil {
		return "", err
	}
	return base64.RawURLEncoding.EncodeToString(b), nil
}

// newPKCE returns an S256 verifier and challenge (96 random bytes, as the
// SDK generates).
func newPKCE() (string, string, error) {
	verifier, err := randomToken(96)
	if err != nil {
		return "", "", err
	}
	sum := sha256.Sum256([]byte(verifier))
	return verifier, base64.RawURLEncoding.EncodeToString(sum[:]), nil
}

// statusError is a provider HTTP failure. It carries the status only:
// provider bodies can echo codes or tokens and are never logged.
type statusError struct {
	op     string
	status int
}

func (e *statusError) Error() string {
	return fmt.Sprintf("%s: provider returned HTTP %d", e.op, e.status)
}

// do sends a request and returns the status and a bounded body.
func do(ctx context.Context, client *http.Client, method, target string, header http.Header, body []byte) (int, []byte, error) {
	var reader io.Reader
	if body != nil {
		reader = bytes.NewReader(body)
	}
	req, err := http.NewRequestWithContext(ctx, method, target, reader)
	if err != nil {
		return 0, nil, err
	}
	for k, v := range header {
		req.Header[k] = v
	}
	resp, err := client.Do(req)
	if err != nil {
		return 0, nil, err
	}
	defer resp.Body.Close()
	data, err := io.ReadAll(io.LimitReader(resp.Body, maxProviderBody))
	if err != nil {
		return resp.StatusCode, nil, err
	}
	return resp.StatusCode, data, nil
}

func jsonHeader() http.Header {
	return http.Header{"Content-Type": {"application/json"}, "Accept": {"application/json"}}
}

func formHeader() http.Header {
	return http.Header{"Content-Type": {"application/x-www-form-urlencoded"}, "Accept": {"application/json"}}
}

// callJSON sends a request and decodes a 2xx JSON answer into out.
func (b *Broker) callJSON(ctx context.Context, client *http.Client, op, method, target string, header http.Header, body []byte, out any) error {
	ctx, cancel := context.WithTimeout(ctx, b.timeout)
	defer cancel()
	status, data, err := do(ctx, client, method, target, header, body)
	if err != nil {
		return fmt.Errorf("%s: %w", op, err)
	}
	if status < 200 || status >= 300 {
		return &statusError{op: op, status: status}
	}
	if out == nil {
		return nil
	}
	if err := json.Unmarshal(data, out); err != nil {
		return fmt.Errorf("%s: invalid provider response", op)
	}
	return nil
}

func expiry(now time.Time, seconds int64) string {
	return now.Add(time.Duration(seconds) * time.Second).Format(time.RFC3339)
}

// googleToken is the token endpoint answer of Google, Codex and Claude.
type tokenResponse struct {
	AccessToken  string `json:"access_token"`
	RefreshToken string `json:"refresh_token"`
	IDToken      string `json:"id_token"`
	TokenType    string `json:"token_type"`
	ExpiresIn    int64  `json:"expires_in"`
	Scope        string `json:"scope"`
}

func (t tokenResponse) check() error {
	if strings.TrimSpace(t.AccessToken) == "" {
		return failed("the provider returned no access token")
	}
	return nil
}

// --- Claude Code ---

func claudeAuthorizationURL(b *Broker, state, challenge string) string {
	q := url.Values{
		"code":                  {"true"},
		"client_id":             {claudeClientID},
		"response_type":         {"code"},
		"redirect_uri":          {claudeRedirectURI},
		"scope":                 {claudeScope},
		"code_challenge":        {challenge},
		"code_challenge_method": {"S256"},
		"state":                 {state},
	}
	return b.ep.ClaudeAuthURL + "?" + q.Encode()
}

func claudeExchange(ctx context.Context, b *Broker, code, callbackState string, sec flowSecret) (token, error) {
	state := callbackState
	if state == "" {
		state = sec.State
	}
	body, err := json.Marshal(map[string]any{
		"code": code, "state": state, "grant_type": "authorization_code", "client_id": claudeClientID,
		"redirect_uri": claudeRedirectURI, "code_verifier": sec.CodeVerifier,
	})
	if err != nil {
		return token{}, err
	}
	var resp struct {
		tokenResponse
		Account struct {
			EmailAddress string `json:"email_address"`
		} `json:"account"`
	}
	if err := b.callJSON(ctx, b.claude, "claude token exchange", http.MethodPost, b.ep.ClaudeTokenURL, jsonHeader(), body, &resp); err != nil {
		return token{}, err
	}
	if err := resp.check(); err != nil {
		return token{}, err
	}
	now := b.now()
	email := strings.TrimSpace(resp.Account.EmailAddress)
	payload := map[string]any{
		"type":          "claude",
		"id_token":      "",
		"access_token":  resp.AccessToken,
		"refresh_token": resp.RefreshToken,
		"last_refresh":  now.Format(time.RFC3339),
		"email":         email,
		"expired":       expiry(now, resp.ExpiresIn),
	}
	return token{payload: payload, label: labelOr(email, "Claude account")}, nil
}

func labelOr(v, fallback string) string {
	if v = strings.TrimSpace(v); v != "" {
		return v
	}
	return fallback
}

// --- Google (Gemini CLI and Antigravity) ---

func googleAuthorizationURL(base, clientID, redirectURI string, scopes []string, state, challenge string) string {
	q := url.Values{
		"access_type":           {"offline"},
		"client_id":             {clientID},
		"prompt":                {"consent"},
		"redirect_uri":          {redirectURI},
		"response_type":         {"code"},
		"scope":                 {strings.Join(scopes, " ")},
		"state":                 {state},
		"code_challenge":        {challenge},
		"code_challenge_method": {"S256"},
	}
	return base + "?" + q.Encode()
}

func (b *Broker) googleExchange(ctx context.Context, op, clientID, clientSecret, redirectURI, code, verifier string) (tokenResponse, error) {
	form := url.Values{
		"code": {code}, "client_id": {clientID}, "client_secret": {clientSecret}, "redirect_uri": {redirectURI},
		"grant_type": {"authorization_code"}, "code_verifier": {verifier},
	}
	var resp tokenResponse
	if err := b.callJSON(ctx, b.http, op, http.MethodPost, b.ep.GoogleTokenURL, formHeader(), []byte(form.Encode()), &resp); err != nil {
		return tokenResponse{}, err
	}
	return resp, resp.check()
}

// googleEmail reads the account email. Failures return "" so a credential
// can still be connected without a label.
func (b *Broker) googleEmail(ctx context.Context, userInfoURL, accessToken string) string {
	var info struct {
		Email string `json:"email"`
	}
	header := http.Header{"Authorization": {"Bearer " + accessToken}}
	if err := b.callJSON(ctx, b.http, "google userinfo", http.MethodGet, userInfoURL, header, nil, &info); err != nil {
		b.log.WarnContext(ctx, "reading the Google account email failed", "error", err)
		return ""
	}
	return strings.TrimSpace(info.Email)
}

func geminiAuthorizationURL(b *Broker, state, challenge string) string {
	return googleAuthorizationURL(b.ep.GoogleAuthURL, geminiClientID, geminiRedirectURI, geminiScopes, state, challenge)
}

func geminiExchange(ctx context.Context, b *Broker, code, _ string, sec flowSecret) (token, error) {
	resp, err := b.googleExchange(ctx, "gemini token exchange", geminiClientID, geminiClientSecret, geminiRedirectURI, code, sec.CodeVerifier)
	if err != nil {
		return token{}, err
	}
	email := b.googleEmail(ctx, b.ep.GeminiUserInfoURL, resp.AccessToken)
	projectID, err := b.geminiProject(ctx, resp.AccessToken)
	if err != nil {
		return token{}, err
	}
	now := b.now()
	// The token object matches what the SDK's Gemini login stores: the
	// oauth2.Token JSON plus the client and scopes the SDK refresh needs.
	tok := &oauth2.Token{AccessToken: resp.AccessToken, TokenType: resp.TokenType, RefreshToken: resp.RefreshToken}
	if resp.ExpiresIn > 0 {
		tok.Expiry = now.Add(time.Duration(resp.ExpiresIn) * time.Second)
	}
	raw, err := json.Marshal(tok)
	if err != nil {
		return token{}, err
	}
	tokenMap := map[string]any{}
	if err := json.Unmarshal(raw, &tokenMap); err != nil {
		return token{}, err
	}
	tokenMap["token_uri"] = googleTokenURL
	tokenMap["client_id"] = geminiClientID
	tokenMap["client_secret"] = geminiClientSecret
	tokenMap["scopes"] = geminiScopes
	tokenMap["universe_domain"] = "googleapis.com"
	payload := map[string]any{
		"type":       "gemini",
		"token":      tokenMap,
		"project_id": projectID,
		"email":      email,
		"auto":       false,
		"checked":    false,
	}
	return token{payload: payload, label: labelOr(email, "Gemini CLI account")}, nil
}

func geminiUserAgent() string {
	osName := runtime.GOOS
	if osName == "windows" {
		osName = "win32"
	}
	arch := runtime.GOARCH
	switch arch {
	case "amd64":
		arch = "x64"
	case "386":
		arch = "x86"
	}
	return fmt.Sprintf("GeminiCLI/%s/unknown (%s; %s; terminal)", geminiCLIVersion, osName, arch)
}

// geminiProject finds or provisions the Gemini Code Assist project of the
// account, as the SDK's Gemini login does (performGeminiCLISetup).
func (b *Broker) geminiProject(ctx context.Context, accessToken string) (string, error) {
	ctx, cancel := context.WithTimeout(ctx, b.setup)
	defer cancel()
	header := http.Header{
		"Authorization": {"Bearer " + accessToken}, "Content-Type": {"application/json"}, "User-Agent": {geminiUserAgent()},
	}
	metadata := map[string]string{"ideType": "IDE_UNSPECIFIED", "platform": "PLATFORM_UNSPECIFIED", "pluginType": "GEMINI"}
	call := func(method string, body any) (gjson.Result, error) {
		raw, err := json.Marshal(body)
		if err != nil {
			return gjson.Result{}, err
		}
		var out json.RawMessage
		target := strings.TrimSuffix(b.ep.GeminiCodeAssist, "/") + "/v1internal:" + method
		if err := b.callJSON(ctx, b.http, "gemini "+method, http.MethodPost, target, header, raw, &out); err != nil {
			return gjson.Result{}, err
		}
		return gjson.ParseBytes(out), nil
	}
	projectOf := func(v gjson.Result) string {
		if v.Type == gjson.String {
			return strings.TrimSpace(v.String())
		}
		return strings.TrimSpace(v.Get("id").String())
	}
	load, err := call("loadCodeAssist", map[string]any{"metadata": metadata})
	if err != nil {
		return "", err
	}
	tier := "legacy-tier"
	for _, t := range load.Get("allowedTiers").Array() {
		if t.Get("isDefault").Bool() && strings.TrimSpace(t.Get("id").String()) != "" {
			tier = strings.TrimSpace(t.Get("id").String())
			break
		}
	}
	onboard := func(body map[string]any) (string, error) {
		for {
			resp, err := call("onboardUser", body)
			if err != nil {
				return "", err
			}
			if resp.Get("done").Bool() {
				return projectOf(resp.Get("response.cloudaicompanionProject")), nil
			}
			select {
			case <-ctx.Done():
				return "", failed("Google did not finish provisioning the Gemini Code Assist project in time; start a new flow")
			case <-time.After(2 * time.Second):
			}
		}
	}
	project := projectOf(load.Get("cloudaicompanionProject"))
	if project == "" {
		if project, err = onboard(map[string]any{"tierId": tier, "metadata": metadata}); err != nil {
			return "", err
		}
		if project == "" {
			return "", failed("the Google account has no Gemini Code Assist project")
		}
	}
	final, err := onboard(map[string]any{"tierId": tier, "metadata": metadata, "cloudaicompanionProject": project})
	if err != nil {
		return "", err
	}
	if final != "" {
		project = final
	}
	return project, nil
}

func antigravityAuthorizationURL(b *Broker, state, challenge string) string {
	return googleAuthorizationURL(b.ep.AntigravityAuthURL, antigravityClientID, antigravityRedirectURI, antigravityScopes, state, challenge)
}

func antigravityExchange(ctx context.Context, b *Broker, code, _ string, sec flowSecret) (token, error) {
	resp, err := b.googleExchange(ctx, "antigravity token exchange", antigravityClientID, antigravityClientSecret, antigravityRedirectURI, code, sec.CodeVerifier)
	if err != nil {
		return token{}, err
	}
	email := b.googleEmail(ctx, b.ep.AntigravityUserInfoURL, resp.AccessToken)
	projectCtx, cancel := context.WithTimeout(ctx, b.timeout)
	projectID, err := sdkauth.FetchAntigravityProjectID(projectCtx, resp.AccessToken, b.http)
	cancel()
	if err != nil {
		// The SDK login also continues without a project.
		b.log.WarnContext(ctx, "antigravity project lookup failed; connecting without a project", "error", err)
		projectID = ""
	}
	now := b.now()
	payload := map[string]any{
		"type":          "antigravity",
		"access_token":  resp.AccessToken,
		"refresh_token": resp.RefreshToken,
		"expires_in":    resp.ExpiresIn,
		"timestamp":     now.UnixMilli(),
		"expired":       expiry(now, resp.ExpiresIn),
	}
	if email != "" {
		payload["email"] = email
	}
	if projectID != "" {
		payload["project_id"] = projectID
	}
	return token{payload: payload, label: labelOr(email, "Antigravity account")}, nil
}

// --- Codex device login ---

func pollInterval(raw json.RawMessage, def int) int {
	var s string
	if json.Unmarshal(raw, &s) == nil {
		if n, err := strconv.Atoi(strings.TrimSpace(s)); err == nil && n > 0 {
			return n
		}
	}
	var n int
	if json.Unmarshal(raw, &n) == nil && n > 0 {
		return n
	}
	return def
}

func codexStartDevice(ctx context.Context, b *Broker) (deviceStart, error) {
	body, _ := json.Marshal(map[string]string{"client_id": codexClientID})
	var resp struct {
		DeviceAuthID string          `json:"device_auth_id"`
		UserCode     string          `json:"user_code"`
		UserCodeAlt  string          `json:"usercode"`
		Interval     json.RawMessage `json:"interval"`
	}
	if err := b.callJSON(ctx, b.http, "codex device code", http.MethodPost, b.ep.CodexDeviceUserCodeURL, jsonHeader(), body, &resp); err != nil {
		return deviceStart{}, err
	}
	userCode := labelOr(resp.UserCode, strings.TrimSpace(resp.UserCodeAlt))
	if userCode == "" || strings.TrimSpace(resp.DeviceAuthID) == "" {
		return deviceStart{}, errors.New("codex device code: incomplete provider response")
	}
	return deviceStart{
		secret: flowSecret{
			DeviceCode: strings.TrimSpace(resp.DeviceAuthID), UserCode: userCode,
			IntervalSeconds: pollInterval(resp.Interval, codexDeviceDefaultPollInterval),
		},
		userCode:        userCode,
		verificationURI: b.ep.CodexDeviceVerificationURL,
		lifetime:        codexDeviceFlowLifetimeMinutes * time.Minute,
	}, nil
}

func codexPoll(ctx context.Context, b *Broker, sec *flowSecret) (*token, error) {
	body, _ := json.Marshal(map[string]string{"device_auth_id": sec.DeviceCode, "user_code": sec.UserCode})
	reqCtx, cancel := context.WithTimeout(ctx, b.timeout)
	status, data, err := do(reqCtx, b.http, http.MethodPost, b.ep.CodexDeviceTokenURL, jsonHeader(), body)
	cancel()
	if err != nil {
		// A transport failure keeps the flow pending; the next poll retries.
		b.log.WarnContext(ctx, "codex device poll failed", "error", err)
		return nil, nil
	}
	switch {
	case status == http.StatusForbidden || status == http.StatusNotFound:
		return nil, nil
	case status < 200 || status >= 300:
		return nil, &statusError{op: "codex device poll", status: status}
	}
	var grant struct {
		AuthorizationCode string `json:"authorization_code"`
		CodeVerifier      string `json:"code_verifier"`
	}
	if json.Unmarshal(data, &grant) != nil || strings.TrimSpace(grant.AuthorizationCode) == "" || strings.TrimSpace(grant.CodeVerifier) == "" {
		return nil, failed("the provider returned an incomplete device authorization")
	}
	form := url.Values{
		"grant_type": {"authorization_code"}, "client_id": {codexClientID}, "code": {strings.TrimSpace(grant.AuthorizationCode)},
		"redirect_uri": {codexDeviceTokenExchangeURI}, "code_verifier": {strings.TrimSpace(grant.CodeVerifier)},
	}
	var resp tokenResponse
	if err := b.callJSON(ctx, b.http, "codex token exchange", http.MethodPost, b.ep.CodexTokenURL, formHeader(), []byte(form.Encode()), &resp); err != nil {
		return nil, err
	}
	if err := resp.check(); err != nil {
		return nil, err
	}
	email, accountID := codexClaims(resp.IDToken)
	now := b.now()
	payload := map[string]any{
		"type":          "codex",
		"id_token":      resp.IDToken,
		"access_token":  resp.AccessToken,
		"refresh_token": resp.RefreshToken,
		"account_id":    accountID,
		"last_refresh":  now.Format(time.RFC3339),
		"email":         email,
		"expired":       expiry(now, resp.ExpiresIn),
	}
	return &token{payload: payload, label: labelOr(email, "Codex account")}, nil
}

// codexClaims reads the email and ChatGPT account ID from the unverified ID
// token claims, as the SDK does. The token came directly from the
// provider's token endpoint over TLS.
func codexClaims(idToken string) (string, string) {
	parts := strings.Split(idToken, ".")
	if len(parts) < 2 {
		return "", ""
	}
	raw, err := base64.RawURLEncoding.DecodeString(parts[1])
	if err != nil {
		return "", ""
	}
	claims := gjson.ParseBytes(raw)
	return strings.TrimSpace(claims.Get("email").String()),
		strings.TrimSpace(claims.Get(gjson.Escape(codexIDTokenAuthClaimsNamespace) + ".chatgpt_account_id").String())
}

// --- Kimi device login ---

func kimiHeader(deviceID string) http.Header {
	h := formHeader()
	h.Set("X-Msh-Platform", kimiPlatform)
	h.Set("X-Msh-Version", kimiVersion)
	h.Set("X-Msh-Device-Name", kimiDeviceName)
	h.Set("X-Msh-Device-Model", kimiDeviceModel)
	h.Set("X-Msh-Device-Id", deviceID)
	return h
}

func kimiStartDevice(ctx context.Context, b *Broker) (deviceStart, error) {
	deviceID := uuid.NewString()
	form := url.Values{"client_id": {kimiClientID}}
	var resp struct {
		DeviceCode              string `json:"device_code"`
		UserCode                string `json:"user_code"`
		VerificationURI         string `json:"verification_uri"`
		VerificationURIComplete string `json:"verification_uri_complete"`
		ExpiresIn               int    `json:"expires_in"`
		Interval                int    `json:"interval"`
	}
	if err := b.callJSON(ctx, b.http, "kimi device code", http.MethodPost, b.ep.KimiDeviceAuthorizationURL, kimiHeader(deviceID), []byte(form.Encode()), &resp); err != nil {
		return deviceStart{}, err
	}
	if strings.TrimSpace(resp.DeviceCode) == "" {
		return deviceStart{}, errors.New("kimi device code: incomplete provider response")
	}
	lifetime := kimiMaxFlowMinutes * time.Minute
	if resp.ExpiresIn > 0 {
		lifetime = min(lifetime, time.Duration(resp.ExpiresIn)*time.Second)
	}
	return deviceStart{
		secret: flowSecret{
			DeviceCode: resp.DeviceCode, DeviceID: deviceID,
			IntervalSeconds: max(resp.Interval, kimiDefaultPollInterval),
		},
		userCode:        resp.UserCode,
		verificationURI: labelOr(resp.VerificationURIComplete, resp.VerificationURI),
		lifetime:        lifetime,
	}, nil
}

func kimiPoll(ctx context.Context, b *Broker, sec *flowSecret) (*token, error) {
	form := url.Values{"client_id": {kimiClientID}, "device_code": {sec.DeviceCode}, "grant_type": {kimiDeviceGrantType}}
	reqCtx, cancel := context.WithTimeout(ctx, b.timeout)
	_, data, err := do(reqCtx, b.http, http.MethodPost, b.ep.KimiTokenURL, kimiHeader(sec.DeviceID), []byte(form.Encode()))
	cancel()
	if err != nil {
		b.log.WarnContext(ctx, "kimi device poll failed", "error", err)
		return nil, nil
	}
	var resp struct {
		Error        string  `json:"error"`
		AccessToken  string  `json:"access_token"`
		RefreshToken string  `json:"refresh_token"`
		TokenType    string  `json:"token_type"`
		ExpiresIn    float64 `json:"expires_in"`
		Scope        string  `json:"scope"`
	}
	if err := json.Unmarshal(data, &resp); err != nil {
		return nil, errors.New("kimi device poll: invalid provider response")
	}
	switch resp.Error {
	case "":
	case "authorization_pending":
		return nil, nil
	case "slow_down":
		sec.IntervalSeconds += 5
		return nil, nil
	case "expired_token":
		return nil, expired()
	case "access_denied":
		return nil, failed("the authorization was denied")
	default:
		code := "error"
		if oauthErrorCode.MatchString(resp.Error) {
			code = resp.Error
		}
		return nil, failed("the provider reported an authorization error: " + code)
	}
	if strings.TrimSpace(resp.AccessToken) == "" {
		return nil, failed("the provider returned no access token")
	}
	now := b.now()
	payload := map[string]any{
		"type":          "kimi",
		"access_token":  resp.AccessToken,
		"refresh_token": resp.RefreshToken,
		"token_type":    resp.TokenType,
		"scope":         resp.Scope,
		"timestamp":     now.UnixMilli(),
		"device_id":     sec.DeviceID,
	}
	if resp.ExpiresIn > 0 {
		payload["expired"] = now.Add(time.Duration(resp.ExpiresIn) * time.Second).UTC().Format(time.RFC3339)
	}
	return &token{payload: payload, label: "Kimi account"}, nil
}
