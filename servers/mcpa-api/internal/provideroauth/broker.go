// SPDX-License-Identifier: FSL-1.1-ALv2

// Package provideroauth runs the provider OAuth flows that connect customer
// accounts (Claude Code, Codex, Gemini CLI, Antigravity and Kimi) to a
// resource. Flow state lives encrypted in PostgreSQL, so a flow survives
// restarts and can be completed or polled by any replica.
package provideroauth

import (
	"context"
	"crypto/subtle"
	"encoding/json"
	"errors"
	"log/slog"
	"net/http"
	"net/url"
	"regexp"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"google.golang.org/protobuf/types/known/timestamppb"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/credential"
	"github.com/delinoio/apps/servers/mcpa-api/internal/crypto"
	"github.com/delinoio/apps/servers/mcpa-api/internal/proxy"
	"github.com/delinoio/apps/servers/mcpa-api/internal/service"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

// authorizationCodeLifetime is how long a customer has to consent and paste
// the final redirect URL.
const authorizationCodeLifetime = 10 * time.Minute

// Options configures the broker.
type Options struct {
	Store *store.Store
	Keys  *crypto.Keyring
	// Listener propagates a registered credential to the data plane: inside
	// the registration transaction for other replicas and after the commit
	// for this process (see service.CredentialListener).
	Listener service.CredentialListener
	Log      *slog.Logger
	// HTTPClient performs provider requests. Default: a client with Timeout.
	HTTPClient *http.Client
	// ClaudeHTTPClient performs Claude token requests. Default: a client with
	// a Chrome TLS fingerprint, as the SDK uses for Anthropic.
	ClaudeHTTPClient *http.Client
	// Endpoints overrides provider URLs; empty fields use the defaults.
	Endpoints Endpoints
	// GeminiCLIClient and AntigravityClient are the Google installed-app
	// OAuth clients of those providers. A provider whose client is not
	// configured rejects new flows with FAILED_PRECONDITION.
	GeminiCLIClient   GoogleOAuthClient
	AntigravityClient GoogleOAuthClient
	// Timeout bounds each provider request. Default 30s.
	Timeout time.Duration
	// SetupTimeout bounds the Gemini Code Assist project setup. Default 60s.
	SetupTimeout time.Duration
	// Now defaults to time.Now.
	Now func() time.Time
}

// GoogleOAuthClient is a Google OAuth client ID and secret supplied by
// operator configuration.
type GoogleOAuthClient struct {
	ClientID     string
	ClientSecret string
}

func (c GoogleOAuthClient) configured() bool { return c.ClientID != "" && c.ClientSecret != "" }

// Broker implements service.OAuthBroker.
type Broker struct {
	db          *store.Store
	keys        *crypto.Keyring
	listener    service.CredentialListener
	log         *slog.Logger
	http        *http.Client
	claude      *http.Client
	ep          Endpoints
	gemini      GoogleOAuthClient
	antigravity GoogleOAuthClient
	timeout     time.Duration
	setup       time.Duration
	now         func() time.Time
}

var _ service.OAuthBroker = (*Broker)(nil)

// New builds a broker.
func New(o Options) *Broker {
	if o.Timeout <= 0 {
		o.Timeout = 30 * time.Second
	}
	if o.SetupTimeout <= 0 {
		o.SetupTimeout = 60 * time.Second
	}
	if o.Now == nil {
		o.Now = time.Now
	}
	if o.Log == nil {
		o.Log = slog.Default()
	}
	if o.HTTPClient == nil {
		o.HTTPClient = &http.Client{Timeout: o.Timeout}
	}
	if o.ClaudeHTTPClient == nil {
		o.ClaudeHTTPClient = NewBrowserTLSClient(o.Timeout)
	}
	return &Broker{
		db: o.Store, keys: o.Keys, listener: o.Listener, log: o.Log.With("component", "provider_oauth"),
		http: o.HTTPClient, claude: o.ClaudeHTTPClient, ep: o.Endpoints.withDefaults(),
		gemini: o.GeminiCLIClient, antigravity: o.AntigravityClient,
		timeout: o.Timeout, setup: o.SetupTimeout, now: o.Now,
	}
}

// flowSecret is the encrypted part of a flow. It is never returned or
// logged.
type flowSecret struct {
	State        string `json:"state,omitempty"`
	CodeVerifier string `json:"code_verifier,omitempty"`
	DeviceCode   string `json:"device_code,omitempty"`
	UserCode     string `json:"user_code,omitempty"`
	DeviceID     string `json:"device_id,omitempty"`
	// IntervalSeconds is the provider's device poll interval.
	IntervalSeconds int `json:"interval,omitempty"`
	// NextPollUnixMilli gates device polls to the provider interval.
	NextPollUnixMilli int64 `json:"next_poll,omitempty"`
	// ClaimedUntilUnixMilli marks a code exchange or poll in progress, so
	// concurrent requests never exchange the same code twice. A claim of a
	// crashed process lapses at this time.
	ClaimedUntilUnixMilli int64 `json:"claimed_until,omitempty"`
}

// token is the result of a successful provider exchange.
type token struct {
	// payload is the SDK token JSON object the engine registers.
	payload map[string]any
	// label is a non-secret account label such as the email.
	label string
}

// flowError is a provider failure with a caller-safe message. The flow
// moves to state.
type flowError struct {
	state   mcpav1.OauthFlowState
	message string
}

func (e *flowError) Error() string { return e.message }

func failed(message string) *flowError {
	return &flowError{state: mcpav1.OauthFlowState_OAUTH_FLOW_STATE_FAILED, message: message}
}

func expired() *flowError {
	return &flowError{state: mcpav1.OauthFlowState_OAUTH_FLOW_STATE_EXPIRED, message: "the authorization expired; start a new flow"}
}

// deviceStart is the provider answer that starts a device flow.
type deviceStart struct {
	secret          flowSecret
	userCode        string
	verificationURI string
	lifetime        time.Duration
}

// provider is one OAuth provider's flow.
type provider struct {
	kind mcpav1.OauthFlowKind
	// Authorization-code flows.
	authURL  func(b *Broker, state, challenge string) string
	exchange func(ctx context.Context, b *Broker, code, callbackState string, sec flowSecret) (token, error)
	// Device flows. poll returns a nil token while authorization is pending.
	startDevice func(ctx context.Context, b *Broker) (deviceStart, error)
	poll        func(ctx context.Context, b *Broker, sec *flowSecret) (*token, error)
}

var providers = map[mcpav1.Provider]provider{
	mcpav1.Provider_PROVIDER_CLAUDE:      {kind: mcpav1.OauthFlowKind_OAUTH_FLOW_KIND_AUTHORIZATION_CODE, authURL: claudeAuthorizationURL, exchange: claudeExchange},
	mcpav1.Provider_PROVIDER_GEMINI_CLI:  {kind: mcpav1.OauthFlowKind_OAUTH_FLOW_KIND_AUTHORIZATION_CODE, authURL: geminiAuthorizationURL, exchange: geminiExchange},
	mcpav1.Provider_PROVIDER_ANTIGRAVITY: {kind: mcpav1.OauthFlowKind_OAUTH_FLOW_KIND_AUTHORIZATION_CODE, authURL: antigravityAuthorizationURL, exchange: antigravityExchange},
	mcpav1.Provider_PROVIDER_CODEX:       {kind: mcpav1.OauthFlowKind_OAUTH_FLOW_KIND_DEVICE_CODE, startDevice: codexStartDevice, poll: codexPoll},
	mcpav1.Provider_PROVIDER_KIMI:        {kind: mcpav1.OauthFlowKind_OAUTH_FLOW_KIND_DEVICE_CODE, startDevice: kimiStartDevice, poll: kimiPoll},
}

// FlowKind returns the flow kind the broker uses for an OAuth provider.
func FlowKind(p mcpav1.Provider) (mcpav1.OauthFlowKind, bool) {
	spec, ok := providers[p]
	return spec.kind, ok
}

func flowProto(f store.OAuthFlow) *mcpav1.ProviderOAuthFlow {
	out := &mcpav1.ProviderOAuthFlow{
		Id: f.ID.String(), ResourceId: f.ResourceID.String(), Provider: f.Provider, Kind: f.Kind, State: f.State,
		ExpireTime: timestamppb.New(f.ExpireTime), FailureMessage: f.FailureMessage,
	}
	if f.State == mcpav1.OauthFlowState_OAUTH_FLOW_STATE_PENDING {
		out.AuthorizationUrl = f.AuthorizationURL
		out.UserCode = f.UserCode
		out.VerificationUri = f.VerificationURI
	}
	if f.CredentialID != uuid.Nil {
		out.CredentialId = f.CredentialID.String()
	}
	return out
}

func (b *Broker) seal(f store.OAuthFlow, sec flowSecret) (crypto.Sealed, error) {
	plain, err := json.Marshal(sec)
	if err != nil {
		return crypto.Sealed{}, err
	}
	defer clear(plain)
	return b.keys.Seal(plain, crypto.OAuthFlowAAD(f.ID.String(), f.ResourceID.String()))
}

func (b *Broker) open(f store.OAuthFlow, s crypto.Sealed) (flowSecret, error) {
	plain, err := b.keys.Open(s, crypto.OAuthFlowAAD(f.ID.String(), f.ResourceID.String()))
	if err != nil {
		return flowSecret{}, err
	}
	defer clear(plain)
	var sec flowSecret
	err = json.Unmarshal(plain, &sec)
	return sec, err
}

func dbErr(err error) error {
	if err == nil {
		return nil
	}
	var ae *apperr.Error
	if errors.As(err, &ae) {
		return ae
	}
	if errors.Is(err, context.Canceled) {
		return apperr.As(err)
	}
	return apperr.DependencyUnavailable(err)
}

// lockLive locks the live resource and then the flow, in the same order as
// resource deletion, so the two never deadlock.
func lockLive(ctx context.Context, tx pgx.Tx, resourceID, flowID uuid.UUID) (store.OAuthFlow, crypto.Sealed, error) {
	r, err := store.LockResource(ctx, tx, resourceID)
	if errors.Is(err, store.ErrNotFound) {
		return store.OAuthFlow{}, crypto.Sealed{}, apperr.NotFound("resource not found")
	}
	if err != nil {
		return store.OAuthFlow{}, crypto.Sealed{}, err
	}
	if r.Deleted() {
		return store.OAuthFlow{}, crypto.Sealed{}, apperr.FailedPrecondition("the resource was deleted")
	}
	f, s, err := store.LockOAuthFlow(ctx, tx, resourceID, flowID)
	if errors.Is(err, store.ErrNotFound) {
		return store.OAuthFlow{}, crypto.Sealed{}, apperr.NotFound("OAuth flow not found")
	}
	return f, s, err
}

// StartProviderOAuth creates a flow. Authorization-code flows return the
// consent URL; device flows return the user code and verification URI.
func (b *Broker) StartProviderOAuth(ctx context.Context, p *auth.Principal, res store.Resource, req *mcpav1.StartProviderOAuthRequest) (*mcpav1.ProviderOAuthFlow, error) {
	spec, ok := providers[req.GetProvider()]
	if !ok {
		return nil, apperr.InvalidInput("provider does not support OAuth accounts")
	}
	if !b.clientConfigured(req.GetProvider()) {
		return nil, apperr.FailedPrecondition("OAuth accounts for this provider are not configured on this server")
	}
	now := b.now()
	f := store.OAuthFlow{
		ID: store.NewID(), ResourceID: res.ID, TenantID: res.TenantID, Provider: req.GetProvider(), Kind: spec.kind,
		State: mcpav1.OauthFlowState_OAUTH_FLOW_STATE_PENDING, Name: req.GetName(), CreatedByUserID: p.UserID,
	}
	var sec flowSecret
	switch spec.kind {
	case mcpav1.OauthFlowKind_OAUTH_FLOW_KIND_AUTHORIZATION_CODE:
		state, err := randomToken(32)
		if err != nil {
			return nil, apperr.Internal(err)
		}
		verifier, challenge, err := newPKCE()
		if err != nil {
			return nil, apperr.Internal(err)
		}
		sec = flowSecret{State: state, CodeVerifier: verifier}
		f.AuthorizationURL = spec.authURL(b, state, challenge)
		f.ExpireTime = now.Add(authorizationCodeLifetime)
	case mcpav1.OauthFlowKind_OAUTH_FLOW_KIND_DEVICE_CODE:
		reqCtx, cancel := context.WithTimeout(ctx, b.timeout)
		start, err := spec.startDevice(reqCtx, b)
		cancel()
		if err != nil {
			b.log.WarnContext(ctx, "provider device login could not start", "provider", req.GetProvider().String(), "error", err)
			return nil, apperr.DependencyUnavailable(err)
		}
		sec = start.secret
		sec.NextPollUnixMilli = now.Add(time.Duration(sec.IntervalSeconds) * time.Second).UnixMilli()
		f.UserCode = start.userCode
		f.VerificationURI = start.verificationURI
		f.ExpireTime = now.Add(start.lifetime)
	}
	sealed, err := b.seal(f, sec)
	if err != nil {
		return nil, apperr.Internal(err)
	}
	err = b.db.InTx(ctx, func(tx pgx.Tx) error {
		r, err := store.LockResource(ctx, tx, res.ID)
		if err != nil {
			return err
		}
		if r.Deleted() {
			return apperr.FailedPrecondition("the resource was deleted")
		}
		f, err = store.InsertOAuthFlow(ctx, tx, f, sealed)
		return err
	})
	if err != nil {
		return nil, dbErr(err)
	}
	b.log.InfoContext(ctx, "provider OAuth flow started", "resource_id", res.ID, "flow_id", f.ID, "provider", f.Provider.String(), "kind", f.Kind.String())
	return flowProto(f), nil
}

// callback is the parsed final redirect URL.
type callback struct {
	code, state, err string
}

var oauthErrorCode = regexp.MustCompile(`^[a-z_]{1,64}$`)

// parseCallback reads the code, state and error from the final redirect
// URL. Claude puts "code#state" in the code parameter.
func parseCallback(raw string) (callback, error) {
	raw = strings.TrimSpace(raw)
	if raw == "" || len(raw) > 8192 {
		return callback{}, apperr.InvalidInput("callback_url must be the full redirect URL from the browser")
	}
	u, err := url.Parse(raw)
	if err != nil {
		return callback{}, apperr.InvalidInput("callback_url must be the full redirect URL from the browser")
	}
	q := u.Query()
	if u.Fragment != "" {
		if fq, err := url.ParseQuery(u.Fragment); err == nil {
			for _, k := range []string{"code", "state", "error"} {
				if q.Get(k) == "" && fq.Get(k) != "" {
					q.Set(k, fq.Get(k))
				}
			}
		}
	}
	cb := callback{code: strings.TrimSpace(q.Get("code")), state: strings.TrimSpace(q.Get("state")), err: strings.TrimSpace(q.Get("error"))}
	if code, state, ok := strings.Cut(cb.code, "#"); ok {
		cb.code = code
		if cb.state == "" {
			cb.state = state
		}
	}
	if cb.err == "" && cb.code == "" {
		return callback{}, apperr.InvalidInput("callback_url does not contain an authorization code")
	}
	return cb, nil
}

// claim marks a flow as being exchanged or polled. It reports false when
// another request holds an unexpired claim.
func (b *Broker) claim(ctx context.Context, tx pgx.Tx, f store.OAuthFlow, sec *flowSecret, now time.Time) (bool, error) {
	if sec.ClaimedUntilUnixMilli > now.UnixMilli() {
		return false, nil
	}
	sec.ClaimedUntilUnixMilli = now.Add(2*b.timeout + b.setup).UnixMilli()
	sealed, err := b.seal(f, *sec)
	if err != nil {
		return false, err
	}
	return true, store.UpdateOAuthFlowSecret(ctx, tx, f.ID, sealed)
}

// finish moves a flow to a terminal state.
func (b *Broker) finish(ctx context.Context, f store.OAuthFlow, ferr *flowError) (*mcpav1.ProviderOAuthFlow, error) {
	var out store.OAuthFlow
	err := b.db.InTx(ctx, func(tx pgx.Tx) error {
		cur, _, err := lockLive(ctx, tx, f.ResourceID, f.ID)
		if err != nil {
			return err
		}
		if cur.State != mcpav1.OauthFlowState_OAUTH_FLOW_STATE_PENDING {
			out = cur
			return nil
		}
		out, err = store.FinishOAuthFlow(ctx, tx, f.ID, ferr.state, uuid.Nil, ferr.message)
		return err
	})
	if err != nil {
		return nil, dbErr(err)
	}
	b.log.InfoContext(ctx, "provider OAuth flow ended", "resource_id", f.ResourceID, "flow_id", f.ID, "state", out.State.String())
	return flowProto(out), nil
}

// CompleteProviderOAuth exchanges the code of an authorization-code flow
// for tokens and registers the credential.
func (b *Broker) CompleteProviderOAuth(ctx context.Context, p *auth.Principal, res store.Resource, req *mcpav1.CompleteProviderOAuthRequest) (*mcpav1.ProviderOAuthFlow, error) {
	flowID, err := uuid.Parse(req.GetFlowId())
	if err != nil {
		return nil, apperr.InvalidInput("flow_id must be a canonical UUID")
	}
	cb, cbErr := parseCallback(req.GetCallbackUrl())
	var (
		f        store.OAuthFlow
		sec      flowSecret
		terminal *mcpav1.ProviderOAuthFlow
		ferr     *flowError
	)
	now := b.now()
	err = b.db.InTx(ctx, func(tx pgx.Tx) error {
		var sealed crypto.Sealed
		var err error
		f, sealed, err = lockLive(ctx, tx, res.ID, flowID)
		if err != nil {
			return err
		}
		if f.Kind != mcpav1.OauthFlowKind_OAUTH_FLOW_KIND_AUTHORIZATION_CODE {
			return apperr.FailedPrecondition("this flow completes through device authorization; poll it with GetProviderOAuthFlow")
		}
		if f.State != mcpav1.OauthFlowState_OAUTH_FLOW_STATE_PENDING {
			terminal = flowProto(f)
			return nil
		}
		if !now.Before(f.ExpireTime) {
			ferr = expired()
			return nil
		}
		if cbErr != nil {
			return cbErr
		}
		if sec, err = b.open(f, sealed); err != nil {
			return err
		}
		if subtle.ConstantTimeCompare([]byte(cb.state), []byte(sec.State)) != 1 {
			return apperr.InvalidInput("callback_url does not belong to this flow")
		}
		if cb.err != "" {
			code := "error"
			if oauthErrorCode.MatchString(cb.err) {
				code = cb.err
			}
			ferr = failed("the provider reported an authorization error: " + code)
			return nil
		}
		ok, err := b.claim(ctx, tx, f, &sec, now)
		if err != nil {
			return err
		}
		if !ok {
			return apperr.FailedPrecondition("this flow is already being completed")
		}
		return nil
	})
	if err != nil {
		return nil, dbErr(err)
	}
	if terminal != nil {
		return terminal, nil
	}
	if ferr != nil {
		return b.finish(ctx, f, ferr)
	}
	tok, err := providers[f.Provider].exchange(ctx, b, cb.code, cb.state, sec)
	if err != nil {
		return b.fail(ctx, f, err)
	}
	return b.register(ctx, p, f, tok)
}

// fail ends a flow after a provider error. Only flowError messages reach
// the caller; other causes are logged without bodies.
func (b *Broker) fail(ctx context.Context, f store.OAuthFlow, err error) (*mcpav1.ProviderOAuthFlow, error) {
	var ferr *flowError
	if !errors.As(err, &ferr) {
		b.log.WarnContext(ctx, "provider OAuth exchange failed", "resource_id", f.ResourceID, "flow_id", f.ID, "provider", f.Provider.String(), "error", err)
		ferr = failed("the provider did not issue tokens; start a new flow")
	}
	return b.finish(ctx, f, ferr)
}

// register stores the credential encrypted, writes the audit event and
// completes the flow in one transaction, then notifies the data plane.
func (b *Broker) register(ctx context.Context, p *auth.Principal, f store.OAuthFlow, tok token) (*mcpav1.ProviderOAuthFlow, error) {
	payload, err := json.Marshal(tok.payload)
	if err != nil {
		return nil, apperr.Internal(err)
	}
	defer clear(payload)
	c := store.Credential{
		ID: store.NewID(), ResourceID: f.ResourceID, TenantID: f.TenantID, Provider: f.Provider,
		Kind: mcpav1.CredentialKind_CREDENTIAL_KIND_OAUTH, Name: f.Name, AccountLabel: store.TruncateUTF8(tok.label, 200),
		CreatedByUserID: p.UserID,
	}
	if err := proxy.ValidateCredential(proxy.Credential{
		ID: c.ID.String(), ResourceID: c.ResourceID.String(), Provider: c.Provider, Kind: c.Kind, Label: c.AccountLabel, Secret: payload,
	}); err != nil {
		b.log.ErrorContext(ctx, "provider token payload is not usable by the engine", "flow_id", f.ID, "provider", f.Provider.String(), "error", err)
		return b.finish(ctx, f, failed("the provider account could not be connected"))
	}
	sealed, err := credential.Seal(b.keys, c, credential.Payload{Kind: credential.KindOAuth, OAuth: payload})
	if err != nil {
		return nil, apperr.Internal(err)
	}
	var out store.OAuthFlow
	registered := false
	err = b.db.InTx(ctx, func(tx pgx.Tx) error {
		cur, _, err := lockLive(ctx, tx, f.ResourceID, f.ID)
		if err != nil {
			return err
		}
		if cur.State != mcpav1.OauthFlowState_OAUTH_FLOW_STATE_PENDING {
			out = cur
			return nil
		}
		if c, err = store.InsertCredential(ctx, tx, c, sealed); err != nil {
			return err
		}
		if err := service.WriteAudit(ctx, tx, p, service.AuditInput{
			TenantID: f.TenantID, ResourceID: f.ResourceID, Action: mcpav1.AuditAction_AUDIT_ACTION_CREDENTIAL_REGISTERED,
			TargetID: c.ID.String(), Details: map[string]string{"provider": c.Provider.String(), "kind": c.Kind.String(), "flow_id": f.ID.String()},
		}); err != nil {
			return err
		}
		if out, err = store.FinishOAuthFlow(ctx, tx, f.ID, mcpav1.OauthFlowState_OAUTH_FLOW_STATE_COMPLETED, c.ID, ""); err != nil {
			return err
		}
		if b.listener != nil {
			if err := b.listener.CredentialsChanging(ctx, tx, c.ResourceID); err != nil {
				return err
			}
		}
		registered = true
		return nil
	})
	if err != nil {
		return nil, dbErr(err)
	}
	if registered {
		if b.listener != nil {
			b.listener.CredentialRegistered(ctx, c.ResourceID, c.ID)
		}
		b.log.InfoContext(ctx, "provider OAuth credential registered", "resource_id", c.ResourceID, "credential_id", c.ID, "flow_id", f.ID, "provider", c.Provider.String())
	}
	return flowProto(out), nil
}

// GetProviderOAuthFlow returns a flow. A pending device flow polls the
// provider when its poll interval has passed.
func (b *Broker) GetProviderOAuthFlow(ctx context.Context, p *auth.Principal, res store.Resource, req *mcpav1.GetProviderOAuthFlowRequest) (*mcpav1.ProviderOAuthFlow, error) {
	flowID, err := uuid.Parse(req.GetFlowId())
	if err != nil {
		return nil, apperr.InvalidInput("flow_id must be a canonical UUID")
	}
	f, err := store.GetOAuthFlow(ctx, b.db.Pool, res.ID, flowID)
	if errors.Is(err, store.ErrNotFound) {
		return nil, apperr.NotFound("OAuth flow not found")
	}
	if err != nil {
		return nil, dbErr(err)
	}
	if f.State != mcpav1.OauthFlowState_OAUTH_FLOW_STATE_PENDING {
		return flowProto(f), nil
	}
	now := b.now()
	if !now.Before(f.ExpireTime) {
		return b.finish(ctx, f, expired())
	}
	if f.Kind != mcpav1.OauthFlowKind_OAUTH_FLOW_KIND_DEVICE_CODE {
		return flowProto(f), nil
	}
	var sec flowSecret
	polling := false
	err = b.db.InTx(ctx, func(tx pgx.Tx) error {
		cur, sealed, err := lockLive(ctx, tx, res.ID, flowID)
		if err != nil {
			return err
		}
		f = cur
		if f.State != mcpav1.OauthFlowState_OAUTH_FLOW_STATE_PENDING {
			return nil
		}
		if sec, err = b.open(f, sealed); err != nil {
			return err
		}
		if now.UnixMilli() < sec.NextPollUnixMilli {
			return nil
		}
		sec.NextPollUnixMilli = now.Add(time.Duration(max(sec.IntervalSeconds, 1)) * time.Second).UnixMilli()
		polling, err = b.claim(ctx, tx, f, &sec, now)
		return err
	})
	if err != nil {
		return nil, dbErr(err)
	}
	if !polling {
		return flowProto(f), nil
	}
	tok, err := providers[f.Provider].poll(ctx, b, &sec)
	if err != nil {
		return b.fail(ctx, f, err)
	}
	if tok == nil {
		// Still pending: release the claim and keep any interval change.
		sec.ClaimedUntilUnixMilli = 0
		sealed, err := b.seal(f, sec)
		if err != nil {
			return nil, apperr.Internal(err)
		}
		if err := store.UpdateOAuthFlowSecret(ctx, b.db.Pool, f.ID, sealed); err != nil {
			return nil, dbErr(err)
		}
		return flowProto(f), nil
	}
	return b.register(ctx, p, f, *tok)
}

// clientConfigured reports whether the provider's OAuth client is available.
// Providers with public SDK client IDs need no operator configuration.
func (b *Broker) clientConfigured(p mcpav1.Provider) bool {
	switch p {
	case mcpav1.Provider_PROVIDER_GEMINI_CLI:
		return b.gemini.configured()
	case mcpav1.Provider_PROVIDER_ANTIGRAVITY:
		return b.antigravity.configured()
	default:
		return true
	}
}
