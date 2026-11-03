// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"strings"
	"sync"
	"time"

	coreauth "github.com/router-for-me/CLIProxyAPI/v6/sdk/cliproxy/auth"
	"golang.org/x/oauth2"
)

// Gemini CLI token refresh
//
// The SDK Gemini CLI executor refreshes an expired access token lazily inside
// every Execute, ExecuteStream and CountTokens call. That path is unusable for
// mcpa for three reasons:
//
//   - it works on a per-call copy of the auth, so the refreshed token is
//     never written back to the registered auth or to PostgreSQL and every
//     later call refreshes again;
//   - it uses the SDK's built-in OAuth client, not the client stored in the
//     credential payload or configured by the operator, so credentials
//     connected through an operator-configured Google client cannot refresh;
//   - its token request uses the call's metering RoundTripper, so the refresh
//     traffic is billed as call bytes.
//
// geminiRefresher refreshes the token before the executor runs, with the
// payload's own OAuth client, through the destination-guarded base transport
// (not the metering transport), shares one refresh among concurrent calls of
// a credential, queues the new payload for encrypted persistence (which also
// notifies the other replicas) and hands the executor an auth whose token is
// valid, so the SDK's lazy path finds nothing to refresh.

const (
	// googleTokenURL is the Google OAuth token endpoint. The payload's
	// token_uri is deliberately ignored: the payload is stored data and the
	// refresh request carries the refresh token and the client secret, so the
	// endpoint must not be steerable by payload content.
	googleTokenURL = "https://oauth2.googleapis.com/token"
	// geminiRefreshSkew refreshes a token that expires within this window. It
	// is larger than the SDK's own expiry margin (oauth2 treats a token as
	// expired 10 seconds early), so the SDK never refreshes a token that
	// passed this check.
	geminiRefreshSkew = 60 * time.Second
	// geminiRefreshTimeout bounds one token request.
	geminiRefreshTimeout = 30 * time.Second
)

// Token endpoint error codes that mean the provider rejected the credential
// itself (revoked or expired refresh token, or an OAuth client that is not
// allowed to use it), as opposed to a failed exchange.
const (
	oauthErrInvalidGrant       = "invalid_grant"
	oauthErrUnauthorizedClient = "unauthorized_client"
	oauthErrInvalidClient      = "invalid_client"
)

// Customer-visible refresh error texts. They never include the token
// endpoint's own text, which can name internal addresses.
const (
	geminiRefreshAuthMessage   = "the Gemini CLI credential could not be refreshed; reconnect the account"
	geminiRefreshFailedMessage = "the Gemini CLI token refresh failed"
)

// geminiRefreshError is the executor error of a failed refresh. Its text is
// the customer-visible error body: the SDK returns the error text verbatim
// when it is valid JSON, and the status from StatusCode.
type geminiRefreshError struct {
	status int
	// code is the provider's OAuth error code, empty for transport failures.
	code string
	// network is true when the token request got no HTTP response.
	network bool
}

func (e *geminiRefreshError) Error() string {
	message, errType := geminiRefreshFailedMessage, "server_error"
	if e.status == http.StatusUnauthorized {
		message, errType = geminiRefreshAuthMessage, "authentication_error"
	}
	body := struct {
		Error struct {
			Message string `json:"message"`
			Type    string `json:"type"`
			Code    string `json:"code,omitempty"`
		} `json:"error"`
	}{}
	body.Error.Message, body.Error.Type, body.Error.Code = message, errType, e.code
	out, err := json.Marshal(body)
	if err != nil {
		return message
	}
	return string(out)
}

func (e *geminiRefreshError) StatusCode() int { return e.status }

// networkFailure marks errors without a provider response so the call is
// classified as a network failure (see callState.observeExecutorResult).
func (e *geminiRefreshError) networkFailure() bool { return e.network }

// mapGeminiRefreshError converts a token request error into the executor
// error. A provider rejection of the credential becomes a 401 carrying the
// provider's error code; every other failure is a 502.
func mapGeminiRefreshError(err error) *geminiRefreshError {
	var retrieve *oauth2.RetrieveError
	if errors.As(err, &retrieve) {
		code := retrieve.ErrorCode
		switch {
		case code == oauthErrInvalidGrant, code == oauthErrUnauthorizedClient, code == oauthErrInvalidClient:
			return &geminiRefreshError{status: http.StatusUnauthorized, code: code}
		case retrieve.Response != nil && retrieve.Response.StatusCode == http.StatusUnauthorized:
			if code == "" {
				code = oauthErrInvalidGrant
			}
			return &geminiRefreshError{status: http.StatusUnauthorized, code: code}
		}
		return &geminiRefreshError{status: http.StatusBadGateway, code: code}
	}
	return &geminiRefreshError{status: http.StatusBadGateway, network: true}
}

// geminiRefreshed is the outcome of a refresh: the credential's full payload
// with the new token, derived from the registered payload identified by
// source.
type geminiRefreshed struct {
	source [sha256.Size]byte
	meta   map[string]any
	token  *oauth2.Token
}

type geminiFlight struct {
	done  chan struct{}
	state *geminiRefreshed
	err   error
}

type geminiKey struct {
	resourceID   string
	credentialID string
}

// geminiRefresher refreshes Gemini CLI tokens for all resources of an engine.
type geminiRefresher struct {
	transport http.RoundTripper
	// clientID and clientSecret are the operator-configured fallback OAuth
	// client for payloads that carry none.
	clientID     string
	clientSecret string
	persister    *asyncPersister
	logger       *slog.Logger
	now          func() time.Time

	mu      sync.Mutex
	flights map[geminiKey]*geminiFlight
	// latest holds the last refreshed payload per credential. The registered
	// auth keeps the payload it was loaded with (changing it would need the
	// SDK manager's lock), so every call re-applies latest when it was
	// derived from that same registered payload. A reload with a different
	// payload, for example after a replica notification, supersedes it.
	latest map[geminiKey]*geminiRefreshed
}

func newGeminiRefresher(transport http.RoundTripper, clientID, clientSecret string, persister *asyncPersister, logger *slog.Logger, now func() time.Time) *geminiRefresher {
	return &geminiRefresher{
		transport:    transport,
		clientID:     strings.TrimSpace(clientID),
		clientSecret: strings.TrimSpace(clientSecret),
		persister:    persister,
		logger:       logger,
		now:          now,
		flights:      map[geminiKey]*geminiFlight{},
		latest:       map[geminiKey]*geminiRefreshed{},
	}
}

// forget drops the state of a removed credential.
func (r *geminiRefresher) forget(resourceID, credentialID string) {
	if r == nil {
		return
	}
	r.mu.Lock()
	delete(r.latest, geminiKey{resourceID, credentialID})
	r.mu.Unlock()
}

// ensureFresh returns an auth whose token is valid for the next minute. The
// registered auth passed in is never modified.
func (r *geminiRefresher) ensureFresh(ctx context.Context, resourceID string, auth *coreauth.Auth) (*coreauth.Auth, error) {
	if r == nil || auth == nil || auth.Metadata == nil {
		return auth, nil
	}
	key := geminiKey{resourceID, auth.Attributes[attrCredentialID]}
	source := geminiFingerprint(auth.Metadata)
	token := geminiTokenOf(auth.Metadata)
	if cached := r.cached(key, source); cached != nil {
		auth = geminiApplied(auth, cached)
		token = cached.token
	}
	if !r.needsRefresh(token) {
		return auth, nil
	}
	if _, _, ok := r.client(auth.Metadata); !ok {
		// No OAuth client is known; leave the refresh to the SDK path.
		return auth, nil
	}
	flight, leader := r.join(key)
	if leader {
		// The refresh is detached from this call: its outcome is shared by
		// every waiting call, and a cancelled leader must not fail them.
		go r.run(key, flight, auth.Metadata, source)
	}
	select {
	case <-flight.done:
	case <-ctx.Done():
		return nil, ctx.Err()
	}
	if flight.err != nil {
		return nil, flight.err
	}
	return geminiApplied(auth, flight.state), nil
}

func (r *geminiRefresher) needsRefresh(token *oauth2.Token) bool {
	if token.RefreshToken == "" {
		return false
	}
	if token.AccessToken == "" {
		return true
	}
	return !token.Expiry.IsZero() && token.Expiry.Before(r.now().Add(geminiRefreshSkew))
}

// cached returns the latest refreshed payload when it was derived from the
// registered payload with the given fingerprint and is still fresh enough to
// be useful; a payload derived from another registered payload is dropped.
func (r *geminiRefresher) cached(key geminiKey, source [sha256.Size]byte) *geminiRefreshed {
	r.mu.Lock()
	defer r.mu.Unlock()
	state := r.latest[key]
	if state == nil {
		return nil
	}
	if state.source != source {
		delete(r.latest, key)
		return nil
	}
	return state
}

// join returns the in-flight refresh of key, creating it when absent. leader
// reports whether the caller must run it.
func (r *geminiRefresher) join(key geminiKey) (*geminiFlight, bool) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if flight := r.flights[key]; flight != nil {
		return flight, false
	}
	flight := &geminiFlight{done: make(chan struct{})}
	r.flights[key] = flight
	return flight, true
}

// run performs the refresh of one flight and publishes its outcome.
func (r *geminiRefresher) run(key geminiKey, flight *geminiFlight, registered map[string]any, source [sha256.Size]byte) {
	state, err := r.refresh(key, registered, source)
	r.mu.Lock()
	if err == nil {
		r.latest[key] = state
	}
	delete(r.flights, key)
	r.mu.Unlock()
	flight.state, flight.err = state, err
	close(flight.done)
}

func (r *geminiRefresher) refresh(key geminiKey, registered map[string]any, source [sha256.Size]byte) (*geminiRefreshed, error) {
	// A refresh that completed after the caller looked at the cache makes
	// this one unnecessary.
	if cached := r.cached(key, source); cached != nil && !r.needsRefresh(cached.token) {
		return cached, nil
	}
	current := geminiTokenOf(registered)
	clientID, clientSecret, ok := r.client(registered)
	if !ok {
		return nil, &geminiRefreshError{status: http.StatusBadGateway}
	}
	conf := &oauth2.Config{
		ClientID:     clientID,
		ClientSecret: clientSecret,
		Endpoint:     oauth2.Endpoint{TokenURL: googleTokenURL, AuthStyle: oauth2.AuthStyleInParams},
	}
	ctx, cancel := context.WithTimeout(context.Background(), geminiRefreshTimeout)
	defer cancel()
	// The client uses the guarded base transport directly: the metering
	// transport would bill the refresh traffic to a call.
	ctx = context.WithValue(ctx, oauth2.HTTPClient, &http.Client{Transport: r.transport})
	// A token without an access token is always expired, which forces the
	// refresh-token exchange.
	fresh, err := conf.TokenSource(ctx, &oauth2.Token{RefreshToken: current.RefreshToken}).Token()
	if err != nil {
		mapped := mapGeminiRefreshError(err)
		r.logger.Warn("gemini cli token refresh failed",
			slog.String("resource_id", key.resourceID),
			slog.String("credential_id", key.credentialID),
			slog.Int("status", mapped.status),
			slog.String("oauth_error", mapped.code),
			slog.Bool("network", mapped.network))
		return nil, mapped
	}
	if fresh.RefreshToken == "" {
		// Google normally keeps the refresh token; keep ours when the
		// response does not rotate it.
		fresh.RefreshToken = current.RefreshToken
	}
	meta := geminiWithToken(registered, fresh)
	state := &geminiRefreshed{source: source, meta: meta, token: fresh}
	if payload, err := json.Marshal(meta); err == nil {
		r.persister.enqueue(key.resourceID, key.credentialID, payload)
	} else {
		r.logger.Error("encoding the refreshed gemini cli credential failed",
			slog.String("resource_id", key.resourceID),
			slog.String("credential_id", key.credentialID))
	}
	r.logger.Info("gemini cli token refreshed",
		slog.String("resource_id", key.resourceID),
		slog.String("credential_id", key.credentialID),
		slog.Time("expiry", fresh.Expiry))
	return state, nil
}

// client returns the OAuth client for a payload: the client stored in the
// payload's token object, else the operator-configured client.
func (r *geminiRefresher) client(meta map[string]any) (string, string, bool) {
	token, _ := meta["token"].(map[string]any)
	id, _ := token["client_id"].(string)
	secret, _ := token["client_secret"].(string)
	if id != "" && secret != "" {
		return id, secret, true
	}
	if r.clientID != "" && r.clientSecret != "" {
		return r.clientID, r.clientSecret, true
	}
	return "", "", false
}

// geminiTokenOf reads the token the way the SDK does: the nested token object
// first, then the top-level fields.
func geminiTokenOf(meta map[string]any) *oauth2.Token {
	token := &oauth2.Token{}
	if nested, ok := meta["token"].(map[string]any); ok && len(nested) > 0 {
		if raw, err := json.Marshal(nested); err == nil {
			_ = json.Unmarshal(raw, token)
		}
	}
	if token.AccessToken == "" {
		token.AccessToken, _ = meta["access_token"].(string)
	}
	if token.RefreshToken == "" {
		token.RefreshToken, _ = meta["refresh_token"].(string)
	}
	if token.Expiry.IsZero() {
		if expiry, _ := meta["expiry"].(string); expiry != "" {
			if parsed, err := time.Parse(time.RFC3339, expiry); err == nil {
				token.Expiry = parsed
			}
		}
	}
	return token
}

// geminiFingerprint identifies the token material of a registered payload.
func geminiFingerprint(meta map[string]any) [sha256.Size]byte {
	token := geminiTokenOf(meta)
	return sha256.Sum256([]byte(fmt.Sprintf("%s\x00%s\x00%d", token.AccessToken, token.RefreshToken, token.Expiry.UnixNano())))
}

// geminiWithToken returns a copy of the payload carrying the token in the
// nested token object and in the top-level fields the SDK reads.
func geminiWithToken(meta map[string]any, token *oauth2.Token) map[string]any {
	out := make(map[string]any, len(meta)+4)
	for k, v := range meta {
		out[k] = v
	}
	nested := map[string]any{}
	if existing, ok := meta["token"].(map[string]any); ok {
		for k, v := range existing {
			nested[k] = v
		}
	}
	expiry := token.Expiry.UTC().Format(time.RFC3339Nano)
	nested["access_token"] = token.AccessToken
	if token.TokenType != "" {
		nested["token_type"] = token.TokenType
	}
	if token.RefreshToken != "" {
		nested["refresh_token"] = token.RefreshToken
	}
	if !token.Expiry.IsZero() {
		nested["expiry"] = expiry
	}
	out["token"] = nested
	// Top-level fields exist only in payloads that already carried them.
	for field, value := range map[string]string{
		"access_token":  token.AccessToken,
		"token_type":    token.TokenType,
		"refresh_token": token.RefreshToken,
	} {
		if _, ok := meta[field]; ok && value != "" {
			out[field] = value
		}
	}
	if _, ok := meta["expiry"]; ok && !token.Expiry.IsZero() {
		out["expiry"] = token.Expiry.Format(time.RFC3339)
	}
	return out
}

// geminiApplied returns a copy of auth that carries the refreshed payload.
// The SDK executor writes into the metadata of the auth it receives, so each
// call gets its own copy of the maps.
func geminiApplied(auth *coreauth.Auth, state *geminiRefreshed) *coreauth.Auth {
	out := auth.Clone()
	out.Metadata = make(map[string]any, len(state.meta))
	for k, v := range state.meta {
		out.Metadata[k] = v
	}
	if nested, ok := state.meta["token"].(map[string]any); ok {
		copied := make(map[string]any, len(nested))
		for k, v := range nested {
			copied[k] = v
		}
		out.Metadata["token"] = copied
	}
	return out
}
