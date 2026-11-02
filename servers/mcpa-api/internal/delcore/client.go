// SPDX-License-Identifier: FSL-1.1-ALv2

// Package delcore is the mcpa client of the Delcore Connect API.
//
// Machine-to-machine calls use a cached Logto client-credentials token for
// the Delcore resource. User-token calls (GetCurrentUser, ListGroups) use a
// Delcore-audience token from the user's own Logto login and never fall back
// to the service identity. Every failure fails closed: only allowed
// authorization results and ACTIVE tenant states are cached, for the
// configured TTL, and Delcore errors are returned as errors after expiry.
package delcore

import (
	"context"
	"crypto/sha256"
	"errors"
	"fmt"
	"net/http"
	"strconv"
	"sync"
	"time"

	"connectrpc.com/connect"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	"golang.org/x/oauth2"
	"golang.org/x/oauth2/clientcredentials"
	"google.golang.org/protobuf/types/known/timestamppb"

	delcorev1 "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/protos/delcore/v1/delcorev1connect"
	"github.com/delinoio/apps/servers/mcpa-api/internal/obs"
	"github.com/delinoio/apps/servers/mcpa-api/internal/pricing"
)

// Capability is a Delcore capability declared by the mcpa service.
type Capability string

// Stable capability names registered with PutService.
const (
	CapabilityCreateResources    Capability = "create_resources"
	CapabilityAccessResources    Capability = "access_resources"
	CapabilityManageTenantBudget Capability = "manage_tenant_budget"
	CapabilityInvokeModels       Capability = "invoke_models"
)

// M2MScopes are the Logto scopes requested for the service token.
const M2MScopes = "authorize usage:write usage:read"

// pageSize is the page size of paginated Delcore reads.
const pageSize = 100

// maxPages bounds paginated reads so a misbehaving server cannot loop us.
const maxPages = 1000

// Config configures the client.
type Config struct {
	BaseURL      string
	TokenURL     string
	ClientID     string
	ClientSecret string
	// Resource is the Logto resource indicator of the Delcore API.
	Resource string
	// ServiceID is the Delcore service ID of mcpa.
	ServiceID string
	// Timeout bounds each Delcore call, including token requests.
	Timeout time.Duration
	// CacheTTL is how long allowed results and ACTIVE states are cached.
	CacheTTL time.Duration
	// HTTPClient defaults to a client with Timeout.
	HTTPClient *http.Client
	Metrics    *obs.Metrics
	// Now defaults to time.Now. Tests replace it to expire cache entries.
	Now func() time.Time
	// TokenSource overrides the client-credentials source in tests.
	TokenSource oauth2.TokenSource
}

// Client calls Delcore.
type Client struct {
	m2m       delcorev1connect.DelcoreServiceClient
	user      delcorev1connect.DelcoreServiceClient
	serviceID string
	timeout   time.Duration
	ttl       time.Duration
	metrics   *obs.Metrics
	now       func() time.Time

	authorizeCache *ttlCache[Decision]
	keyCache       *ttlCache[KeyDecision]
	stateCache     *ttlCache[struct{}]
	periodCache    *ttlCache[Period]
}

// New builds a client. It does not contact Delcore or Logto.
func New(cfg Config) (*Client, error) {
	if cfg.BaseURL == "" {
		return nil, errors.New("delcore: base URL is required")
	}
	if cfg.Timeout <= 0 {
		cfg.Timeout = 5 * time.Second
	}
	if cfg.CacheTTL <= 0 {
		return nil, errors.New("delcore: cache TTL must be positive")
	}
	if cfg.Now == nil {
		cfg.Now = time.Now
	}
	httpClient := cfg.HTTPClient
	if httpClient == nil {
		httpClient = &http.Client{Timeout: cfg.Timeout}
	}
	tokens := cfg.TokenSource
	if tokens == nil {
		cc := clientcredentials.Config{
			ClientID:       cfg.ClientID,
			ClientSecret:   cfg.ClientSecret,
			TokenURL:       cfg.TokenURL,
			Scopes:         []string{M2MScopes},
			EndpointParams: map[string][]string{"resource": {cfg.Resource}},
			AuthStyle:      oauth2.AuthStyleInHeader,
		}
		// The token source caches the token until shortly before expiry.
		tokens = cc.TokenSource(context.WithValue(context.Background(), oauth2.HTTPClient, httpClient))
	}
	c := &Client{
		serviceID:      cfg.ServiceID,
		timeout:        cfg.Timeout,
		ttl:            cfg.CacheTTL,
		metrics:        cfg.Metrics,
		now:            cfg.Now,
		authorizeCache: newTTLCache[Decision](cfg.Now),
		keyCache:       newTTLCache[KeyDecision](cfg.Now),
		stateCache:     newTTLCache[struct{}](cfg.Now),
		periodCache:    newTTLCache[Period](cfg.Now),
	}
	trace := connect.WithInterceptors(traceInterceptor())
	c.m2m = delcorev1connect.NewDelcoreServiceClient(httpClient, cfg.BaseURL, trace, connect.WithInterceptors(m2mInterceptor(tokens)))
	c.user = delcorev1connect.NewDelcoreServiceClient(httpClient, cfg.BaseURL, trace)
	return c, nil
}

func m2mInterceptor(tokens oauth2.TokenSource) connect.UnaryInterceptorFunc {
	return func(next connect.UnaryFunc) connect.UnaryFunc {
		return func(ctx context.Context, req connect.AnyRequest) (connect.AnyResponse, error) {
			tok, err := tokens.Token()
			if err != nil {
				return nil, &Error{Kind: KindUnavailable, RPC: req.Spec().Procedure, Cause: fmt.Errorf("service token: %w", err)}
			}
			req.Header().Set("Authorization", "Bearer "+tok.AccessToken)
			return next(ctx, req)
		}
	}
}

func traceInterceptor() connect.UnaryInterceptorFunc {
	return func(next connect.UnaryFunc) connect.UnaryFunc {
		return func(ctx context.Context, req connect.AnyRequest) (connect.AnyResponse, error) {
			ctx, span := obs.Tracer().Start(ctx, "delcore "+req.Spec().Procedure)
			defer span.End()
			resp, err := next(ctx, req)
			if err != nil {
				span.SetStatus(codes.Error, "delcore call failed")
				span.SetAttributes(attribute.String("rpc.connect.code", connect.CodeOf(err).String()))
			}
			return resp, err
		}
	}
}

// Kind classifies a Delcore failure.
type Kind int

// Failure kinds.
const (
	// KindUnavailable means Delcore or Logto could not be reached or timed out.
	KindUnavailable Kind = iota + 1
	// KindUnauthenticated means Delcore rejected the presented credential.
	KindUnauthenticated
	// KindRejected means Delcore answered with another error.
	KindRejected
)

// Error is a failed Delcore call.
type Error struct {
	Kind Kind
	RPC  string
	Code connect.Code
	// Reason is the Delcore ErrorDetail reason when present.
	Reason delcorev1.ErrorReason
	Cause  error
}

func (e *Error) Error() string {
	return fmt.Sprintf("delcore %s: %v", e.RPC, e.Cause)
}

func (e *Error) Unwrap() error { return e.Cause }

// ReasonOf returns the Delcore error reason of err, if any.
func ReasonOf(err error) delcorev1.ErrorReason {
	var e *Error
	if errors.As(err, &e) {
		return e.Reason
	}
	return delcorev1.ErrorReason_ERROR_REASON_UNSPECIFIED
}

// IsUnauthenticated reports whether Delcore rejected the credential.
func IsUnauthenticated(err error) bool {
	var e *Error
	return errors.As(err, &e) && e.Kind == KindUnauthenticated
}

func (c *Client) fail(rpc string, err error) error {
	var de *Error
	if errors.As(err, &de) {
		c.metrics.RecordDelcoreFailure(rpc, obs.DelcoreUnavailable)
		de.RPC = rpc
		return de
	}
	out := &Error{RPC: rpc, Cause: err, Code: connect.CodeOf(err)}
	var ce *connect.Error
	if errors.As(err, &ce) {
		for _, d := range ce.Details() {
			if v, derr := d.Value(); derr == nil {
				if detail, ok := v.(*delcorev1.ErrorDetail); ok {
					out.Reason = detail.GetReason()
				}
			}
		}
	}
	switch out.Code {
	case connect.CodeUnavailable, connect.CodeDeadlineExceeded, connect.CodeUnknown, connect.CodeCanceled, connect.CodeResourceExhausted, connect.CodeInternal:
		out.Kind = KindUnavailable
		c.metrics.RecordDelcoreFailure(rpc, obs.DelcoreUnavailable)
	case connect.CodeUnauthenticated:
		out.Kind = KindUnauthenticated
		c.metrics.RecordDelcoreFailure(rpc, obs.DelcoreError)
	default:
		out.Kind = KindRejected
		c.metrics.RecordDelcoreFailure(rpc, obs.DelcoreError)
	}
	return out
}

func (c *Client) callCtx(ctx context.Context) (context.Context, context.CancelFunc) {
	return context.WithTimeout(ctx, c.timeout)
}

func userRequest[T any](msg *T, token string) (*connect.Request[T], error) {
	if token == "" {
		return nil, errors.New("delcore: a user token is required")
	}
	req := connect.NewRequest(msg)
	req.Header().Set("Authorization", "Bearer "+token)
	return req, nil
}

// Decision is an authorization result.
type Decision struct {
	Allowed bool
	Reasons []delcorev1.BlockReason
}

func boolKey(b bool) string { return strconv.FormatBool(b) }

// Authorize checks a user's capability in a tenant with the service
// identity. Allowed results are cached for the TTL; denials and errors are
// never cached.
func (c *Client) Authorize(ctx context.Context, tenantID, userID string, capability Capability, paid bool) (Decision, error) {
	key := tenantID + "\x00" + userID + "\x00" + string(capability) + "\x00" + boolKey(paid)
	if d, ok := c.authorizeCache.get(key); ok {
		return d, nil
	}
	ctx, cancel := c.callCtx(ctx)
	defer cancel()
	resp, err := c.m2m.Authorize(ctx, connect.NewRequest(&delcorev1.AuthorizeRequest{
		TenantId: tenantID, UserId: userID, Capability: string(capability), Paid: paid,
	}))
	if err != nil {
		return Decision{}, c.fail("Authorize", err)
	}
	d := Decision{Allowed: resp.Msg.GetAllowed(), Reasons: resp.Msg.GetReasons()}
	if d.Allowed {
		c.authorizeCache.put(key, d, c.ttl)
	} else {
		c.metrics.RecordDelcoreFailure("Authorize", obs.DelcoreDenied)
	}
	return d, nil
}

// KeyDecision is the result of AuthorizeApiKey for a valid secret.
type KeyDecision struct {
	Allowed   bool
	Reasons   []delcorev1.BlockReason
	KeyID     string
	TenantID  string
	OwnerKind delcorev1.ApiKeyOwnerKind
	UserID    string
}

// AuthorizeAPIKey verifies a presented Delcore API key secret. An invalid
// secret returns an error for which IsUnauthenticated is true. Allowed
// results are cached by the SHA-256 digest of the secret; the secret itself
// is never stored.
func (c *Client) AuthorizeAPIKey(ctx context.Context, secret string, capability Capability, paid bool) (KeyDecision, error) {
	digest := sha256.Sum256([]byte(secret))
	key := string(digest[:]) + "\x00" + string(capability) + "\x00" + boolKey(paid)
	if d, ok := c.keyCache.get(key); ok {
		return d, nil
	}
	ctx, cancel := c.callCtx(ctx)
	defer cancel()
	resp, err := c.m2m.AuthorizeApiKey(ctx, connect.NewRequest(&delcorev1.AuthorizeApiKeyRequest{
		Secret: secret, Capability: string(capability), Paid: paid,
	}))
	if err != nil {
		return KeyDecision{}, c.fail("AuthorizeApiKey", err)
	}
	m := resp.Msg
	d := KeyDecision{
		Allowed: m.GetAllowed(), Reasons: m.GetReasons(), KeyID: m.GetKeyId(),
		TenantID: m.GetTenantId(), OwnerKind: m.GetOwnerKind(), UserID: m.GetUserId(),
	}
	if d.Allowed {
		c.keyCache.put(key, d, c.ttl)
	} else {
		c.metrics.RecordDelcoreFailure("AuthorizeApiKey", obs.DelcoreDenied)
	}
	return d, nil
}

// TenantState returns the lifecycle state of one tenant. Only ACTIVE is
// cached; any other state or error is returned uncached.
func (c *Client) TenantState(ctx context.Context, tenantID string) (delcorev1.TenantState, error) {
	if _, ok := c.stateCache.get(tenantID); ok {
		return delcorev1.TenantState_TENANT_STATE_ACTIVE, nil
	}
	states, err := c.GetTenantStates(ctx, []string{tenantID})
	if err != nil {
		return delcorev1.TenantState_TENANT_STATE_UNSPECIFIED, err
	}
	state := states[tenantID]
	if state == delcorev1.TenantState_TENANT_STATE_ACTIVE {
		c.stateCache.put(tenantID, struct{}{}, c.ttl)
	}
	return state, nil
}

// GetTenantStates returns the state of each requested tenant. A missing
// entry maps to TENANT_STATE_UNSPECIFIED, which callers treat as inactive.
func (c *Client) GetTenantStates(ctx context.Context, tenantIDs []string) (map[string]delcorev1.TenantState, error) {
	ctx, cancel := c.callCtx(ctx)
	defer cancel()
	resp, err := c.m2m.GetTenantStates(ctx, connect.NewRequest(&delcorev1.GetTenantStatesRequest{TenantIds: tenantIDs}))
	if err != nil {
		return nil, c.fail("GetTenantStates", err)
	}
	out := make(map[string]delcorev1.TenantState, len(tenantIDs))
	for _, e := range resp.Msg.GetStates() {
		out[e.GetTenantId()] = e.GetState()
	}
	return out, nil
}

// CheckTenantLifecycle returns the IDs whose deletion Delcore finalized.
// Callers may pass at most 100 IDs.
func (c *Client) CheckTenantLifecycle(ctx context.Context, tenantIDs []string) ([]string, error) {
	ctx, cancel := c.callCtx(ctx)
	defer cancel()
	resp, err := c.m2m.CheckTenantLifecycle(ctx, connect.NewRequest(&delcorev1.CheckTenantLifecycleRequest{TenantIds: tenantIDs}))
	if err != nil {
		return nil, c.fail("CheckTenantLifecycle", err)
	}
	return resp.Msg.GetFinalizedTenantIds(), nil
}

// Period is a tenant billing period.
type Period struct {
	Start time.Time
	End   time.Time
}

// BillingPeriod returns the tenant's current billing period from the mcpa
// traffic meter. Successful results are cached for the TTL.
func (c *Client) BillingPeriod(ctx context.Context, tenantID string) (Period, error) {
	if p, ok := c.periodCache.get(tenantID); ok && c.now().Before(p.End) {
		return p, nil
	}
	resp, err := c.ListMeterAggregates(ctx, &delcorev1.ListMeterAggregatesRequest{
		TenantId: tenantID, MeterKey: pricing.MeterKey, Page: &delcorev1.Page{Size: 1},
	})
	if err != nil {
		return Period{}, err
	}
	if resp.GetPeriodStart() == nil || resp.GetPeriodEnd() == nil {
		return Period{}, c.fail("ListMeterAggregates", connect.NewError(connect.CodeFailedPrecondition, errors.New("billing period bounds are missing")))
	}
	p := Period{Start: resp.GetPeriodStart().AsTime(), End: resp.GetPeriodEnd().AsTime()}
	c.periodCache.put(tenantID, p, c.ttl)
	return p, nil
}

// ListMeterAggregates reads one page of mcpa meter aggregates. ServiceId
// defaults to the configured mcpa service ID.
func (c *Client) ListMeterAggregates(ctx context.Context, req *delcorev1.ListMeterAggregatesRequest) (*delcorev1.ListMeterAggregatesResponse, error) {
	if req.GetServiceId() == "" {
		req.ServiceId = c.serviceID
	}
	ctx, cancel := c.callCtx(ctx)
	defer cancel()
	resp, err := c.m2m.ListMeterAggregates(ctx, connect.NewRequest(req))
	if err != nil {
		return nil, c.fail("ListMeterAggregates", err)
	}
	return resp.Msg, nil
}

// IngestEvent reports one measured event. ServiceId defaults to the
// configured mcpa service ID. Identical replays with the same event key are
// safe; use ReasonOf to classify failures.
func (c *Client) IngestEvent(ctx context.Context, requestKey string, event *delcorev1.Event) (*delcorev1.IngestEventResponse, error) {
	if event.GetServiceId() == "" {
		event.ServiceId = c.serviceID
	}
	ctx, cancel := c.callCtx(ctx)
	defer cancel()
	resp, err := c.m2m.IngestEvent(ctx, connect.NewRequest(&delcorev1.IngestEventRequest{RequestKey: requestKey, Event: event}))
	if err != nil {
		return nil, c.fail("IngestEvent", err)
	}
	return resp.Msg, nil
}

// CurrentUser is the Delcore identity of a user token.
type CurrentUser struct {
	UserID  string
	Tenants []*delcorev1.Tenant
}

// GetCurrentUser reads the user's identity and all tenant pages with a
// Delcore-audience user token.
func (c *Client) GetCurrentUser(ctx context.Context, userToken string) (CurrentUser, error) {
	var out CurrentUser
	after := ""
	for range maxPages {
		req, err := userRequest(&delcorev1.GetCurrentUserRequest{Page: &delcorev1.Page{Size: pageSize, AfterId: after}}, userToken)
		if err != nil {
			return CurrentUser{}, err
		}
		callCtx, cancel := c.callCtx(ctx)
		resp, err := c.user.GetCurrentUser(callCtx, req)
		cancel()
		if err != nil {
			return CurrentUser{}, c.fail("GetCurrentUser", err)
		}
		if out.UserID == "" {
			out.UserID = resp.Msg.GetUserId()
		}
		out.Tenants = append(out.Tenants, resp.Msg.GetTenants()...)
		after = resp.Msg.GetNextAfterId()
		if after == "" {
			if out.UserID == "" {
				return CurrentUser{}, c.fail("GetCurrentUser", connect.NewError(connect.CodeInternal, errors.New("empty user ID")))
			}
			return out, nil
		}
	}
	return CurrentUser{}, c.fail("GetCurrentUser", connect.NewError(connect.CodeInternal, errors.New("too many pages")))
}

// ListGroups reads every group of a tenant, with member user IDs, using a
// Delcore-audience user token of a tenant member.
func (c *Client) ListGroups(ctx context.Context, userToken, tenantID string) ([]*delcorev1.Group, error) {
	var out []*delcorev1.Group
	after := ""
	for range maxPages {
		req, err := userRequest(&delcorev1.ListGroupsRequest{TenantId: tenantID, Page: &delcorev1.Page{Size: pageSize, AfterId: after}}, userToken)
		if err != nil {
			return nil, err
		}
		callCtx, cancel := c.callCtx(ctx)
		resp, err := c.user.ListGroups(callCtx, req)
		cancel()
		if err != nil {
			return nil, c.fail("ListGroups", err)
		}
		out = append(out, resp.Msg.GetGroups()...)
		after = resp.Msg.GetNextAfterId()
		if after == "" {
			return out, nil
		}
	}
	return nil, c.fail("ListGroups", connect.NewError(connect.CodeInternal, errors.New("too many pages")))
}

// Timestamp converts t for Delcore events.
func Timestamp(t time.Time) *timestamppb.Timestamp { return timestamppb.New(t) }

// ttlCache is a small expiring map.
type ttlCache[V any] struct {
	mu      sync.Mutex
	now     func() time.Time
	entries map[string]ttlEntry[V]
}

type ttlEntry[V any] struct {
	value  V
	expiry time.Time
}

// maxCacheEntries bounds memory; expired entries are pruned first.
const maxCacheEntries = 100_000

func newTTLCache[V any](now func() time.Time) *ttlCache[V] {
	return &ttlCache[V]{now: now, entries: map[string]ttlEntry[V]{}}
}

func (c *ttlCache[V]) get(key string) (V, bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	e, ok := c.entries[key]
	if !ok {
		var zero V
		return zero, false
	}
	if !c.now().Before(e.expiry) {
		delete(c.entries, key)
		var zero V
		return zero, false
	}
	return e.value, true
}

func (c *ttlCache[V]) put(key string, v V, ttl time.Duration) {
	c.mu.Lock()
	defer c.mu.Unlock()
	now := c.now()
	if len(c.entries) >= maxCacheEntries {
		for k, e := range c.entries {
			if !now.Before(e.expiry) {
				delete(c.entries, k)
			}
		}
		if len(c.entries) >= maxCacheEntries {
			// Still full of live entries: start over rather than grow unbounded.
			c.entries = map[string]ttlEntry[V]{}
		}
	}
	c.entries[key] = ttlEntry[V]{value: v, expiry: now.Add(ttl)}
}

// Invalidate drops cached results of a tenant's states. It is used when a
// caller learns that a tenant changed state.
func (c *Client) Invalidate(tenantID string) {
	c.stateCache.mu.Lock()
	delete(c.stateCache.entries, tenantID)
	c.stateCache.mu.Unlock()
}
