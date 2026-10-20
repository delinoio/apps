// SPDX-License-Identifier: FSL-1.1-ALv2

// Package delcore wraps the Delcore Connect API. DevBase never reads Delcore's database.
package delcore

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/protos/delcore/v1/delcorev1connect"
)

// State is the billing-independent tenant state reported by Delcore.
type State int

const (
	StateUnknown State = iota
	StateActive
	StateBlocked
	StateDeleting
	StateDeleted
)

func (s State) String() string {
	return [...]string{"unknown", "active", "blocked", "deleting", "deleted"}[s]
}

var (
	// ErrUnauthenticated reports an invalid or expired login token.
	ErrUnauthenticated = errors.New("delino authentication failed")
	// ErrForbidden reports that the caller is not a current tenant Owner or Admin.
	ErrForbidden = errors.New("tenant owner or administrator authority is required")
	// ErrUnavailable reports any dependency failure. Callers must deny the operation.
	ErrUnavailable = errors.New("delcore unavailable")
)

// Authority is the set of Delcore checks DevBase depends on.
type Authority interface {
	// Identify verifies the login token and returns the Delcore user ID.
	Identify(ctx context.Context, authorization string) (string, error)
	// RequireManager succeeds only for a current Owner or Admin of the tenant.
	RequireManager(ctx context.Context, authorization, tenantID string) error
	// TenantStates returns one state per requested tenant, UNKNOWN when unlisted.
	TenantStates(ctx context.Context, tenantIDs []string) (map[string]State, error)
}

// TokenSource returns a service access token with the authorize scope.
type TokenSource func(context.Context) (string, error)

// Client implements Authority with Delcore's generated Connect client.
type Client struct {
	RPC     delcorev1connect.DelcoreServiceClient
	Token   TokenSource
	Timeout time.Duration
}

func (c *Client) bounded(ctx context.Context) (context.Context, context.CancelFunc) {
	return context.WithTimeout(ctx, c.Timeout)
}

// classify maps a Delcore failure to a stable category. Anything that is not an
// explicit authentication or permission answer denies as unavailable.
func classify(e error) error {
	switch connect.CodeOf(e) {
	case connect.CodeUnauthenticated:
		return ErrUnauthenticated
	case connect.CodePermissionDenied:
		return ErrForbidden
	}
	return ErrUnavailable
}

func (c *Client) Identify(ctx context.Context, authorization string) (string, error) {
	ctx, cancel := c.bounded(ctx)
	defer cancel()
	q := connect.NewRequest(&pb.GetCurrentUserRequest{Page: &pb.Page{Size: 1}})
	q.Header().Set("Authorization", authorization)
	r, e := c.RPC.GetCurrentUser(ctx, q)
	if e != nil {
		return "", classify(e)
	}
	if r.Msg.UserId == "" {
		return "", ErrUnavailable
	}
	return r.Msg.UserId, nil
}

func (c *Client) RequireManager(ctx context.Context, authorization, tenantID string) error {
	ctx, cancel := c.bounded(ctx)
	defer cancel()
	q := connect.NewRequest(&pb.ListMembersRequest{TenantId: tenantID, Page: &pb.Page{Size: 1}})
	q.Header().Set("Authorization", authorization)
	if _, e := c.RPC.ListMembers(ctx, q); e != nil {
		return classify(e)
	}
	return nil
}

func (c *Client) TenantStates(ctx context.Context, tenantIDs []string) (map[string]State, error) {
	ctx, cancel := c.bounded(ctx)
	defer cancel()
	token, e := c.Token(ctx)
	if e != nil {
		return nil, ErrUnavailable
	}
	q := connect.NewRequest(&pb.GetTenantStatesRequest{TenantIds: tenantIDs})
	q.Header().Set("Authorization", "Bearer "+token)
	r, e := c.RPC.GetTenantStates(ctx, q)
	if e != nil {
		return nil, ErrUnavailable
	}
	// Delcore answers one entry per requested ID, in request order. Any other shape is a
	// malformed answer: a duplicate or reordered entry could otherwise overwrite a
	// DELETED state with ACTIVE, so it fails closed as unavailable.
	if len(r.Msg.States) != len(tenantIDs) {
		return nil, ErrUnavailable
	}
	out := make(map[string]State, len(tenantIDs))
	for i, entry := range r.Msg.States {
		if entry == nil || entry.TenantId != tenantIDs[i] || entry.State == pb.TenantState_TENANT_STATE_UNSPECIFIED {
			return nil, ErrUnavailable
		}
		if _, dup := out[entry.TenantId]; dup {
			return nil, ErrUnavailable
		}
		out[entry.TenantId] = fromProto(entry.State)
	}
	return out, nil
}

func fromProto(s pb.TenantState) State {
	switch s {
	case pb.TenantState_TENANT_STATE_ACTIVE:
		return StateActive
	case pb.TenantState_TENANT_STATE_BLOCKED:
		return StateBlocked
	case pb.TenantState_TENANT_STATE_DELETING:
		return StateDeleting
	case pb.TenantState_TENANT_STATE_DELETED:
		return StateDeleted
	}
	return StateUnknown
}

// ClientCredentials fetches and caches a Logto client-credentials access token.
type ClientCredentials struct {
	HTTP     *http.Client
	URL      string
	Resource string
	ID       string
	Secret   string
	Scope    string
	Now      func() time.Time

	mu      sync.Mutex
	token   string
	expires time.Time
}

// Token implements TokenSource. The token is reused until shortly before expiry.
func (t *ClientCredentials) Token(ctx context.Context) (string, error) {
	t.mu.Lock()
	defer t.mu.Unlock()
	now := time.Now
	if t.Now != nil {
		now = t.Now
	}
	if t.token != "" && now().Before(t.expires) {
		return t.token, nil
	}
	form := url.Values{"grant_type": {"client_credentials"}, "resource": {t.Resource}, "scope": {t.Scope}}
	req, e := http.NewRequestWithContext(ctx, http.MethodPost, t.URL, strings.NewReader(form.Encode()))
	if e != nil {
		return "", e
	}
	req.SetBasicAuth(t.ID, t.Secret)
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	resp, e := t.HTTP.Do(req)
	if e != nil {
		return "", errors.New("service token unavailable")
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return "", errors.New("service token rejected")
	}
	var body struct {
		Token   string `json:"access_token"`
		Expires int64  `json:"expires_in"`
	}
	if e = json.NewDecoder(http.MaxBytesReader(nil, resp.Body, 64<<10)).Decode(&body); e != nil || body.Token == "" {
		return "", errors.New("service token malformed")
	}
	lifetime := time.Duration(body.Expires) * time.Second
	if lifetime <= time.Minute {
		// Short or unspecified lifetimes are not cached beyond this request window.
		t.token, t.expires = "", time.Time{}
		return body.Token, nil
	}
	t.token, t.expires = body.Token, now().Add(lifetime-30*time.Second)
	return t.token, nil
}
