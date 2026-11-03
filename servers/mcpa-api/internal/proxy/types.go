// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"time"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
)

// Credential is a decrypted provider credential of one resource. The core
// layer loads, decrypts and owns persistence; the engine only keeps it in
// memory while the resource's runtime is loaded.
type Credential struct {
	// ID is the credential UUID. It is also the SDK auth ID.
	ID string
	// ResourceID is the owning resource. A credential is only ever registered
	// in the auth manager of this resource.
	ResourceID string
	Provider   mcpav1.Provider
	Kind       mcpav1.CredentialKind
	// Label is a non-secret display label such as an account email.
	Label string
	// Secret is the decrypted payload. For CREDENTIAL_KIND_API_KEY it is the raw
	// API key, except for PROVIDER_VERTEX where a JSON service-account key
	// ({"type":"service_account",...}) is also accepted. For
	// CREDENTIAL_KIND_OAUTH it is the SDK token JSON produced by the OAuth
	// broker (the same object the SDK file store would persist).
	Secret []byte
	// BaseURL overrides the provider endpoint. It is required for
	// PROVIDER_OPENAI_COMPATIBLE.
	BaseURL string
	// Models lists the upstream model names the credential serves. It is
	// required for PROVIDER_OPENAI_COMPATIBLE. For other providers an empty list
	// selects the SDK's default catalog for the provider.
	Models []string
}

// CredentialSource loads the credentials of one resource. Implementations
// return decrypted credentials and must only return credentials whose
// ResourceID equals resourceID.
type CredentialSource interface {
	LoadResourceCredentials(ctx context.Context, resourceID string) ([]Credential, error)
}

// TokenPersister receives OAuth payloads that the SDK changed, for example
// after a token refresh, so the core layer can re-encrypt and store them.
// Calls are made from a background goroutine and never from the SDK's
// selection lock.
type TokenPersister interface {
	PersistCredentialSecret(ctx context.Context, resourceID, credentialID string, secret []byte) error
}

// CallPrincipal is the identity resolved from a call key.
type CallPrincipal struct {
	ResourceID   string
	TenantID     string
	CallKeyID    string
	IssuerUserID string
}

// KeyAuthenticator resolves a call key presented on the model-call path.
// secret is empty when the request carried no key. A failed verification
// returns a *Rejection (normally RejectionInvalidCallKey); other errors are
// reported to the caller as dependency_unavailable.
type KeyAuthenticator interface {
	AuthenticateCallKey(ctx context.Context, resourceID, secret string) (CallPrincipal, error)
}

// CallAuthorizer applies the tenant, billing and budget gates before any
// provider contact. Each method returns nil to allow the call or a
// *Rejection. The engine calls AuthorizeCall before it reads the request body
// and CheckBudgets after the model allowlist, so a request for a model the
// resource does not allow is never reported as a budget rejection.
type CallAuthorizer interface {
	// AuthorizeCall applies the tenant state and paid-invocation gates.
	AuthorizeCall(ctx context.Context, principal CallPrincipal) error
	// CheckBudgets applies the resource and tenant budget gates.
	CheckBudgets(ctx context.Context, principal CallPrincipal) error
}

// RoutingSource returns the model allowlist of a resource. An empty list
// allows every model the resource's credentials can serve.
type RoutingSource interface {
	ModelAllowlist(ctx context.Context, resourceID string) ([]string, error)
}

// CallRecordUpdate is the call metadata attached to a metering checkpoint.
// It never contains request or response bodies.
type CallRecordUpdate struct {
	CallID       string
	ResourceID   string
	TenantID     string
	CallKeyID    string
	CredentialID string
	Provider     mcpav1.Provider
	Model        string
	Format       mcpav1.EndpointFormat
	Streaming    bool
	StartTime    time.Time
	// CheckpointSeq numbers the checkpoints of a call from 1. Together with
	// CallID it forms the idempotent billing event key.
	CheckpointSeq int64
	// RequestBytes and ResponseBytes are the cumulative billed totals after
	// this checkpoint.
	RequestBytes  int64
	ResponseBytes int64
	// The outcome fields are final only when the checkpoint is final.
	Status              mcpav1.CallStatus
	ProviderStatusClass mcpav1.StatusClass
	ErrorClass          ErrorClass
	Latency             time.Duration
}

// MeterSink commits metering checkpoints. deltaRequestBytes and
// deltaResponseBytes are the bytes not billed by earlier checkpoints of the
// call. A returned error leaves the delta unbilled; the next checkpoint
// carries it again.
type MeterSink interface {
	Checkpoint(ctx context.Context, callID string, deltaRequestBytes, deltaResponseBytes int64, final bool, record CallRecordUpdate) error
}

// RejectionCode is a typed reason for refusing a call before provider
// contact.
type RejectionCode int

const (
	RejectionUnspecified RejectionCode = iota
	RejectionInvalidCallKey
	RejectionModelNotAllowed
	RejectionBudgetExhausted
	RejectionTenantInactive
	RejectionDependencyUnavailable
	RejectionNoCredential
	RejectionRequestTooLarge
	RejectionNotFound
	RejectionInternal
)

// String returns the stable error type used in client error bodies.
func (c RejectionCode) String() string {
	switch c {
	case RejectionInvalidCallKey:
		return "invalid_call_key"
	case RejectionModelNotAllowed:
		return "model_not_allowed"
	case RejectionBudgetExhausted:
		return "budget_exhausted"
	case RejectionTenantInactive:
		return "tenant_inactive"
	case RejectionDependencyUnavailable:
		return "dependency_unavailable"
	case RejectionNoCredential:
		return "no_credential"
	case RejectionRequestTooLarge:
		return "request_too_large"
	case RejectionNotFound:
		return "not_found"
	case RejectionInternal:
		return "internal_error"
	default:
		return "unspecified"
	}
}

// HTTPStatus returns the status code used for the rejection. The call-path
// statuses follow the server contract: 401 invalid_call_key, 403
// tenant_inactive and model_not_allowed, 429 budget_exhausted, 503
// dependency_unavailable and 400 no_credential.
func (c RejectionCode) HTTPStatus() int {
	switch c {
	case RejectionInvalidCallKey:
		return http.StatusUnauthorized
	case RejectionModelNotAllowed, RejectionTenantInactive:
		return http.StatusForbidden
	case RejectionBudgetExhausted:
		return http.StatusTooManyRequests
	case RejectionDependencyUnavailable:
		return http.StatusServiceUnavailable
	case RejectionNoCredential:
		return http.StatusBadRequest
	case RejectionNotFound:
		return http.StatusNotFound
	case RejectionRequestTooLarge:
		return http.StatusRequestEntityTooLarge
	default:
		return http.StatusInternalServerError
	}
}

// BudgetScope identifies which budget rejected a call.
type BudgetScope int

const (
	BudgetScopeUnspecified BudgetScope = iota
	BudgetScopeResource
	BudgetScopeTenant
)

// String returns the metric label value of the scope.
func (s BudgetScope) String() string {
	switch s {
	case BudgetScopeResource:
		return "resource"
	case BudgetScopeTenant:
		return "tenant"
	default:
		return "unspecified"
	}
}

// Rejection is a typed refusal of a call. Message must be safe to return to
// the caller and must not contain secrets.
type Rejection struct {
	Code    RejectionCode
	Message string
	// BudgetScope is set when Code is RejectionBudgetExhausted.
	BudgetScope BudgetScope
}

func (r *Rejection) Error() string {
	if r == nil {
		return "<nil rejection>"
	}
	if r.Message == "" {
		return r.Code.String()
	}
	return fmt.Sprintf("%s: %s", r.Code, r.Message)
}

// Reject builds a rejection with a caller-safe message.
func Reject(code RejectionCode, message string) *Rejection {
	return &Rejection{Code: code, Message: message}
}

// asRejection converts err into a rejection. Errors that are not rejections
// become fallback with a generic message so internal details never reach the
// caller.
func asRejection(err error, fallback RejectionCode) *Rejection {
	if err == nil {
		return nil
	}
	var rejection *Rejection
	if errors.As(err, &rejection) && rejection != nil {
		return rejection
	}
	message := "the request could not be authorized right now"
	if fallback == RejectionInternal {
		message = "internal error"
	}
	return Reject(fallback, message)
}
