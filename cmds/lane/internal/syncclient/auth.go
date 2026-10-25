// SPDX-License-Identifier: Apache-2.0
package syncclient

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/url"
	"strings"
	"time"

	"github.com/zalando/go-keyring"
)

var ErrCredentials = errors.New("OS credentials unavailable; approve the connection again")
var ErrApproval = errors.New("invalid or expired browser approval callback")

// CredentialStore never falls back to files or environment variables.
type CredentialStore interface {
	Get(string) (string, error)
	Set(string, string) error
	Delete(string) error
}
type OSCredentials struct{}

func (OSCredentials) Get(key string) (string, error) {
	v, e := keyring.Get("io.delino.lane", key)
	if e != nil {
		return "", ErrCredentials
	}
	return v, nil
}
func (OSCredentials) Set(key, value string) error {
	if e := keyring.Set("io.delino.lane", key, value); e != nil {
		return ErrCredentials
	}
	return nil
}
func (OSCredentials) Delete(key string) error {
	e := keyring.Delete("io.delino.lane", key)
	if e != nil && !errors.Is(e, keyring.ErrNotFound) {
		return ErrCredentials
	}
	return nil
}

type Credential struct {
	AccessToken  string    `json:"access_token"`
	RefreshToken string    `json:"refresh_token"`
	TokenType    string    `json:"token_type"`
	ExpiresIn    int64     `json:"expires_in"`
	Issuer       string    `json:"issuer"`
	AccountID    string    `json:"account_id"`
	TenantID     string    `json:"tenant_id"`
	DeviceID     string    `json:"device_id"`
	SessionID    string    `json:"session_id"`
	Endpoint     string    `json:"endpoint"`
	ExpiresAt    time.Time `json:"expires_at"`
}

type Approval struct {
	AuthorizationURL string `json:"authorizationUrl"`
	ChallengeID      string `json:"challengeId"`
}
type pendingApproval struct {
	State, Verifier, RedirectURI, Endpoint, DeviceID string
	ExpiresAt                                        time.Time
}
type Auth struct {
	Credentials CredentialStore
	HTTPClient  *http.Client
	ClientID    string
}

func (a Auth) vault() CredentialStore {
	if a.Credentials != nil {
		return a.Credentials
	}
	return OSCredentials{}
}
func (a Auth) clientID() string {
	if a.ClientID != "" {
		return a.ClientID
	}
	return "lane-cli"
}
func (a Auth) httpClient() *http.Client {
	client := &http.Client{Timeout: 20 * time.Second}
	if a.HTTPClient != nil {
		*client = *a.HTTPClient
	}
	// Token endpoints never follow a redirect carrying a code, verifier or bearer.
	client.CheckRedirect = func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }
	return client
}
func credentialKey(space, device, issuer, account, tenant string) string {
	sum := sha256.Sum256([]byte(strings.Join([]string{space, device, issuer, account, tenant}, "\x00")))
	return "credential:" + hex.EncodeToString(sum[:])
}
func pendingKey(space, state string) string { return "approval:" + space + ":" + state }
func randomURLToken() (string, error) {
	b := make([]byte, 32)
	if _, e := rand.Read(b); e != nil {
		return "", e
	}
	return base64.RawURLEncoding.EncodeToString(b), nil
}
func endpointURL(raw string) (string, error) {
	u, e := url.Parse(raw)
	if e != nil || u.Host == "" || u.User != nil || u.RawQuery != "" || u.Fragment != "" || (u.Path != "" && u.Path != "/") {
		return "", errors.New("invalid API endpoint")
	}
	if u.Scheme != "https" && !(u.Scheme == "http" && (u.Hostname() == "127.0.0.1" || u.Hostname() == "localhost" || u.Hostname() == "::1")) {
		return "", errors.New("API endpoint requires HTTPS")
	}
	return strings.TrimRight(u.String(), "/"), nil
}
func redirectURL(raw string) error {
	u, e := url.Parse(raw)
	if e != nil || u.User != nil || u.RawQuery != "" || u.Fragment != "" {
		return ErrApproval
	}
	if (u.Scheme == "lane" || u.Scheme == "lane-dev") && u.Host == "auth" && u.Path == "/callback" {
		return nil
	}
	if u.Scheme == "http" && (u.Hostname() == "127.0.0.1" || u.Hostname() == "::1") && u.Port() != "" && u.Path == "/auth/callback" {
		return nil
	}
	return ErrApproval
}

// Begin persists PKCE/state in OS credentials before returning the browser URL.
// The caller opens this URL; a later process calls Complete with the callback URI.
func (a Auth) Begin(ctx context.Context, spaceID, deviceID, endpoint, redirectURI string) (Approval, error) {
	if err := ctx.Err(); err != nil {
		return Approval{}, err
	}
	if spaceID == "" || deviceID == "" {
		return Approval{}, ErrApproval
	}
	endpoint, err := endpointURL(endpoint)
	if err != nil {
		return Approval{}, err
	}
	if err = redirectURL(redirectURI); err != nil {
		return Approval{}, err
	}
	state, err := randomURLToken()
	if err != nil {
		return Approval{}, err
	}
	verifier, err := randomURLToken()
	if err != nil {
		return Approval{}, err
	}
	pending := pendingApproval{State: state, Verifier: verifier, RedirectURI: redirectURI, Endpoint: endpoint, DeviceID: deviceID, ExpiresAt: time.Now().Add(10 * time.Minute)}
	raw, err := json.Marshal(pending)
	if err != nil {
		return Approval{}, err
	}
	if err = a.vault().Set(pendingKey(spaceID, state), string(raw)); err != nil {
		return Approval{}, err
	}
	digest := sha256.Sum256([]byte(verifier))
	q := url.Values{"response_type": {"code"}, "client_id": {a.clientID()}, "redirect_uri": {redirectURI}, "state": {state}, "code_challenge": {base64.RawURLEncoding.EncodeToString(digest[:])}, "code_challenge_method": {"S256"}, "device_id": {deviceID}, "device_name": {"Lane CLI"}}
	return Approval{AuthorizationURL: endpoint + "/oauth/authorize?" + q.Encode(), ChallengeID: state}, nil
}
func (a Auth) Complete(ctx context.Context, spaceID, callbackURI string) (Credential, error) {
	u, err := url.Parse(callbackURI)
	if err != nil {
		return Credential{}, ErrApproval
	}
	q, err := url.ParseQuery(u.RawQuery)
	if err != nil {
		return Credential{}, ErrApproval
	}
	if len(q["state"]) != 1 || len(q["code"]) != 1 || q.Get("error") != "" || u.Fragment != "" {
		return Credential{}, ErrApproval
	}
	state := q.Get("state")
	if state == "" || q.Get("code") == "" {
		return Credential{}, ErrApproval
	}
	raw, err := a.vault().Get(pendingKey(spaceID, state))
	if err != nil {
		return Credential{}, ErrApproval
	}
	var pending pendingApproval
	if json.Unmarshal([]byte(raw), &pending) != nil || subtle.ConstantTimeCompare([]byte(state), []byte(pending.State)) != 1 || !time.Now().Before(pending.ExpiresAt) {
		return Credential{}, ErrApproval
	}
	u.RawQuery = ""
	if u.String() != pending.RedirectURI {
		return Credential{}, ErrApproval
	}
	credential, err := a.exchange(ctx, pending.Endpoint, url.Values{"grant_type": {"authorization_code"}, "code": {q.Get("code")}, "client_id": {a.clientID()}, "redirect_uri": {pending.RedirectURI}, "code_verifier": {pending.Verifier}})
	if err != nil {
		return Credential{}, err
	}
	if credential.DeviceID != pending.DeviceID {
		return Credential{}, ErrAccountMismatch
	}
	if err = a.vault().Delete(pendingKey(spaceID, state)); err != nil {
		return Credential{}, err
	}
	return credential, nil
}
func (a Auth) exchange(ctx context.Context, endpoint string, form url.Values) (Credential, error) {
	var c Credential
	if err := a.post(ctx, endpoint+"/oauth/token", "application/x-www-form-urlencoded", form.Encode(), "", &c); err != nil {
		return c, err
	}
	if c.AccessToken == "" || c.TokenType != "Bearer" || c.ExpiresIn <= 0 || c.Issuer == "" || c.AccountID == "" || c.TenantID == "" || c.DeviceID == "" || c.SessionID == "" {
		return Credential{}, ErrCredentials
	}
	c.Endpoint = endpoint
	c.ExpiresAt = time.Now().Add(time.Duration(c.ExpiresIn) * time.Second)
	return c, nil
}
func (a Auth) post(ctx context.Context, target, contentType, body, token string, out any) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, target, strings.NewReader(body))
	if err != nil {
		return ErrCredentials
	}
	req.Header.Set("Content-Type", contentType)
	req.Header.Set("Accept", "application/json")
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	response, err := a.httpClient().Do(req)
	if err != nil {
		return errors.New("authentication service unavailable")
	}
	defer response.Body.Close()
	if response.StatusCode < 200 || response.StatusCode >= 300 {
		return ErrCredentials
	}
	if out == nil {
		return nil
	}
	raw, err := io.ReadAll(io.LimitReader(response.Body, 1<<20))
	if err != nil || json.Unmarshal(raw, out) != nil {
		return ErrCredentials
	}
	return nil
}
func (a Auth) Save(ctx context.Context, spaceID, deviceID string, c Credential) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	if spaceID == "" || deviceID == "" || c.DeviceID != deviceID || c.AccountID == "" || c.Issuer == "" || c.TenantID == "" || c.AccessToken == "" {
		return ErrAccountMismatch
	}
	if _, err := endpointURL(c.Endpoint); err != nil {
		return err
	}
	raw, err := json.Marshal(c)
	if err != nil {
		return err
	}
	return a.vault().Set(credentialKey(spaceID, deviceID, c.Issuer, c.AccountID, c.TenantID), string(raw))
}
func (a Auth) Load(ctx context.Context, spaceID, deviceID, issuer, accountID, tenantID string) (Credential, error) {
	if err := ctx.Err(); err != nil {
		return Credential{}, err
	}
	raw, err := a.vault().Get(credentialKey(spaceID, deviceID, issuer, accountID, tenantID))
	if err != nil {
		return Credential{}, err
	}
	var c Credential
	if json.Unmarshal([]byte(raw), &c) != nil {
		return c, ErrCredentials
	}
	if c.Issuer != issuer || c.AccountID != accountID || c.TenantID != tenantID || c.DeviceID != deviceID {
		return Credential{}, ErrAccountMismatch
	}
	return c, nil
}
func (a Auth) Token(ctx context.Context, spaceID, deviceID, issuer, accountID, tenantID string) (string, error) {
	c, err := a.Load(ctx, spaceID, deviceID, issuer, accountID, tenantID)
	if err != nil {
		return "", err
	}
	if time.Now().Add(30 * time.Second).Before(c.ExpiresAt) {
		return c.AccessToken, nil
	}
	if c.RefreshToken == "" {
		return "", ErrCredentials
	}
	fresh, err := a.exchange(ctx, c.Endpoint, url.Values{"grant_type": {"refresh_token"}, "refresh_token": {c.RefreshToken}, "client_id": {a.clientID()}})
	if err != nil {
		return "", err
	}
	if fresh.Issuer != issuer || fresh.AccountID != accountID || fresh.TenantID != tenantID || fresh.DeviceID != deviceID || fresh.SessionID != c.SessionID {
		return "", ErrAccountMismatch
	}
	if fresh.RefreshToken == "" {
		fresh.RefreshToken = c.RefreshToken
	}
	if err = a.Save(ctx, spaceID, deviceID, fresh); err != nil {
		return "", err
	}
	return fresh.AccessToken, nil
}
func (a Auth) Delete(ctx context.Context, spaceID, deviceID, issuer, accountID, tenantID string) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	return a.vault().Delete(credentialKey(spaceID, deviceID, issuer, accountID, tenantID))
}
func (a Auth) Revoke(ctx context.Context, c Credential) error {
	endpoint, err := endpointURL(c.Endpoint)
	if err != nil {
		return err
	}
	token := c.RefreshToken
	if token == "" {
		token = c.AccessToken
	}
	return a.post(ctx, endpoint+"/oauth/revoke", "application/x-www-form-urlencoded", url.Values{"token": {token}, "client_id": {a.clientID()}}.Encode(), "", nil)
}

// ReauthenticationProof redeems the browser challenge under the existing grant.
// The caller must match this challenge to its BeginDataDeletion response.
func (a Auth) ReauthenticationProof(ctx context.Context, c Credential, challengeID, callbackURI string) (string, error) {
	u, err := url.Parse(callbackURI)
	if err != nil {
		return "", ErrApproval
	}
	q, err := url.ParseQuery(u.RawQuery)
	if err != nil || len(q["reauth_challenge"]) != 1 || q.Get("reauth_challenge") != challengeID || challengeID == "" || u.Fragment != "" {
		return "", ErrApproval
	}
	u.RawQuery = ""
	if redirectURL(u.String()) != nil {
		return "", ErrApproval
	}
	endpoint, err := endpointURL(c.Endpoint)
	if err != nil {
		return "", err
	}
	raw, _ := json.Marshal(map[string]string{"challenge_id": challengeID})
	var response struct {
		Proof string `json:"reauth_proof"`
	}
	if err = a.post(ctx, endpoint+"/auth/reauth/proof", "application/json", string(raw), c.AccessToken, &response); err != nil {
		return "", err
	}
	if response.Proof == "" {
		return "", ErrApproval
	}
	return response.Proof, nil
}
