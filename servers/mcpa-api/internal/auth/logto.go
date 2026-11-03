// SPDX-License-Identifier: FSL-1.1-ALv2

package auth

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/coreos/go-oidc/v3/oidc"
)

// ErrInvalidGrant means Logto rejected a code or refresh token. The user
// must sign in again.
var ErrInvalidGrant = errors.New("logto: invalid grant")

// ErrLogtoUnavailable means Logto could not be reached or answered with an
// unexpected response.
var ErrLogtoUnavailable = errors.New("logto: unavailable")

// Logto talks to the Logto OIDC provider. Discovery is lazy so the API can
// start while Logto is unreachable; a failed discovery is retried on the
// next use.
type Logto struct {
	issuer       string
	clientID     string
	clientSecret string
	httpClient   *http.Client

	mu sync.Mutex
	// inflight is the running discovery attempt, nil when none runs.
	inflight *discoverCall
	provider *oidc.Provider
	meta     providerMetadata
	// accessVerifier and idVerifier share one cached JWKS key set. They are
	// built once after a successful discovery.
	accessVerifier *oidc.IDTokenVerifier
	idVerifier     *oidc.IDTokenVerifier
}

type providerMetadata struct {
	AuthorizationEndpoint string `json:"authorization_endpoint"`
	TokenEndpoint         string `json:"token_endpoint"`
	EndSessionEndpoint    string `json:"end_session_endpoint"`
	RevocationEndpoint    string `json:"revocation_endpoint"`
	JWKSURI               string `json:"jwks_uri"`
	// IDTokenSigningAlgs restricts accepted token algorithms the same way
	// oidc.Provider.Verifier does.
	IDTokenSigningAlgs []string `json:"id_token_signing_alg_values_supported"`
}

// supportedSigningAlgs lists the algorithms go-oidc accepts from discovery.
var supportedSigningAlgs = map[string]bool{
	"RS256": true, "RS384": true, "RS512": true,
	"ES256": true, "ES384": true, "ES512": true,
	"PS256": true, "PS384": true, "PS512": true,
	"EdDSA": true,
}

func (m providerMetadata) signingAlgs() []string {
	var algs []string
	for _, a := range m.IDTokenSigningAlgs {
		if supportedSigningAlgs[a] {
			algs = append(algs, a)
		}
	}
	return algs
}

// NewLogto configures the provider. clientID and clientSecret identify the
// mcpa traditional web application used by the console BFF and the MCP
// OAuth broker.
func NewLogto(issuer, clientID, clientSecret string, httpClient *http.Client) *Logto {
	if httpClient == nil {
		httpClient = &http.Client{Timeout: 10 * time.Second}
	}
	return &Logto{issuer: issuer, clientID: clientID, clientSecret: clientSecret, httpClient: httpClient}
}

// ClientID returns the Logto application client ID.
func (l *Logto) ClientID() string { return l.clientID }

// defaultDiscoveryTimeout bounds one discovery attempt when the HTTP client
// has no timeout of its own.
const defaultDiscoveryTimeout = 10 * time.Second

// discoverCall is one in-flight discovery attempt shared by every caller that
// arrives while it runs.
type discoverCall struct {
	done chan struct{}
	// err is written before done is closed and read only after it.
	err error
}

// discover returns the discovered provider, running at most one discovery
// attempt at a time. Concurrent callers during an outage share the single
// attempt and fail together after at most one discovery timeout instead of
// queueing one full attempt each. The attempt is detached from the caller that
// started it, so a cancelled caller neither aborts it for the others nor
// leaves the mutex held. Only success is cached; a failed attempt is dropped
// when it completes and the next call starts a new one.
func (l *Logto) discover(ctx context.Context) (*oidc.Provider, providerMetadata, error) {
	l.mu.Lock()
	if l.provider != nil {
		p, meta := l.provider, l.meta
		l.mu.Unlock()
		return p, meta, nil
	}
	call := l.inflight
	if call == nil {
		call = &discoverCall{done: make(chan struct{})}
		l.inflight = call
		go l.runDiscovery(context.WithoutCancel(ctx), call)
	}
	l.mu.Unlock()

	select {
	case <-call.done:
	case <-ctx.Done():
		return nil, providerMetadata{}, fmt.Errorf("logto: discovery: %w", ctx.Err())
	}
	if call.err != nil {
		return nil, providerMetadata{}, call.err
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.provider, l.meta, nil
}

// runDiscovery performs the shared discovery attempt and publishes its
// outcome. ctx must not be cancelled by the starting caller.
func (l *Logto) runDiscovery(ctx context.Context, call *discoverCall) {
	timeout := l.httpClient.Timeout
	if timeout <= 0 {
		timeout = defaultDiscoveryTimeout
	}
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	err := l.fetchDiscovery(ctx)
	call.err = err
	l.mu.Lock()
	l.inflight = nil
	l.mu.Unlock()
	close(call.done)
}

// fetchDiscovery fetches the provider metadata and, on success, stores the
// provider and verifiers.
func (l *Logto) fetchDiscovery(ctx context.Context) error {
	p, err := oidc.NewProvider(oidc.ClientContext(ctx, l.httpClient), l.issuer)
	if err != nil {
		return fmt.Errorf("%w: discovery: %v", ErrLogtoUnavailable, err)
	}
	var meta providerMetadata
	if err := p.Claims(&meta); err != nil || meta.AuthorizationEndpoint == "" || meta.TokenEndpoint == "" {
		return fmt.Errorf("%w: discovery metadata is incomplete", ErrLogtoUnavailable)
	}
	if meta.JWKSURI == "" {
		return fmt.Errorf("%w: discovery metadata has no jwks_uri", ErrLogtoUnavailable)
	}
	// One key set serves every verification: keys are cached and refetched
	// only for an unknown key ID.
	keys := newJWKSKeySet(meta.JWKSURI, l.httpClient)
	algs := meta.signingAlgs()
	l.mu.Lock()
	defer l.mu.Unlock()
	// Access tokens are not ID tokens: the audience is the API resource, not
	// the client ID, so the client check is replaced by an explicit audience
	// check in VerifyAccessToken.
	l.accessVerifier = oidc.NewVerifier(l.issuer, keys, &oidc.Config{SkipClientIDCheck: true, SupportedSigningAlgs: algs})
	l.idVerifier = oidc.NewVerifier(l.issuer, keys, &oidc.Config{ClientID: l.clientID, SupportedSigningAlgs: algs})
	l.provider = p
	l.meta = meta
	return nil
}

// Endpoints returns the discovered authorization, token and end-session
// endpoints.
func (l *Logto) Endpoints(ctx context.Context) (authorization, token, endSession string, err error) {
	_, meta, err := l.discover(ctx)
	if err != nil {
		return "", "", "", err
	}
	return meta.AuthorizationEndpoint, meta.TokenEndpoint, meta.EndSessionEndpoint, nil
}

// VerifiedToken is a verified Logto JWT.
type VerifiedToken struct {
	Subject  string
	Audience []string
	Expiry   time.Time
	IssuedAt time.Time
	// ClientID is the client_id claim of an access token, when present.
	ClientID string
	Claims   map[string]any
	// Digest is the SHA-256 digest of the raw token. The MCP OAuth broker
	// binds issued access tokens to their grant by digest; the raw token is
	// never kept.
	Digest []byte
}

// VerifyAccessToken verifies the signature, issuer and expiry of a Logto
// access token and requires audience among its aud values.
//
// Failures to obtain Logto signing keys wrap ErrLogtoUnavailable. Every other
// failure is a token failure and does not.
func (l *Logto) VerifyAccessToken(ctx context.Context, raw, audience string) (*VerifiedToken, error) {
	if _, _, err := l.discover(ctx); err != nil {
		return nil, err
	}
	l.mu.Lock()
	v := l.accessVerifier
	l.mu.Unlock()
	ctx, outage := withJWKSOutage(ctx)
	tok, err := v.Verify(ctx, raw)
	if err != nil {
		if outage.err != nil {
			return nil, fmt.Errorf("%w: signing keys: %v", ErrLogtoUnavailable, outage.err)
		}
		return nil, fmt.Errorf("verify access token: %w", err)
	}
	if !slices.Contains(tok.Audience, audience) {
		return nil, errors.New("verify access token: audience mismatch")
	}
	out, err := toVerified(tok)
	if err != nil {
		return nil, err
	}
	out.Digest = Digest(raw)
	return out, nil
}

// VerifyIDToken verifies an ID token for the configured client and nonce.
// Failures to obtain Logto signing keys wrap ErrLogtoUnavailable.
func (l *Logto) VerifyIDToken(ctx context.Context, raw, nonce string) (*VerifiedToken, error) {
	if _, _, err := l.discover(ctx); err != nil {
		return nil, err
	}
	l.mu.Lock()
	v := l.idVerifier
	l.mu.Unlock()
	ctx, outage := withJWKSOutage(ctx)
	tok, err := v.Verify(ctx, raw)
	if err != nil {
		if outage.err != nil {
			return nil, fmt.Errorf("%w: signing keys: %v", ErrLogtoUnavailable, outage.err)
		}
		return nil, fmt.Errorf("verify id token: %w", err)
	}
	if tok.Nonce != nonce {
		return nil, errors.New("verify id token: nonce mismatch")
	}
	return toVerified(tok)
}

func toVerified(tok *oidc.IDToken) (*VerifiedToken, error) {
	claims := map[string]any{}
	if err := tok.Claims(&claims); err != nil {
		return nil, err
	}
	out := &VerifiedToken{Subject: tok.Subject, Audience: tok.Audience, Expiry: tok.Expiry, IssuedAt: tok.IssuedAt, Claims: claims}
	if cid, ok := claims["client_id"].(string); ok {
		out.ClientID = cid
	}
	if out.Subject == "" {
		return nil, errors.New("token has no subject")
	}
	return out, nil
}

// TokenResponse is a Logto token endpoint response. Its fields are secrets.
type TokenResponse struct {
	AccessToken  string `json:"access_token"`
	TokenType    string `json:"token_type"`
	ExpiresIn    int64  `json:"expires_in"`
	RefreshToken string `json:"refresh_token"`
	IDToken      string `json:"id_token"`
	Scope        string `json:"scope"`
}

type tokenError struct {
	Error string `json:"error"`
}

// ExchangeCode redeems an authorization code with PKCE for an access token
// of resource.
func (l *Logto) ExchangeCode(ctx context.Context, code, verifier, redirectURI, resource string) (*TokenResponse, error) {
	return l.token(ctx, url.Values{
		"grant_type":    {"authorization_code"},
		"code":          {code},
		"code_verifier": {verifier},
		"redirect_uri":  {redirectURI},
		"resource":      {resource},
	})
}

// RefreshForResource redeems a refresh token for an access token of
// resource. Logto may rotate the refresh token; callers must keep the
// returned one when it is non-empty.
func (l *Logto) RefreshForResource(ctx context.Context, refreshToken, resource string) (*TokenResponse, error) {
	return l.token(ctx, url.Values{
		"grant_type":    {"refresh_token"},
		"refresh_token": {refreshToken},
		"resource":      {resource},
	})
}

func (l *Logto) token(ctx context.Context, form url.Values) (*TokenResponse, error) {
	_, meta, err := l.discover(ctx)
	if err != nil {
		return nil, err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, meta.TokenEndpoint, strings.NewReader(form.Encode()))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	req.Header.Set("Accept", "application/json")
	req.SetBasicAuth(url.QueryEscape(l.clientID), url.QueryEscape(l.clientSecret))
	resp, err := l.httpClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("%w: token request: %v", ErrLogtoUnavailable, err)
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
	if err != nil {
		return nil, fmt.Errorf("%w: token response: %v", ErrLogtoUnavailable, err)
	}
	if resp.StatusCode != http.StatusOK {
		var te tokenError
		_ = json.Unmarshal(body, &te)
		// Only the OAuth error code is kept; descriptions may echo input.
		if te.Error == "invalid_grant" {
			return nil, ErrInvalidGrant
		}
		// 429 is Logto rate limiting: a retryable outage, not a rejected
		// grant, so callers keep the credential usable.
		if resp.StatusCode >= 500 || resp.StatusCode == http.StatusTooManyRequests {
			return nil, fmt.Errorf("%w: token endpoint status %d", ErrLogtoUnavailable, resp.StatusCode)
		}
		return nil, fmt.Errorf("logto: token endpoint status %d error %q", resp.StatusCode, te.Error)
	}
	var tr TokenResponse
	if err := json.Unmarshal(body, &tr); err != nil || tr.AccessToken == "" {
		return nil, fmt.Errorf("%w: malformed token response", ErrLogtoUnavailable)
	}
	return &tr, nil
}

// RevokeRefreshToken revokes a refresh token at the Logto revocation
// endpoint (RFC 7009). It is a no-op when Logto publishes no revocation
// endpoint.
func (l *Logto) RevokeRefreshToken(ctx context.Context, refreshToken string) error {
	_, meta, err := l.discover(ctx)
	if err != nil {
		return err
	}
	if meta.RevocationEndpoint == "" {
		return nil
	}
	form := url.Values{"token": {refreshToken}, "token_type_hint": {"refresh_token"}}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, meta.RevocationEndpoint, strings.NewReader(form.Encode()))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	req.SetBasicAuth(url.QueryEscape(l.clientID), url.QueryEscape(l.clientSecret))
	resp, err := l.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("%w: revocation request: %v", ErrLogtoUnavailable, err)
	}
	defer resp.Body.Close()
	_, _ = io.Copy(io.Discard, io.LimitReader(resp.Body, 1<<16))
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("logto: revocation endpoint status %d", resp.StatusCode)
	}
	return nil
}

// MFAEvidence reports whether verified token claims carry MFA evidence: the
// configured boolean claim set to true, or a standard amr array containing
// mfa, otp or hwk.
func MFAEvidence(claims map[string]any, claimName string) bool {
	if v, ok := claims[claimName].(bool); ok && v {
		return true
	}
	amr, ok := claims["amr"].([]any)
	if !ok {
		return false
	}
	for _, m := range amr {
		switch m {
		case "mfa", "otp", "hwk":
			return true
		}
	}
	return false
}
