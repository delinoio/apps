// SPDX-License-Identifier: FSL-1.1-ALv2

// Package authtest provides a fake Logto OIDC provider for tests.
package authtest

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/url"
	"sync"
	"testing"
	"time"

	"github.com/go-jose/go-jose/v4"
	"github.com/go-jose/go-jose/v4/jwt"
)

// Provider is a fake Logto issuer with discovery, JWKS and a token endpoint.
type Provider struct {
	Server *httptest.Server
	key    *ecdsa.PrivateKey

	mu sync.Mutex
	// TokenHandler answers token endpoint requests when set.
	TokenHandler func(form url.Values) (int, any)
	// TokenRequests records token endpoint forms.
	TokenRequests []url.Values
	// RevokedTokens records tokens sent to the revocation endpoint.
	RevokedTokens []string
	// revokeStatus, when set, fails revocation requests with this status
	// without recording the token.
	revokeStatus int
}

// New starts a provider and stops it at test cleanup.
func New(t testing.TB) *Provider {
	t.Helper()
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	p := &Provider{key: key}
	mux := http.NewServeMux()
	mux.HandleFunc("GET /.well-known/openid-configuration", func(w http.ResponseWriter, r *http.Request) {
		base := p.Server.URL
		writeJSON(w, http.StatusOK, map[string]any{
			"issuer":                                base,
			"authorization_endpoint":                base + "/auth",
			"token_endpoint":                        base + "/token",
			"end_session_endpoint":                  base + "/session/end",
			"revocation_endpoint":                   base + "/token/revocation",
			"jwks_uri":                              base + "/jwks",
			"id_token_signing_alg_values_supported": []string{"ES256"},
		})
	})
	mux.HandleFunc("GET /jwks", func(w http.ResponseWriter, r *http.Request) {
		writeJSON(w, http.StatusOK, jose.JSONWebKeySet{Keys: []jose.JSONWebKey{{Key: &p.key.PublicKey, KeyID: "k1", Algorithm: "ES256", Use: "sig"}}})
	})
	mux.HandleFunc("POST /token", func(w http.ResponseWriter, r *http.Request) {
		if err := r.ParseForm(); err != nil {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid_request"})
			return
		}
		p.mu.Lock()
		p.TokenRequests = append(p.TokenRequests, r.PostForm)
		h := p.TokenHandler
		p.mu.Unlock()
		if h == nil {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid_grant"})
			return
		}
		status, body := h(r.PostForm)
		writeJSON(w, status, body)
	})
	mux.HandleFunc("POST /token/revocation", func(w http.ResponseWriter, r *http.Request) {
		if err := r.ParseForm(); err != nil {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid_request"})
			return
		}
		p.mu.Lock()
		status := p.revokeStatus
		if status == 0 {
			p.RevokedTokens = append(p.RevokedTokens, r.PostForm.Get("token"))
		}
		p.mu.Unlock()
		if status != 0 {
			writeJSON(w, status, map[string]string{"error": "temporarily_unavailable"})
			return
		}
		w.WriteHeader(http.StatusOK)
	})
	p.Server = httptest.NewServer(mux)
	t.Cleanup(p.Server.Close)
	return p
}

// Issuer returns the issuer URL.
func (p *Provider) Issuer() string { return p.Server.URL }

// Sign signs claims. Standard claims default to this issuer, one hour of
// validity and the current time.
func (p *Provider) Sign(t testing.TB, claims map[string]any) string {
	t.Helper()
	signer, err := jose.NewSigner(jose.SigningKey{Algorithm: jose.ES256, Key: p.key}, (&jose.SignerOptions{}).WithType("JWT").WithHeader("kid", "k1"))
	if err != nil {
		t.Fatal(err)
	}
	now := time.Now()
	full := map[string]any{"iss": p.Server.URL, "iat": now.Unix(), "exp": now.Add(time.Hour).Unix()}
	for k, v := range claims {
		full[k] = v
	}
	raw, err := jwt.Signed(signer).Claims(full).Serialize()
	if err != nil {
		t.Fatal(err)
	}
	return raw
}

// Revoked returns a copy of the revoked tokens.
func (p *Provider) Revoked() []string {
	p.mu.Lock()
	defer p.mu.Unlock()
	return append([]string(nil), p.RevokedTokens...)
}

// SetRevokeStatus makes the revocation endpoint fail with status; zero
// restores success.
func (p *Provider) SetRevokeStatus(status int) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.revokeStatus = status
}

// SetTokenHandler replaces the token endpoint behavior.
func (p *Provider) SetTokenHandler(h func(form url.Values) (int, any)) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.TokenHandler = h
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}
