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
	"strconv"
	"sync"
	"testing"
	"time"

	"github.com/go-jose/go-jose/v4"
	"github.com/go-jose/go-jose/v4/jwt"
)

// Provider is a fake Logto issuer with discovery, JWKS and a token endpoint.
type Provider struct {
	Server *httptest.Server

	mu sync.Mutex
	// published holds the keys served by the JWKS endpoint; the last
	// signing key is the one Sign uses.
	published []signingKey
	// discoveryRequests counts discovery document requests.
	discoveryRequests int
	// discoveryHang, when non-nil, holds discovery requests until it is
	// closed.
	discoveryHang chan struct{}
	// jwksRequests counts JWKS endpoint requests.
	jwksRequests int
	// jwksStatus, when set, fails JWKS requests with this status.
	jwksStatus int
	// jwksHang, when non-nil, holds JWKS requests until it is closed.
	jwksHang chan struct{}
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

type signingKey struct {
	kid string
	key *ecdsa.PrivateKey
}

// New starts a provider and stops it at test cleanup.
func New(t testing.TB) *Provider {
	t.Helper()
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	p := &Provider{published: []signingKey{{kid: "k1", key: key}}}
	mux := http.NewServeMux()
	mux.HandleFunc("GET /.well-known/openid-configuration", func(w http.ResponseWriter, r *http.Request) {
		p.mu.Lock()
		p.discoveryRequests++
		hang := p.discoveryHang
		p.mu.Unlock()
		if hang != nil {
			select {
			case <-hang:
			case <-r.Context().Done():
				return
			}
		}
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
		p.mu.Lock()
		p.jwksRequests++
		status := p.jwksStatus
		hang := p.jwksHang
		set := jose.JSONWebKeySet{}
		for _, k := range p.published {
			set.Keys = append(set.Keys, jose.JSONWebKey{Key: &k.key.PublicKey, KeyID: k.kid, Algorithm: "ES256", Use: "sig"})
		}
		p.mu.Unlock()
		if hang != nil {
			select {
			case <-hang:
			case <-r.Context().Done():
				return
			}
		}
		if status != 0 {
			writeJSON(w, status, map[string]string{"error": "temporarily_unavailable"})
			return
		}
		writeJSON(w, http.StatusOK, set)
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
	p.mu.Lock()
	active := p.published[len(p.published)-1]
	p.mu.Unlock()
	return p.signWith(t, active.key, active.kid, claims)
}

// SignForged signs claims with a key the JWKS endpoint never publishes but
// names kid in the token header. With the ID of a published key it models a
// bad signature; with an unknown ID it models an unknown key.
func (p *Provider) SignForged(t testing.TB, kid string, claims map[string]any) string {
	t.Helper()
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	return p.signWith(t, key, kid, claims)
}

// RotateKey publishes a new signing key, retires every earlier key from the
// JWKS endpoint and makes Sign use the new key. It returns the new key ID.
func (p *Provider) RotateKey(t testing.TB) string {
	t.Helper()
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	p.mu.Lock()
	defer p.mu.Unlock()
	kid := "k" + strconv.Itoa(len(p.published)+1)
	p.published = []signingKey{{kid: kid, key: key}}
	return kid
}

// JWKSRequests returns how many requests the JWKS endpoint has received.
func (p *Provider) JWKSRequests() int {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.jwksRequests
}

// DiscoveryRequests returns how many requests the discovery endpoint has
// received.
func (p *Provider) DiscoveryRequests() int {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.discoveryRequests
}

// SetDiscoveryHang makes discovery requests wait until it is called again
// with false, which releases them. Tests that enable it must release before
// cleanup.
func (p *Provider) SetDiscoveryHang(hang bool) {
	p.mu.Lock()
	defer p.mu.Unlock()
	switch {
	case hang && p.discoveryHang == nil:
		p.discoveryHang = make(chan struct{})
	case !hang && p.discoveryHang != nil:
		close(p.discoveryHang)
		p.discoveryHang = nil
	}
}

// SetJWKSStatus makes the JWKS endpoint fail with status; zero restores
// success.
func (p *Provider) SetJWKSStatus(status int) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.jwksStatus = status
}

// SetJWKSHang makes JWKS requests wait until it is called again with false,
// which releases them. Tests that enable it must release before cleanup.
func (p *Provider) SetJWKSHang(hang bool) {
	p.mu.Lock()
	defer p.mu.Unlock()
	switch {
	case hang && p.jwksHang == nil:
		p.jwksHang = make(chan struct{})
	case !hang && p.jwksHang != nil:
		close(p.jwksHang)
		p.jwksHang = nil
	}
}

func (p *Provider) signWith(t testing.TB, key *ecdsa.PrivateKey, kid string, claims map[string]any) string {
	t.Helper()
	signer, err := jose.NewSigner(jose.SigningKey{Algorithm: jose.ES256, Key: key}, (&jose.SignerOptions{}).WithType("JWT").WithHeader("kid", kid))
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
