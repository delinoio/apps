// SPDX-License-Identifier: FSL-1.1-ALv2

// Package service implements mcpa.v1.McpaService.
//
// Handlers return *apperr.Error values; the Connect interceptor in the
// server package renders them with a typed ErrorDetail. Every mutation runs
// in one transaction with its audit event.
package service

import (
	"context"
	"crypto/sha256"
	"encoding/binary"
	"errors"
	"log/slog"
	"strings"
	"sync"
	"time"
	"unicode"
	"unicode/utf8"

	"connectrpc.com/connect"
	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"google.golang.org/protobuf/types/known/timestamppb"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/protos/mcpa/v1/mcpav1connect"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/authz"
	"github.com/delinoio/apps/servers/mcpa-api/internal/crypto"
	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

// OAuthBroker runs provider OAuth flows. The data plane layer implements it.
// The service has already verified that the caller holds MANAGE on res and
// that res is not deleted. Implementations write their own audit events
// (AUDIT_ACTION_CREDENTIAL_REGISTERED) with Service.WriteAudit.
type OAuthBroker interface {
	StartProviderOAuth(ctx context.Context, p *auth.Principal, res store.Resource, req *mcpav1.StartProviderOAuthRequest) (*mcpav1.ProviderOAuthFlow, error)
	CompleteProviderOAuth(ctx context.Context, p *auth.Principal, res store.Resource, req *mcpav1.CompleteProviderOAuthRequest) (*mcpav1.ProviderOAuthFlow, error)
	GetProviderOAuthFlow(ctx context.Context, p *auth.Principal, res store.Resource, req *mcpav1.GetProviderOAuthFlowRequest) (*mcpav1.ProviderOAuthFlow, error)
}

// UsageQuery is an authorized usage or call-record query.
type UsageQuery struct {
	TenantID string
	// ResourceIDs are the resources the caller may read, already intersected
	// with Filter.ResourceIds. It is never empty.
	ResourceIDs []uuid.UUID
	// Filter is the caller's filter; ignore its ResourceIds in favor of
	// ResourceIDs.
	Filter *mcpav1.CallFilter
	Page   Page
}

// UsageReader reads call records and daily aggregates. The data plane layer
// implements it. Responses need no Metadata; the service sets it.
type UsageReader interface {
	GetUsage(ctx context.Context, q UsageQuery) (*mcpav1.GetUsageResponse, error)
	ListCallRecords(ctx context.Context, q UsageQuery) (*mcpav1.ListCallRecordsResponse, error)
}

// SpendReader returns locally recorded billed bytes for budgets. The data
// plane layer implements it; without one, measured spend is zero.
type SpendReader interface {
	ResourceBilledBytes(ctx context.Context, resourceID uuid.UUID, start, end time.Time) (int64, error)
	TenantBilledBytes(ctx context.Context, tenantID string, start, end time.Time) (int64, error)
}

// CredentialListener is notified after credential changes commit so the
// data plane can update its in-process provider auth state immediately.
// Implementations must not block for long.
type CredentialListener interface {
	CredentialRegistered(ctx context.Context, resourceID, credentialID uuid.UUID)
	CredentialRemoved(ctx context.Context, resourceID, credentialID uuid.UUID)
	ResourceDeleted(ctx context.Context, resourceID uuid.UUID, credentialIDs []uuid.UUID)
}

// Hooks are the extension points filled by later layers. Nil members keep
// the default behavior: unimplemented OAuth and usage RPCs, zero spend and
// no credential notifications.
type Hooks struct {
	OAuth       OAuthBroker
	Usage       UsageReader
	Spend       SpendReader
	Credentials CredentialListener
}

// DelcoreReader is the subset of the Delcore client used by handlers.
type DelcoreReader interface {
	GetCurrentUser(ctx context.Context, userToken string) (delcore.CurrentUser, error)
	BillingPeriod(ctx context.Context, tenantID string) (delcore.Period, error)
}

// Config configures the service.
type Config struct {
	Store      *store.Store
	Keys       *crypto.Keyring
	Authorizer *authz.Authorizer
	Delcore    DelcoreReader
	// PublicURL is the API origin used to build endpoint base URLs.
	PublicURL string
	Log       *slog.Logger
}

// Service implements mcpav1connect.McpaServiceHandler.
type Service struct {
	db        *store.Store
	keys      *crypto.Keyring
	authz     *authz.Authorizer
	dc        DelcoreReader
	publicURL string
	log       *slog.Logger
	now       func() time.Time

	hooksMu sync.RWMutex
	hooks   Hooks
}

var _ mcpav1connect.McpaServiceHandler = (*Service)(nil)

// New builds the service.
func New(cfg Config) *Service {
	return &Service{
		db: cfg.Store, keys: cfg.Keys, authz: cfg.Authorizer, dc: cfg.Delcore,
		publicURL: strings.TrimSuffix(cfg.PublicURL, "/"), log: cfg.Log, now: time.Now,
	}
}

// SetHooks installs extension hooks. Call it before serving.
func (s *Service) SetHooks(h Hooks) {
	s.hooksMu.Lock()
	defer s.hooksMu.Unlock()
	s.hooks = h
}

func (s *Service) getHooks() Hooks {
	s.hooksMu.RLock()
	defer s.hooksMu.RUnlock()
	return s.hooks
}

func principal(ctx context.Context) (*auth.Principal, error) {
	p := auth.FromContext(ctx)
	if p == nil {
		return nil, apperr.Unauthenticated("sign in to continue")
	}
	return p, nil
}

// Endpoints returns the base URLs of a resource for every client format.
func Endpoints(publicURL string, resourceID uuid.UUID) []*mcpav1.Endpoint {
	base := strings.TrimSuffix(publicURL, "/") + "/r/" + resourceID.String()
	return []*mcpav1.Endpoint{
		{Format: mcpav1.EndpointFormat_ENDPOINT_FORMAT_OPENAI, BaseUrl: base + "/v1"},
		{Format: mcpav1.EndpointFormat_ENDPOINT_FORMAT_CLAUDE, BaseUrl: base},
		{Format: mcpav1.EndpointFormat_ENDPOINT_FORMAT_GEMINI, BaseUrl: base},
	}
}

func ts(t time.Time) *timestamppb.Timestamp { return timestamppb.New(t) }

func tsPtr(t *time.Time) *timestamppb.Timestamp {
	if t == nil {
		return nil
	}
	return timestamppb.New(*t)
}

func (s *Service) resourceProto(r store.Resource, role mcpav1.ResourceRole) *mcpav1.Resource {
	return &mcpav1.Resource{
		Id: r.ID.String(), TenantId: r.TenantID, Name: r.Name, Description: r.Description,
		Routing:   &mcpav1.RoutingSettings{LoadBalancing: r.LoadBalancing, ModelAllowlist: r.ModelAllowlist},
		Endpoints: Endpoints(s.publicURL, r.ID), CallerRole: role, CreatedByUserId: r.CreatedByUserID,
		CreateTime: ts(r.CreateTime), UpdateTime: ts(r.UpdateTime),
	}
}

func aclProto(e store.ACLEntry) *mcpav1.AclEntry {
	return &mcpav1.AclEntry{
		ResourceId: e.ResourceID.String(), PrincipalKind: e.PrincipalKind, PrincipalId: e.PrincipalID,
		Role: e.Role, CreatedByUserId: e.CreatedByUserID, CreateTime: ts(e.CreateTime),
	}
}

// CredentialProto converts stored credential metadata. It never includes
// secrets.
func CredentialProto(c store.Credential) *mcpav1.Credential {
	return &mcpav1.Credential{
		Id: c.ID.String(), ResourceId: c.ResourceID.String(), Provider: c.Provider, Kind: c.Kind, Name: c.Name,
		AccountLabel: c.AccountLabel, SecretHint: c.SecretHint, BaseUrl: c.BaseURL, Models: c.Models,
		CreatedByUserId: c.CreatedByUserID, CreateTime: ts(c.CreateTime),
	}
}

func callKeyProto(k store.CallKey) *mcpav1.CallKey {
	return &mcpav1.CallKey{
		Id: k.ID.String(), ResourceId: k.ResourceID.String(), Name: k.Name, DisplayPrefix: displayPrefix(k.ID),
		IssuerUserId: k.IssuerUserID, CreateTime: ts(k.CreateTime), ExpireTime: tsPtr(k.ExpireTime),
		LastUsedTime: tsPtr(k.LastUsedTime), RevokeTime: tsPtr(k.RevokeTime),
	}
}

func auditProto(e store.AuditEvent) *mcpav1.AuditEvent {
	out := &mcpav1.AuditEvent{
		Id: e.ID.String(), TenantId: e.TenantID, Action: e.Action, ActorKind: e.ActorKind,
		ActorUserId: e.ActorUserID, TargetId: e.TargetID, Details: e.Details, OccurTime: ts(e.OccurTime),
	}
	if e.ResourceID != uuid.Nil {
		out.ResourceId = e.ResourceID.String()
	}
	return out
}

// AuditInput describes one audit event.
type AuditInput struct {
	TenantID   string
	ResourceID uuid.UUID
	Action     mcpav1.AuditAction
	TargetID   string
	Details    map[string]string
}

// WriteAudit records an audit event for the principal inside tx. Later
// layers use it for OAuth credential registration.
func WriteAudit(ctx context.Context, tx store.DB, p *auth.Principal, in AuditInput) error {
	_, err := store.InsertAudit(ctx, tx, store.AuditEvent{
		TenantID: in.TenantID, ResourceID: in.ResourceID, Action: in.Action, ActorKind: p.Kind,
		ActorUserID: p.UserID, TargetID: in.TargetID, Details: in.Details,
	})
	return err
}

// mutate locks a live resource and runs fn in one transaction.
func (s *Service) mutate(ctx context.Context, resourceID uuid.UUID, fn func(tx pgx.Tx, r store.Resource) error) error {
	err := s.db.InTx(ctx, func(tx pgx.Tx) error {
		r, err := store.LockResource(ctx, tx, resourceID)
		if err != nil {
			if errors.Is(err, store.ErrNotFound) {
				return apperr.NotFound("resource not found")
			}
			return err
		}
		if r.Deleted() {
			return apperr.FailedPrecondition("the resource was deleted")
		}
		return fn(tx, r)
	})
	return dbErr(err)
}

// dbErr keeps typed and Connect errors and maps other storage errors to
// DEPENDENCY_UNAVAILABLE. The cause is logged by the interceptor.
func dbErr(err error) error {
	if err == nil {
		return nil
	}
	var ae *apperr.Error
	if errors.As(err, &ae) {
		return ae
	}
	var ce *connect.Error
	if errors.As(err, &ce) || errors.Is(err, context.Canceled) {
		return apperr.As(err)
	}
	return apperr.DependencyUnavailable(err)
}

// Limits on management input.
const (
	maxNameBytes        = 100
	maxDescriptionBytes = 1000
	maxAllowlist        = 200
)

func validateName(field, v string) error {
	if strings.TrimSpace(v) == "" || len(v) > maxNameBytes || !utf8.ValidString(v) || hasControl(v) {
		return apperr.InvalidInput(field + " must contain 1 to 100 bytes of printable text")
	}
	return nil
}

func validateDescription(v string) error {
	if len(v) > maxDescriptionBytes || !utf8.ValidString(v) {
		return apperr.InvalidInput("description must contain at most 1000 bytes of text")
	}
	for _, r := range v {
		if unicode.IsControl(r) && r != '\n' && r != '\t' {
			return apperr.InvalidInput("description must not contain control characters")
		}
	}
	return nil
}

func hasControl(v string) bool {
	for _, r := range v {
		if unicode.IsControl(r) {
			return true
		}
	}
	return false
}

func parseRequestKey(v string) (uuid.UUID, error) {
	id, err := uuid.Parse(v)
	if err != nil || id.String() != v || id.Version() != 7 {
		return uuid.Nil, apperr.InvalidInput("request_key must be a canonical UUID v7")
	}
	return id, nil
}

// fingerprint hashes length-prefixed request fields for idempotency.
func fingerprint(fields ...string) []byte {
	h := sha256.New()
	var n [8]byte
	for _, f := range fields {
		binary.BigEndian.PutUint64(n[:], uint64(len(f)))
		h.Write(n[:])
		h.Write([]byte(f))
	}
	return h.Sum(nil)
}

// Page is a validated list page.
type Page struct {
	Size  int
	After uuid.UUID
}

// Default and maximum page sizes.
const (
	DefaultPageSize = 50
	MaxPageSize     = 200
)

// ParsePage validates a request page.
func ParsePage(p *mcpav1.Page) (Page, error) {
	out := Page{Size: DefaultPageSize}
	if p == nil {
		return out, nil
	}
	switch {
	case p.GetSize() < 0:
		return Page{}, apperr.InvalidInput("page.size must not be negative")
	case p.GetSize() > MaxPageSize:
		out.Size = MaxPageSize
	case p.GetSize() > 0:
		out.Size = int(p.GetSize())
	}
	if p.GetAfterId() != "" {
		id, err := authz.ParseID(p.GetAfterId(), "page.after_id")
		if err != nil {
			return Page{}, err
		}
		out.After = id
	}
	return out, nil
}
