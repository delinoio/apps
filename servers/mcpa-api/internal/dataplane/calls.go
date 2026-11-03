// SPDX-License-Identifier: FSL-1.1-ALv2

package dataplane

import (
	"context"
	"errors"
	"log/slog"
	"math/big"
	"slices"
	"sync"
	"time"

	"github.com/google/uuid"

	delcorev1 "github.com/delinoio/apps/protos/delcore/v1"
	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/authz"
	"github.com/delinoio/apps/servers/mcpa-api/internal/callkey"
	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
	"github.com/delinoio/apps/servers/mcpa-api/internal/pricing"
	"github.com/delinoio/apps/servers/mcpa-api/internal/proxy"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

// lastUsedInterval is the minimum time between last_used_time updates of a
// key.
const lastUsedInterval = time.Minute

func invalidKey() error {
	return proxy.Reject(proxy.RejectionInvalidCallKey, "the call key is not valid for this resource")
}

// KeyAuthenticator verifies call keys on the model-call path.
type KeyAuthenticator struct {
	db  *store.Store
	log *slog.Logger
	now func() time.Time

	mu      sync.Mutex
	touched map[uuid.UUID]time.Time
}

var _ proxy.KeyAuthenticator = (*KeyAuthenticator)(nil)

// NewKeyAuthenticator builds the call-key verifier.
func NewKeyAuthenticator(db *store.Store, log *slog.Logger, now func() time.Time) *KeyAuthenticator {
	return &KeyAuthenticator{db: db, log: log, now: now, touched: map[uuid.UUID]time.Time{}}
}

// AuthenticateCallKey resolves a presented call key. A key is valid only
// when its digest matches in constant time, it is neither revoked nor
// expired, it belongs to the path resource, the resource is live and the
// issuer still holds INVOKE or MANAGE (directly or through a group of the
// tenant's latest snapshot). Database failures return a plain error, which
// the engine reports as dependency_unavailable.
func (k *KeyAuthenticator) AuthenticateCallKey(ctx context.Context, resourceID, secret string) (proxy.CallPrincipal, error) {
	keyID, err := callkey.Parse(secret)
	if err != nil {
		return proxy.CallPrincipal{}, invalidKey()
	}
	rid, err := uuid.Parse(resourceID)
	if err != nil || rid.String() != resourceID {
		return proxy.CallPrincipal{}, invalidKey()
	}
	key, err := store.LookupCallKey(ctx, k.db.Pool, keyID)
	if errors.Is(err, store.ErrNotFound) {
		return proxy.CallPrincipal{}, invalidKey()
	}
	if err != nil {
		return proxy.CallPrincipal{}, err
	}
	now := k.now()
	if !callkey.Verify(secret, key.SecretHash) || key.RevokeTime != nil ||
		(key.ExpireTime != nil && !now.Before(*key.ExpireTime)) || key.ResourceID != rid {
		return proxy.CallPrincipal{}, invalidKey()
	}
	res, err := store.GetResource(ctx, k.db.Pool, rid)
	if errors.Is(err, store.ErrNotFound) {
		return proxy.CallPrincipal{}, invalidKey()
	}
	if err != nil {
		return proxy.CallPrincipal{}, err
	}
	if res.Deleted() || res.TenantID != key.TenantID {
		return proxy.CallPrincipal{}, invalidKey()
	}
	groups, err := store.UserGroupIDs(ctx, k.db.Pool, res.TenantID, key.IssuerUserID)
	if err != nil {
		return proxy.CallPrincipal{}, err
	}
	entries, err := store.ListACL(ctx, k.db.Pool, rid)
	if err != nil {
		return proxy.CallPrincipal{}, err
	}
	if !authz.Satisfies(authz.Effective(entries, key.IssuerUserID, groups), mcpav1.ResourceRole_RESOURCE_ROLE_INVOKE) {
		return proxy.CallPrincipal{}, invalidKey()
	}
	k.touch(ctx, key.ID, now)
	return proxy.CallPrincipal{
		ResourceID: rid.String(), TenantID: res.TenantID, CallKeyID: key.ID.String(), IssuerUserID: key.IssuerUserID,
	}, nil
}

// touch updates last_used_time at most once per minute per key. The
// in-process throttle avoids a write per call; the SQL condition keeps the
// limit across replicas.
func (k *KeyAuthenticator) touch(ctx context.Context, id uuid.UUID, now time.Time) {
	k.mu.Lock()
	last, ok := k.touched[id]
	if ok && now.Sub(last) < lastUsedInterval {
		k.mu.Unlock()
		return
	}
	k.touched[id] = now
	if len(k.touched) > 100_000 {
		for kid, at := range k.touched {
			if now.Sub(at) >= lastUsedInterval {
				delete(k.touched, kid)
			}
		}
	}
	k.mu.Unlock()
	if err := store.TouchCallKey(ctx, k.db.Pool, id, now); err != nil {
		k.log.Warn("updating call key last use failed", "call_key_id", id, "error", err)
	}
}

// CallAuthorizer applies the tenant, billing and budget gates.
type CallAuthorizer struct {
	db       *store.Store
	dc       Delcore
	log      *slog.Logger
	now      func() time.Time
	spendTTL time.Duration

	mu    sync.Mutex
	spend map[spendKey]spendEntry
}

type spendKey struct {
	scope proxy.BudgetScope
	id    string
	start time.Time
}

type spendEntry struct {
	bytes  int64
	expiry time.Time
}

var _ proxy.CallAuthorizer = (*CallAuthorizer)(nil)

// NewCallAuthorizer builds the call gates. Measured spend is cached for
// spendTTL (the checkpoint interval): spend only grows when checkpoints
// commit, and budgets tolerate this small lag by contract.
func NewCallAuthorizer(db *store.Store, dc Delcore, log *slog.Logger, now func() time.Time, spendTTL time.Duration) *CallAuthorizer {
	return &CallAuthorizer{db: db, dc: dc, log: log, now: now, spendTTL: spendTTL, spend: map[spendKey]spendEntry{}}
}

func dependencyUnavailable() error {
	return proxy.Reject(proxy.RejectionDependencyUnavailable, "a required service is unavailable; retry later")
}

// AuthorizeCall requires an ACTIVE tenant and an allowed paid
// Authorize(invoke_models) for the key's issuer. Delcore results come from
// the allowed-only cache; errors fail closed. Budgets are checked separately
// by CheckBudgets, after the model allowlist.
func (a *CallAuthorizer) AuthorizeCall(ctx context.Context, p proxy.CallPrincipal) error {
	state, err := a.dc.TenantState(ctx, p.TenantID)
	if err != nil {
		a.log.Warn("tenant state check failed", "tenant_id", p.TenantID, "error", err)
		return dependencyUnavailable()
	}
	if state != delcorev1.TenantState_TENANT_STATE_ACTIVE {
		return proxy.Reject(proxy.RejectionTenantInactive, "the tenant cannot make model calls")
	}
	d, err := a.dc.Authorize(ctx, p.TenantID, p.IssuerUserID, delcore.CapabilityInvokeModels, true)
	if err != nil {
		a.log.Warn("invoke authorization failed", "tenant_id", p.TenantID, "error", err)
		return dependencyUnavailable()
	}
	if !d.Allowed {
		if slices.Contains(d.Reasons, delcorev1.BlockReason_BLOCK_REASON_PERMISSION) {
			// The issuer left the tenant or lost invoke_models.
			return invalidKey()
		}
		return proxy.Reject(proxy.RejectionTenantInactive, "the tenant cannot make paid model calls")
	}
	return nil
}

// CheckBudgets requires unexhausted resource and tenant budgets. Calls
// without a budget need no billing period; an unreadable budget, period or
// spend fails closed.
func (a *CallAuthorizer) CheckBudgets(ctx context.Context, p proxy.CallPrincipal) error {
	rid, err := uuid.Parse(p.ResourceID)
	if err != nil {
		return invalidKey()
	}
	resourceLimit, err := store.GetResourceBudgetLimit(ctx, a.db.Pool, rid)
	if err != nil {
		return dependencyUnavailable()
	}
	tenantLimit, err := store.GetTenantBudgetLimit(ctx, a.db.Pool, p.TenantID)
	if err != nil {
		return dependencyUnavailable()
	}
	if resourceLimit == "" && tenantLimit == "" {
		return nil
	}
	period, err := a.dc.BillingPeriod(ctx, p.TenantID)
	if err != nil {
		a.log.Warn("billing period read failed", "tenant_id", p.TenantID, "error", err)
		return dependencyUnavailable()
	}
	checks := []struct {
		scope proxy.BudgetScope
		limit string
		id    string
		rid   *uuid.UUID
	}{
		{proxy.BudgetScopeResource, resourceLimit, p.ResourceID, &rid},
		{proxy.BudgetScopeTenant, tenantLimit, p.TenantID, nil},
	}
	for _, c := range checks {
		if c.limit == "" {
			continue
		}
		limit, ok := new(big.Rat).SetString(c.limit)
		if !ok {
			return proxy.Reject(proxy.RejectionInternal, "the budget could not be read")
		}
		billed, err := a.billed(ctx, c.scope, c.id, p.TenantID, c.rid, period)
		if err != nil {
			return dependencyUnavailable()
		}
		if pricing.SpendUSD(billed).Cmp(limit) >= 0 {
			return &proxy.Rejection{Code: proxy.RejectionBudgetExhausted, Message: "the " + c.scope.String() + " budget is exhausted", BudgetScope: c.scope}
		}
	}
	return nil
}

func (a *CallAuthorizer) billed(ctx context.Context, scope proxy.BudgetScope, id, tenantID string, rid *uuid.UUID, period delcore.Period) (int64, error) {
	key := spendKey{scope: scope, id: id, start: period.Start}
	now := a.now()
	a.mu.Lock()
	if e, ok := a.spend[key]; ok && now.Before(e.expiry) {
		a.mu.Unlock()
		return e.bytes, nil
	}
	a.mu.Unlock()
	n, err := store.BilledBytes(ctx, a.db.Pool, tenantID, rid, period.Start, period.End)
	if err != nil {
		return 0, err
	}
	a.mu.Lock()
	if len(a.spend) > 100_000 {
		a.spend = map[spendKey]spendEntry{}
	}
	a.spend[key] = spendEntry{bytes: n, expiry: now.Add(a.spendTTL)}
	a.mu.Unlock()
	return n, nil
}

// Routing reads the model allowlist of a resource.
type Routing struct {
	db *store.Store
}

var _ proxy.RoutingSource = Routing{}

// ModelAllowlist returns the allowlist of a live resource.
func (r Routing) ModelAllowlist(ctx context.Context, resourceID string) ([]string, error) {
	id, err := uuid.Parse(resourceID)
	if err != nil {
		return nil, invalidKey()
	}
	res, err := store.GetResource(ctx, r.db.Pool, id)
	if errors.Is(err, store.ErrNotFound) {
		return nil, invalidKey()
	}
	if err != nil {
		return nil, err
	}
	if res.Deleted() {
		return nil, invalidKey()
	}
	return res.ModelAllowlist, nil
}

// MeterSink commits metering checkpoints to PostgreSQL.
type MeterSink struct {
	db *store.Store
}

var _ proxy.MeterSink = (*MeterSink)(nil)

// Checkpoint commits one checkpoint: the call record, the daily aggregate
// and, for a non-zero delta, one outbox row with event key
// mcpa:<call_id>:<seq>, all in one transaction. A resent checkpoint is
// recognized by its sequence number and applied once.
func (m *MeterSink) Checkpoint(ctx context.Context, callID string, deltaReq, deltaResp int64, final bool, r proxy.CallRecordUpdate) error {
	id, err := uuid.Parse(callID)
	if err != nil {
		return err
	}
	rid, err := uuid.Parse(r.ResourceID)
	if err != nil {
		return err
	}
	kid, err := uuid.Parse(r.CallKeyID)
	if err != nil {
		return err
	}
	var cid uuid.UUID
	if r.CredentialID != "" {
		if cid, err = uuid.Parse(r.CredentialID); err != nil {
			return err
		}
	}
	_, err = m.db.ApplyCheckpoint(ctx, store.Checkpoint{
		CallID: id, TenantID: r.TenantID, ResourceID: rid, CallKeyID: kid, CredentialID: cid,
		Provider: r.Provider, Model: r.Model, Format: r.Format, Streaming: r.Streaming, StartTime: r.StartTime,
		Seq: r.CheckpointSeq, DeltaRequestBytes: deltaReq, DeltaResponseBytes: deltaResp,
		RequestBytes: r.RequestBytes, ResponseBytes: r.ResponseBytes, Final: final,
		Status: r.Status, ProviderStatusClass: r.ProviderStatusClass, ErrorClass: string(r.ErrorClass),
		LatencyMS: max(r.Latency.Milliseconds(), 0), OutboxID: store.NewID(), RequestKey: store.NewID(),
	})
	return err
}
