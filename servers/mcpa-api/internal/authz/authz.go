// SPDX-License-Identifier: FSL-1.1-ALv2

// Package authz combines Delcore capabilities with per-resource roles.
//
// Every per-resource management action requires the Delcore capability
// access_resources and a resource role. The effective role is the strongest
// of the user's direct entry and the entries of groups the user belongs to
// in the latest tenant group snapshot. MANAGE implies INVOKE.
package authz

import (
	"context"
	"errors"
	"log/slog"
	"slices"
	"sync"
	"time"

	"connectrpc.com/connect"
	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"

	delcorev1 "github.com/delinoio/apps/protos/delcore/v1"
	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

// rank orders roles; unknown roles rank lowest.
func rank(r mcpav1.ResourceRole) int {
	switch r {
	case mcpav1.ResourceRole_RESOURCE_ROLE_MANAGE:
		return 2
	case mcpav1.ResourceRole_RESOURCE_ROLE_INVOKE:
		return 1
	default:
		return 0
	}
}

// Strongest returns the strongest of roles, or UNSPECIFIED.
func Strongest(roles ...mcpav1.ResourceRole) mcpav1.ResourceRole {
	best := mcpav1.ResourceRole_RESOURCE_ROLE_UNSPECIFIED
	for _, r := range roles {
		if rank(r) > rank(best) {
			best = r
		}
	}
	return best
}

// Satisfies reports whether have permits an action that needs need.
func Satisfies(have, need mcpav1.ResourceRole) bool {
	return rank(need) > 0 && rank(have) >= rank(need)
}

// Effective resolves a user's role from ACL entries and the user's groups.
func Effective(entries []store.ACLEntry, userID string, groupIDs []string) mcpav1.ResourceRole {
	best := mcpav1.ResourceRole_RESOURCE_ROLE_UNSPECIFIED
	for _, e := range entries {
		match := (e.PrincipalKind == mcpav1.PrincipalKind_PRINCIPAL_KIND_USER && e.PrincipalID == userID) ||
			(e.PrincipalKind == mcpav1.PrincipalKind_PRINCIPAL_KIND_GROUP && slices.Contains(groupIDs, e.PrincipalID))
		if match {
			best = Strongest(best, e.Role)
		}
	}
	return best
}

// HasOtherManager reports whether entries keep a MANAGE holder after the
// entry identified by kind and principalID is removed or downgraded. A group
// with MANAGE counts.
func HasOtherManager(entries []store.ACLEntry, kind mcpav1.PrincipalKind, principalID string) bool {
	for _, e := range entries {
		if e.PrincipalKind == kind && e.PrincipalID == principalID {
			continue
		}
		if e.Role == mcpav1.ResourceRole_RESOURCE_ROLE_MANAGE {
			return true
		}
	}
	return false
}

// Delcore is the subset of the Delcore client used for authorization.
type Delcore interface {
	Authorize(ctx context.Context, tenantID, userID string, capability delcore.Capability, paid bool) (delcore.Decision, error)
	AuthorizeAPIKey(ctx context.Context, secret string, capability delcore.Capability, paid bool) (delcore.KeyDecision, error)
	ListGroups(ctx context.Context, userToken, tenantID string) ([]*delcorev1.Group, error)
}

// Authorizer evaluates management access.
type Authorizer struct {
	db             *store.Store
	dc             Delcore
	refreshTTL     time.Duration
	refreshTimeout time.Duration
	gate           tenantGate
	log            *slog.Logger
	now            func() time.Time
}

// New builds an Authorizer. refreshTTL bounds group snapshot refreshes.
func New(db *store.Store, dc Delcore, refreshTTL time.Duration, log *slog.Logger) *Authorizer {
	return &Authorizer{db: db, dc: dc, refreshTTL: refreshTTL, log: log, now: time.Now}
}

// keyAllowedForManagement reports whether a personal-key decision admits a
// management action, which Delcore evaluates with paid=false. Delcore
// authorizes keys with a strict tenant lifecycle, so a tenant that is being
// deleted denies even an unpaid check with the single reason
// BLOCK_REASON_DELETING. Management must stay available in that state so
// customers can still remove credentials and keys (see the management tenant
// state rule in the Managed CPA API contract). Any other reason, including
// BLOCK_REASON_PERMISSION next to DELETING, or an empty reason list on a
// denial, keeps the denial.
func keyAllowedForManagement(d delcore.KeyDecision) bool {
	if d.Allowed {
		return true
	}
	if len(d.Reasons) == 0 {
		return false
	}
	for _, r := range d.Reasons {
		if r != delcorev1.BlockReason_BLOCK_REASON_DELETING {
			return false
		}
	}
	return true
}

// Capability requires a Delcore capability of the principal in tenantID
// with paid=false. Personal keys act only in their own tenant. A personal key
// of a tenant that Delcore is deleting still passes when its only denial
// reason is BLOCK_REASON_DELETING.
func (a *Authorizer) Capability(ctx context.Context, p *auth.Principal, tenantID string, capability delcore.Capability) error {
	if tenantID == "" {
		return apperr.InvalidInput("tenant_id is required")
	}
	if p.IsPersonalKey() {
		if p.KeyTenantID != tenantID {
			return apperr.PermissionDenied("the API key belongs to another tenant")
		}
		d, err := a.dc.AuthorizeAPIKey(ctx, p.APIKeySecret(), capability, false)
		if err != nil {
			if delcore.IsUnauthenticated(err) {
				return apperr.Unauthenticated("invalid Delcore API key")
			}
			return apperr.DependencyUnavailable(err)
		}
		if !keyAllowedForManagement(d) || d.TenantID != tenantID || d.UserID != p.UserID {
			return apperr.PermissionDenied("missing Delcore capability " + string(capability))
		}
		return nil
	}
	d, err := a.dc.Authorize(ctx, tenantID, p.UserID, capability, false)
	if err != nil {
		var de *delcore.Error
		if errors.As(err, &de) && de.Kind == delcore.KindRejected && de.Code == connect.CodeInvalidArgument {
			return apperr.InvalidInput("tenant_id is not a valid tenant ID")
		}
		return apperr.DependencyUnavailable(err)
	}
	if !d.Allowed {
		return apperr.PermissionDenied("missing Delcore capability " + string(capability))
	}
	return nil
}

// defaultRefreshTimeout bounds one group snapshot refresh when the operator
// has not set one with WithRefreshTimeout.
const defaultRefreshTimeout = 5 * time.Second

// WithRefreshTimeout bounds one group snapshot refresh: the wait for other
// refreshes of the same tenant, the Delcore ListGroups call and the snapshot
// write, so no transaction outlives it. The server passes DEPENDENCY_TIMEOUT.
// Values of zero or less keep the default. It returns a for chaining.
func (a *Authorizer) WithRefreshTimeout(d time.Duration) *Authorizer {
	if d > 0 {
		a.refreshTimeout = d
	}
	return a
}

// RefreshGroups refreshes the tenant group snapshot with the principal's
// Delcore user token when it is older than the refresh TTL. Failures keep
// the previous snapshot and are logged; capability checks stay live. It is
// the best-effort form for callers that have no console request to fail,
// such as the OAuth grant snapshot. Console request paths call
// RequireGroups, which also reports a lost MFA evidence.
func (a *Authorizer) RefreshGroups(ctx context.Context, p *auth.Principal, tenantID string) {
	if err := a.RequireGroups(ctx, p, tenantID); err != nil && isMFARequired(err) {
		a.log.WarnContext(ctx, "group snapshot refresh skipped because the session lost MFA evidence", "error", err)
	}
}

// RequireGroups is RefreshGroups for console request paths. Other refresh
// failures keep the previous snapshot and are logged, and it returns only an
// ERROR_REASON_MFA_REQUIRED error: a console session whose in-request token
// refresh finds no MFA evidence must not serve data in that request, because
// the request was admitted with the MFA flag stored before the refresh.
func (a *Authorizer) RequireGroups(ctx context.Context, p *auth.Principal, tenantID string) error {
	if !p.HasUserToken() {
		return nil
	}
	if _, err := a.refreshGroupsIfStale(ctx, p, tenantID); err != nil {
		if isMFARequired(err) {
			return err
		}
		a.log.WarnContext(ctx, "group snapshot refresh failed", "error", err)
	}
	return nil
}

// isMFARequired reports whether err is an MFA-required application error.
func isMFARequired(err error) bool {
	var ae *apperr.Error
	return errors.As(err, &ae) && ae.Reason == mcpav1.ErrorReason_ERROR_REASON_MFA_REQUIRED
}

// SnapshotGroups returns the tenant's groups after the TTL-aware refresh of
// RefreshGroups, so that an ACL editor that opens repeatedly does not call
// Delcore ListGroups more than once per refresh TTL for the tenant, across
// concurrent callers and replicas. A principal without a user token (a
// personal key) reads the latest snapshot without a refresh.
//
// A failed refresh keeps the previous snapshot and its refresh time. The
// error is returned when the tenant has no snapshot yet, because the caller
// would otherwise show an empty group list as if it were current, and when
// the user's Delcore credentials are the cause, because the user must sign
// in again. Any other failure is logged and the previous snapshot is served.
func (a *Authorizer) SnapshotGroups(ctx context.Context, p *auth.Principal, tenantID string) ([]store.Group, error) {
	if p.HasUserToken() {
		hadSnapshot, err := a.refreshGroupsIfStale(ctx, p, tenantID)
		if err != nil {
			if !hadSnapshot || isCredentialError(err) {
				return nil, err
			}
			a.log.WarnContext(ctx, "group snapshot refresh failed; serving the previous snapshot", "error", err)
		}
	}
	return store.SnapshotGroups(ctx, a.db.Pool, tenantID)
}

// isCredentialError reports whether err means that the principal's Delcore
// user credentials are missing, expired or rejected.
func isCredentialError(err error) bool {
	if errors.Is(err, auth.ErrNoUserToken) || delcore.IsUnauthenticated(err) || isMFARequired(err) {
		return true
	}
	var ae *apperr.Error
	return errors.As(err, &ae) && ae.Reason == mcpav1.ErrorReason_ERROR_REASON_UNAUTHENTICATED
}

// groupRefreshLockKey namespaces the PostgreSQL advisory lock that
// serializes refreshes of one tenant across replicas.
const groupRefreshLockKey = "mcpa:group-snapshot-refresh:"

// refreshGroupsIfStale refreshes the tenant snapshot when it is older than
// the refresh TTL. It reports whether a snapshot existed before the call.
//
// Refreshes of one tenant are serialized in two steps. A per-process gate
// admits one goroutine per tenant, so waiting callers do not each hold a
// pooled connection. A transaction-scoped PostgreSQL advisory lock then
// serializes replicas. The refresh time is read again under the lock, so a
// caller that waited finds the snapshot another caller just wrote and skips
// Delcore. The lock lives only as long as the transaction, which ends at the
// refresh timeout at the latest; a failed refresh rolls back and leaves the
// snapshot and its refresh time unchanged.
func (a *Authorizer) refreshGroupsIfStale(ctx context.Context, p *auth.Principal, tenantID string) (hadSnapshot bool, err error) {
	last, err := store.SnapshotRefreshTime(ctx, a.db.Pool, tenantID)
	if err != nil {
		return false, err
	}
	if a.fresh(last) {
		return true, nil
	}
	timeout := a.refreshTimeout
	if timeout <= 0 {
		timeout = defaultRefreshTimeout
	}
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	release, err := a.gate.acquire(ctx, tenantID)
	if err != nil {
		return !last.IsZero(), err
	}
	defer release()

	hadSnapshot = !last.IsZero()
	err = a.db.InTx(ctx, func(tx pgx.Tx) error {
		if _, err := tx.Exec(ctx, `SELECT pg_advisory_xact_lock(hashtextextended($1, 0))`, groupRefreshLockKey+tenantID); err != nil {
			return err
		}
		locked, err := store.SnapshotRefreshTime(ctx, tx, tenantID)
		if err != nil {
			return err
		}
		hadSnapshot = !locked.IsZero()
		if a.fresh(locked) {
			return nil
		}
		groups, err := a.listGroups(ctx, p, tenantID)
		if err != nil {
			return err
		}
		return store.ReplaceGroupSnapshot(ctx, tx, tenantID, groups, a.now())
	})
	return hadSnapshot, err
}

// fresh reports whether a snapshot refreshed at last is within the TTL.
func (a *Authorizer) fresh(last time.Time) bool {
	return !last.IsZero() && a.now().Sub(last) < a.refreshTTL
}

// listGroups reads all tenant groups with the principal's user token.
func (a *Authorizer) listGroups(ctx context.Context, p *auth.Principal, tenantID string) ([]store.Group, error) {
	token, err := p.DelcoreToken(ctx)
	if err != nil {
		return nil, err
	}
	groups, err := a.dc.ListGroups(ctx, token, tenantID)
	if err != nil {
		return nil, err
	}
	out := make([]store.Group, 0, len(groups))
	for _, g := range groups {
		out = append(out, store.Group{ID: g.GetId(), Name: g.GetName(), UserIDs: g.GetUserIds()})
	}
	return out, nil
}

// tenantGate admits one holder per tenant key inside this process. Its zero
// value is ready to use.
type tenantGate struct {
	mu      sync.Mutex
	entries map[string]*gateEntry
}

type gateEntry struct {
	slot chan struct{}
	refs int
}

// acquire waits for the key's slot or for ctx to end. The returned release
// function frees the slot.
func (g *tenantGate) acquire(ctx context.Context, key string) (func(), error) {
	g.mu.Lock()
	if g.entries == nil {
		g.entries = map[string]*gateEntry{}
	}
	e := g.entries[key]
	if e == nil {
		e = &gateEntry{slot: make(chan struct{}, 1)}
		g.entries[key] = e
	}
	e.refs++
	g.mu.Unlock()

	unref := func() {
		g.mu.Lock()
		defer g.mu.Unlock()
		if e.refs--; e.refs == 0 {
			delete(g.entries, key)
		}
	}
	select {
	case e.slot <- struct{}{}:
		return func() { <-e.slot; unref() }, nil
	case <-ctx.Done():
		unref()
		return nil, ctx.Err()
	}
}

// EffectiveRole returns the principal's role on a resource.
func (a *Authorizer) EffectiveRole(ctx context.Context, db store.DB, tenantID string, resourceID uuid.UUID, userID string) (mcpav1.ResourceRole, error) {
	return effectiveRole(ctx, db, tenantID, resourceID, userID)
}

// Recheck reads the role p holds on r through db and checks it against need.
// Callers run it in the transaction that locked r with store.LockResource,
// after a first Resource check outside that lock. ACL changes run under the
// same lock, so the recheck applies an ACL change that committed while the
// caller waited for the lock or worked outside the transaction. Like
// Resource, it reports no role as NOT_FOUND, a weaker role as
// PERMISSION_DENIED and a deleted resource as FAILED_PRECONDITION. Storage
// errors are returned unwrapped.
func Recheck(ctx context.Context, db store.DB, p *auth.Principal, r store.Resource, need mcpav1.ResourceRole) (mcpav1.ResourceRole, error) {
	role, err := effectiveRole(ctx, db, r.TenantID, r.ID, p.UserID)
	if err != nil {
		return 0, err
	}
	if role == mcpav1.ResourceRole_RESOURCE_ROLE_UNSPECIFIED {
		return 0, apperr.NotFound("resource not found")
	}
	if !Satisfies(role, need) {
		return 0, apperr.PermissionDenied("this action requires the " + RoleName(need) + " role on the resource")
	}
	if r.Deleted() {
		return 0, apperr.FailedPrecondition("the resource was deleted")
	}
	return role, nil
}

func effectiveRole(ctx context.Context, db store.DB, tenantID string, resourceID uuid.UUID, userID string) (mcpav1.ResourceRole, error) {
	groups, err := store.UserGroupIDs(ctx, db, tenantID, userID)
	if err != nil {
		return 0, err
	}
	entries, err := store.ListACL(ctx, db, resourceID)
	if err != nil {
		return 0, err
	}
	return Effective(entries, userID, groups), nil
}

// UserGroups returns the principal's snapshot groups in a tenant.
func (a *Authorizer) UserGroups(ctx context.Context, tenantID, userID string) ([]string, error) {
	return store.UserGroupIDs(ctx, a.db.Pool, tenantID, userID)
}

// Access is the result of a per-resource check.
type Access struct {
	Resource store.Resource
	Role     mcpav1.ResourceRole
}

// AccessOptions modifies a per-resource check.
type AccessOptions struct {
	// AllowDeleted admits read-only actions on a resource tombstone.
	AllowDeleted bool
}

// Resource checks that the principal holds need on resourceID and the
// access_resources capability in its tenant. A resource on which the
// principal holds no role is reported as not found.
func (a *Authorizer) Resource(ctx context.Context, p *auth.Principal, resourceID string, need mcpav1.ResourceRole, opts AccessOptions) (Access, error) {
	id, err := ParseID(resourceID, "resource_id")
	if err != nil {
		return Access{}, err
	}
	r, err := store.GetResource(ctx, a.db.Pool, id)
	if errors.Is(err, store.ErrNotFound) {
		return Access{}, apperr.NotFound("resource not found")
	}
	if err != nil {
		return Access{}, apperr.DependencyUnavailable(err)
	}
	if p.IsPersonalKey() && p.KeyTenantID != r.TenantID {
		return Access{}, apperr.NotFound("resource not found")
	}
	if err := a.RequireGroups(ctx, p, r.TenantID); err != nil {
		return Access{}, err
	}
	role, err := a.EffectiveRole(ctx, a.db.Pool, r.TenantID, r.ID, p.UserID)
	if err != nil {
		return Access{}, apperr.DependencyUnavailable(err)
	}
	if role == mcpav1.ResourceRole_RESOURCE_ROLE_UNSPECIFIED {
		return Access{}, apperr.NotFound("resource not found")
	}
	if err := a.Capability(ctx, p, r.TenantID, delcore.CapabilityAccessResources); err != nil {
		return Access{}, err
	}
	if !Satisfies(role, need) {
		return Access{}, apperr.PermissionDenied("this action requires the " + RoleName(need) + " role on the resource")
	}
	if r.Deleted() && !opts.AllowDeleted {
		return Access{}, apperr.FailedPrecondition("the resource was deleted")
	}
	return Access{Resource: r, Role: role}, nil
}

// RoleName returns a lowercase role name for messages.
func RoleName(r mcpav1.ResourceRole) string {
	switch r {
	case mcpav1.ResourceRole_RESOURCE_ROLE_MANAGE:
		return "manage"
	case mcpav1.ResourceRole_RESOURCE_ROLE_INVOKE:
		return "invoke"
	default:
		return "unspecified"
	}
}

// ParseID parses a canonical lowercase UUID field.
func ParseID(value, field string) (uuid.UUID, error) {
	id, err := uuid.Parse(value)
	if err != nil || id.String() != value {
		return uuid.Nil, apperr.InvalidInput(field + " must be a canonical UUID")
	}
	return id, nil
}
