// SPDX-License-Identifier: FSL-1.1-ALv2

package auth

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"sync"
	"time"

	"github.com/go-jose/go-jose/v4"
)

// jwksFetchTimeout bounds one JWKS download. The fetch runs detached from the
// verification that triggered it so that concurrent verifications share one
// request and a canceled caller does not poison the shared attempt.
const jwksFetchTimeout = 10 * time.Second

// jwksBodyLimit caps the JWKS document size read from Logto.
const jwksBodyLimit = 1 << 20

// jwksAlgorithms are the signature algorithms the key set parses. The
// verifier built from the discovery document restricts which of them a token
// may use.
var jwksAlgorithms = []jose.SignatureAlgorithm{
	jose.RS256, jose.RS384, jose.RS512,
	jose.ES256, jose.ES384, jose.ES512,
	jose.PS256, jose.PS384, jose.PS512,
	jose.EdDSA,
}

// errJWKSSignature means no cached or fetched key verified the signature.
// It is a token failure, never an outage.
var errJWKSSignature = errors.New("failed to verify token signature")

// jwksOutageKey carries a *jwksOutage through the context that go-oidc
// forwards from IDTokenVerifier.Verify to KeySet.VerifySignature.
type jwksOutageKey struct{}

// jwksOutage records that signing keys could not be fetched while verifying
// one token. go-oidc re-wraps signature errors with %v, which drops the
// error chain, so the key set reports an outage through the context instead.
// Remove this side channel if go-oidc starts preserving the wrapped error.
type jwksOutage struct{ err error }

func withJWKSOutage(ctx context.Context) (context.Context, *jwksOutage) {
	o := &jwksOutage{}
	return context.WithValue(ctx, jwksOutageKey{}, o), o
}

// jwksKeySet implements oidc.KeySet over a Logto JWKS endpoint. Unlike
// oidc.RemoteKeySet it refetches the key set only when the token names a key
// ID that is not cached (or names none), so a token whose key is known but
// whose signature is wrong never reaches Logto.
type jwksKeySet struct {
	url    string
	client *http.Client

	mu       sync.Mutex
	keys     []jose.JSONWebKey
	inflight *jwksFetch
}

type jwksFetch struct {
	done chan struct{}
	keys []jose.JSONWebKey
	err  error
}

func newJWKSKeySet(url string, client *http.Client) *jwksKeySet {
	return &jwksKeySet{url: url, client: client}
}

// VerifySignature implements oidc.KeySet.
func (s *jwksKeySet) VerifySignature(ctx context.Context, raw string) ([]byte, error) {
	jws, err := jose.ParseSigned(raw, jwksAlgorithms)
	if err != nil {
		return nil, fmt.Errorf("malformed jwt: %w", err)
	}
	if len(jws.Signatures) != 1 {
		return nil, errors.New("jwt must carry exactly one signature")
	}
	kid := jws.Signatures[0].Header.KeyID

	cached := s.snapshot()
	if payload, ok := verifyWithKeys(jws, cached, kid); ok {
		return payload, nil
	}
	if kid != "" && hasKeyID(cached, kid) {
		// The key is known, so the signature is wrong. Do not refetch.
		return nil, errJWKSSignature
	}

	fresh, err := s.refresh(ctx)
	if err != nil {
		if o, ok := ctx.Value(jwksOutageKey{}).(*jwksOutage); ok {
			o.err = err
		}
		return nil, fmt.Errorf("fetching keys: %w", err)
	}
	if payload, ok := verifyWithKeys(jws, fresh, kid); ok {
		return payload, nil
	}
	return nil, errJWKSSignature
}

func (s *jwksKeySet) snapshot() []jose.JSONWebKey {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.keys
}

func verifyWithKeys(jws *jose.JSONWebSignature, keys []jose.JSONWebKey, kid string) ([]byte, bool) {
	for i := range keys {
		if kid != "" && keys[i].KeyID != kid {
			continue
		}
		if payload, err := jws.Verify(&keys[i]); err == nil {
			return payload, true
		}
	}
	return nil, false
}

func hasKeyID(keys []jose.JSONWebKey, kid string) bool {
	for i := range keys {
		if keys[i].KeyID == kid {
			return true
		}
	}
	return false
}

// refresh downloads the key set once for all concurrent callers. The caller
// stops waiting when ctx ends; the download itself continues and still fills
// the cache.
func (s *jwksKeySet) refresh(ctx context.Context) ([]jose.JSONWebKey, error) {
	s.mu.Lock()
	f := s.inflight
	if f == nil {
		f = &jwksFetch{done: make(chan struct{})}
		s.inflight = f
		go s.run(f)
	}
	s.mu.Unlock()

	select {
	case <-ctx.Done():
		return nil, ctx.Err()
	case <-f.done:
		return f.keys, f.err
	}
}

func (s *jwksKeySet) run(f *jwksFetch) {
	ctx, cancel := context.WithTimeout(context.Background(), jwksFetchTimeout)
	defer cancel()
	keys, err := s.fetch(ctx)

	s.mu.Lock()
	if err == nil {
		s.keys = keys
	}
	s.inflight = nil
	s.mu.Unlock()

	f.keys, f.err = keys, err
	close(f.done)
}

func (s *jwksKeySet) fetch(ctx context.Context) ([]jose.JSONWebKey, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, s.url, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Accept", "application/json")
	resp, err := s.client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(io.LimitReader(resp.Body, jwksBodyLimit))
	if err != nil {
		return nil, err
	}
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("jwks endpoint status %d", resp.StatusCode)
	}
	var doc struct {
		Keys []json.RawMessage `json:"keys"`
	}
	if err := json.Unmarshal(body, &doc); err != nil {
		return nil, fmt.Errorf("decode jwks: %w", err)
	}
	keys := make([]jose.JSONWebKey, 0, len(doc.Keys))
	for _, raw := range doc.Keys {
		var k jose.JSONWebKey
		// A key of an unsupported type must not hide the usable ones.
		if err := k.UnmarshalJSON(raw); err != nil || !k.Valid() || k.Use == "enc" {
			continue
		}
		keys = append(keys, k)
	}
	if len(keys) == 0 {
		return nil, errors.New("jwks contains no usable signing key")
	}
	return keys, nil
}
