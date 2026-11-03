// SPDX-License-Identifier: FSL-1.1-ALv2

// Package apperr defines typed management errors and their Connect form.
package apperr

import (
	"context"
	"errors"
	"fmt"

	"connectrpc.com/connect"
	"github.com/google/uuid"
	"github.com/jackc/pgx/v5/pgconn"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
)

// Error is a management failure with a stable reason. Message must be safe
// to show to callers: it never contains secrets or internal details.
type Error struct {
	Code      connect.Code
	Reason    mcpav1.ErrorReason
	Message   string
	ReauthURL string
	// Cause is logged but never returned to callers.
	Cause error
}

func (e *Error) Error() string {
	if e.Cause != nil {
		return fmt.Sprintf("%s: %s: %v", e.Reason, e.Message, e.Cause)
	}
	return fmt.Sprintf("%s: %s", e.Reason, e.Message)
}

func (e *Error) Unwrap() error { return e.Cause }

func newErr(code connect.Code, reason mcpav1.ErrorReason, msg string) *Error {
	return &Error{Code: code, Reason: reason, Message: msg}
}

// InvalidInput reports a malformed request field.
func InvalidInput(msg string) *Error {
	return newErr(connect.CodeInvalidArgument, mcpav1.ErrorReason_ERROR_REASON_INVALID_INPUT, msg)
}

// Unauthenticated reports missing or invalid credentials.
func Unauthenticated(msg string) *Error {
	return newErr(connect.CodeUnauthenticated, mcpav1.ErrorReason_ERROR_REASON_UNAUTHENTICATED, msg)
}

// PermissionDenied reports a failed capability or role check.
func PermissionDenied(msg string) *Error {
	return newErr(connect.CodePermissionDenied, mcpav1.ErrorReason_ERROR_REASON_PERMISSION_DENIED, msg)
}

// NotFound reports a missing or invisible entity.
func NotFound(msg string) *Error {
	return newErr(connect.CodeNotFound, mcpav1.ErrorReason_ERROR_REASON_NOT_FOUND, msg)
}

// AlreadyExists reports a uniqueness conflict.
func AlreadyExists(msg string) *Error {
	return newErr(connect.CodeAlreadyExists, mcpav1.ErrorReason_ERROR_REASON_ALREADY_EXISTS, msg)
}

// FailedPrecondition reports a state that forbids the operation.
func FailedPrecondition(msg string) *Error {
	return newErr(connect.CodeFailedPrecondition, mcpav1.ErrorReason_ERROR_REASON_FAILED_PRECONDITION, msg)
}

// MFARequired reports a Logto token without MFA evidence.
func MFARequired(reauthURL string) *Error {
	e := newErr(connect.CodePermissionDenied, mcpav1.ErrorReason_ERROR_REASON_MFA_REQUIRED, "multi-factor authentication is required")
	e.ReauthURL = reauthURL
	return e
}

// DependencyUnavailable reports a Delcore or database failure.
func DependencyUnavailable(cause error) *Error {
	e := newErr(connect.CodeUnavailable, mcpav1.ErrorReason_ERROR_REASON_DEPENDENCY_UNAVAILABLE, "a dependency is unavailable; retry later")
	e.Cause = cause
	return e
}

// TenantInactive reports a tenant that cannot use paid features.
func TenantInactive(msg string) *Error {
	return newErr(connect.CodeFailedPrecondition, mcpav1.ErrorReason_ERROR_REASON_TENANT_INACTIVE, msg)
}

// IdempotencyConflict reports a reused request key with different content.
func IdempotencyConflict() *Error {
	return newErr(connect.CodeAlreadyExists, mcpav1.ErrorReason_ERROR_REASON_IDEMPOTENCY_CONFLICT, "request_key was already used with different content")
}

// TenantServiceKeyRejected reports a Delcore tenant service key.
func TenantServiceKeyRejected() *Error {
	return newErr(connect.CodePermissionDenied, mcpav1.ErrorReason_ERROR_REASON_TENANT_SERVICE_KEY_REJECTED, "tenant service keys cannot manage mcpa; use a personal key")
}

// Unimplemented reports a feature supplied by a layer that is not installed.
func Unimplemented(msg string) *Error {
	return newErr(connect.CodeUnimplemented, mcpav1.ErrorReason_ERROR_REASON_UNSPECIFIED, msg)
}

// Internal wraps an unexpected failure. The cause is logged only.
func Internal(cause error) *Error {
	e := newErr(connect.CodeInternal, mcpav1.ErrorReason_ERROR_REASON_UNSPECIFIED, "internal error")
	e.Cause = cause
	return e
}

type traceKey struct{}

// WithTraceID stores the request correlation ID.
func WithTraceID(ctx context.Context, id string) context.Context {
	return context.WithValue(ctx, traceKey{}, id)
}

// TraceID returns the request correlation ID, or an empty string.
func TraceID(ctx context.Context) string {
	id, _ := ctx.Value(traceKey{}).(string)
	return id
}

// NewTraceID returns a fresh UUID v7 correlation ID.
func NewTraceID() string {
	id, err := uuid.NewV7()
	if err != nil {
		return uuid.NewString()
	}
	return id.String()
}

// As converts any error to an *Error. Connect errors from other layers keep
// their code; context cancellation maps to Canceled/DeadlineExceeded.
func As(err error) *Error {
	var e *Error
	if errors.As(err, &e) {
		return e
	}
	var ce *connect.Error
	if errors.As(err, &ce) {
		out := newErr(ce.Code(), mcpav1.ErrorReason_ERROR_REASON_UNSPECIFIED, ce.Message())
		for _, d := range ce.Details() {
			if v, derr := d.Value(); derr == nil {
				if detail, ok := v.(*mcpav1.ErrorDetail); ok {
					out.Reason = detail.GetReason()
					out.ReauthURL = detail.GetReauthenticationUrl()
				}
			}
		}
		return out
	}
	switch {
	case errors.Is(err, context.Canceled):
		return &Error{Code: connect.CodeCanceled, Reason: mcpav1.ErrorReason_ERROR_REASON_UNSPECIFIED, Message: "request canceled", Cause: err}
	case errors.Is(err, context.DeadlineExceeded), isDatabaseTimeout(err):
		return DependencyUnavailable(err)
	}
	return Internal(err)
}

// isDatabaseTimeout reports whether PostgreSQL ended a statement or a lock
// wait because of the statement_timeout or lock_timeout that serve mode sets
// from DEPENDENCY_TIMEOUT. Such failures are dependency outages, not
// internal errors.
func isDatabaseTimeout(err error) bool {
	var pgErr *pgconn.PgError
	if !errors.As(err, &pgErr) {
		return false
	}
	// 57014 query_canceled covers statement_timeout; 55P03 lock_not_available
	// covers lock_timeout.
	return pgErr.Code == "57014" || pgErr.Code == "55P03"
}

// ToConnect renders err as a Connect error with a typed ErrorDetail.
func ToConnect(ctx context.Context, err error) *connect.Error {
	e := As(err)
	ce := connect.NewError(e.Code, errors.New(e.Message))
	detail := &mcpav1.ErrorDetail{Reason: e.Reason, TraceId: TraceID(ctx), ReauthenticationUrl: e.ReauthURL}
	if d, derr := connect.NewErrorDetail(detail); derr == nil {
		ce.AddDetail(d)
	}
	return ce
}

// Metadata returns the response metadata of the current request.
func Metadata(ctx context.Context) *mcpav1.Metadata {
	return &mcpav1.Metadata{TraceId: TraceID(ctx)}
}
