// SPDX-License-Identifier: Apache-2.0
package syncclient

import (
	"context"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"sync"
	"testing"
	"time"
)

type memoryCredentials struct {
	mu     sync.Mutex
	values map[string]string
	fail   bool
}

func (m *memoryCredentials) Get(key string) (string, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.fail {
		return "", ErrCredentials
	}
	v, ok := m.values[key]
	if !ok {
		return "", ErrCredentials
	}
	return v, nil
}
func (m *memoryCredentials) Set(key, value string) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.fail {
		return ErrCredentials
	}
	if m.values == nil {
		m.values = map[string]string{}
	}
	m.values[key] = value
	return nil
}
func (m *memoryCredentials) Delete(key string) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.fail {
		return ErrCredentials
	}
	delete(m.values, key)
	return nil
}
func testCredential(endpoint string) Credential {
	return Credential{AccessToken: "secret-access", RefreshToken: "secret-refresh", TokenType: "Bearer", ExpiresIn: 3600, Issuer: "https://issuer.example", AccountID: "account", TenantID: "tenant", DeviceID: "device", SessionID: "session", Endpoint: endpoint, ExpiresAt: time.Now().Add(time.Hour)}
}
func TestBrowserApprovalPersistsAcrossProcesses(t *testing.T) {
	vault := &memoryCredentials{}
	var challenge string
	calls := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls++
		if r.URL.Path != "/oauth/token" || r.ParseForm() != nil {
			t.Error("unexpected request")
		}
		digest := sha256.Sum256([]byte(r.Form.Get("code_verifier")))
		if base64.RawURLEncoding.EncodeToString(digest[:]) != challenge {
			t.Error("PKCE mismatch")
		}
		if r.Form.Get("redirect_uri") != "lane-dev://auth/callback" || r.Form.Get("client_id") != "lane-cli" {
			t.Error("binding mismatch")
		}
		json.NewEncoder(w).Encode(testCredential(""))
	}))
	defer server.Close()
	first := Auth{CredentialLockRoot: t.TempDir(), Credentials: vault}
	approval, e := first.Begin(context.Background(), "space", "device", server.URL, "lane-dev://auth/callback")
	if e != nil {
		t.Fatal(e)
	}
	authorize, e := url.Parse(approval.AuthorizationURL)
	if e != nil {
		t.Fatal(e)
	}
	challenge = authorize.Query().Get("code_challenge")
	if authorize.Query().Get("device_id") != "device" || authorize.Query().Get("state") != approval.ChallengeID {
		t.Fatal("missing device/state binding")
	}
	second := Auth{CredentialLockRoot: t.TempDir(), Credentials: vault}
	callback := "lane-dev://auth/callback?code=code&state=" + approval.ChallengeID
	if _, e = second.Complete(context.Background(), "other-space", callback); !errors.Is(e, ErrApproval) {
		t.Fatal("cross-space callback accepted")
	}
	if _, e = second.Complete(context.Background(), "space", strings.Replace(callback, "lane-dev:", "lane:", 1)); !errors.Is(e, ErrApproval) {
		t.Fatal("redirect mismatch accepted")
	}
	credential, e := second.Complete(context.Background(), "space", callback)
	if e != nil {
		t.Fatal(e)
	}
	if credential.Endpoint != server.URL || credential.AccountID != "account" {
		t.Fatal("identity not retained")
	}
	if _, e = second.Complete(context.Background(), "space", callback); !errors.Is(e, ErrApproval) {
		t.Fatal("callback replay accepted")
	}
	if calls != 1 {
		t.Fatalf("unexpected exchanges %d", calls)
	}
}
func TestApprovalFailureAndDeviceBinding(t *testing.T) {
	for _, failure := range []string{"network", "device"} {
		t.Run(failure, func(t *testing.T) {
			vault := &memoryCredentials{}
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				if failure == "network" {
					w.WriteHeader(503)
					return
				}
				c := testCredential("")
				c.DeviceID = "other"
				json.NewEncoder(w).Encode(c)
			}))
			defer server.Close()
			a := Auth{CredentialLockRoot: t.TempDir(), Credentials: vault}
			p, e := a.Begin(context.Background(), "space", "device", server.URL, "lane://auth/callback")
			if e != nil {
				t.Fatal(e)
			}
			_, e = a.Complete(context.Background(), "space", "lane://auth/callback?code=x&state="+p.ChallengeID)
			if e == nil {
				t.Fatal("accepted failed exchange")
			}
			if _, e = vault.Get(pendingKey("space", p.ChallengeID)); e != nil {
				t.Fatal("pending verifier lost on failure")
			}
		})
	}
}
func TestCredentialsAccountIsolationAndRefresh(t *testing.T) {
	vault := &memoryCredentials{}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.ParseForm() != nil || r.Form.Get("grant_type") != "refresh_token" || r.Form.Get("refresh_token") != "secret-refresh" {
			t.Error("refresh binding")
		}
		c := testCredential("")
		c.AccessToken = "fresh"
		c.RefreshToken = "rotated"
		json.NewEncoder(w).Encode(c)
	}))
	defer server.Close()
	a := Auth{CredentialLockRoot: t.TempDir(), Credentials: vault}
	c := testCredential(server.URL)
	c.ExpiresAt = time.Now().Add(-time.Hour)
	if e := a.Save(context.Background(), "space", "device", c); e != nil {
		t.Fatal(e)
	}
	if _, e := a.Token(context.Background(), "other-space", "device", c.Issuer, c.AccountID, c.TenantID); e == nil {
		t.Fatal("cross-space token")
	}
	if _, e := a.Token(context.Background(), "space", "device", c.Issuer, "other", c.TenantID); e == nil {
		t.Fatal("cross-account token")
	}
	token, e := a.Token(context.Background(), "space", "device", c.Issuer, c.AccountID, c.TenantID)
	if e != nil || token != "fresh" {
		t.Fatalf("refresh: %q %v", token, e)
	}
	saved, e := a.Load(context.Background(), "space", "device", c.Issuer, c.AccountID, c.TenantID)
	if e != nil || saved.RefreshToken != "rotated" {
		t.Fatal("rotated credential not durable")
	}
}
func TestRefreshFailureRetainsCredentialAndNoRedirect(t *testing.T) {
	calls := 0
	evil := httptest.NewServer(http.HandlerFunc(func(http.ResponseWriter, *http.Request) { calls++ }))
	defer evil.Close()
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { http.Redirect(w, r, evil.URL, 307) }))
	defer server.Close()
	vault := &memoryCredentials{}
	a := Auth{CredentialLockRoot: t.TempDir(), Credentials: vault}
	c := testCredential(server.URL)
	c.ExpiresAt = time.Now().Add(-time.Hour)
	if e := a.Save(context.Background(), "space", "device", c); e != nil {
		t.Fatal(e)
	}
	if _, e := a.Token(context.Background(), "space", "device", c.Issuer, c.AccountID, c.TenantID); e == nil {
		t.Fatal("redirect accepted")
	}
	if calls != 0 {
		t.Fatal("token leaked across redirect")
	}
	saved, e := a.Load(context.Background(), "space", "device", c.Issuer, c.AccountID, c.TenantID)
	if e != nil || saved.RefreshToken != c.RefreshToken {
		t.Fatal("failure destroyed credential")
	}
}
func TestUnavailableVaultAndInsecureEndpointFail(t *testing.T) {
	a := Auth{CredentialLockRoot: t.TempDir(), Credentials: &memoryCredentials{fail: true}}
	if _, e := a.Begin(context.Background(), "space", "device", "https://lane.example", "lane://auth/callback"); !errors.Is(e, ErrCredentials) {
		t.Fatal("vault fallback")
	}
	a.Credentials = &memoryCredentials{}
	for _, endpoint := range []string{"http://lane.example", "https://user:password@lane.example", "https://lane.example/?secret=x"} {
		if _, e := a.Begin(context.Background(), "space", "device", endpoint, "lane://auth/callback"); e == nil {
			t.Fatal("unsafe endpoint accepted")
		}
	}
}
func TestReauthenticationProofRequiresMatchingChallenge(t *testing.T) {
	calls := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls++
		if r.Header.Get("Authorization") != "Bearer secret-access" || r.URL.Path != "/auth/reauth/proof" {
			t.Error("proof credential binding")
		}
		io := map[string]string{}
		json.NewDecoder(r.Body).Decode(&io)
		if io["challenge_id"] != "challenge" {
			t.Error("wrong challenge")
		}
		w.Write([]byte(`{"reauth_proof":"one-use"}`))
	}))
	defer server.Close()
	a := Auth{CredentialLockRoot: t.TempDir()}
	c := testCredential(server.URL)
	if _, e := a.ReauthenticationProof(context.Background(), c, "challenge", "lane://auth/callback?reauth_challenge=other"); e == nil {
		t.Fatal("wrong challenge accepted")
	}
	proof, e := a.ReauthenticationProof(context.Background(), c, "challenge", "lane://auth/callback?reauth_challenge=challenge")
	if e != nil || proof != "one-use" || calls != 1 {
		t.Fatalf("proof: %q %v %d", proof, e, calls)
	}
}
