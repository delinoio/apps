// SPDX-License-Identifier: FSL-1.1-ALv2

package api

import (
	"errors"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/devbase/v1"
	"google.golang.org/protobuf/types/known/durationpb"
)

// failure is the only error type handlers return on purpose. Any other error
// is a dependency or database failure and surfaces as unavailable.
type failure struct {
	code   connect.Code
	reason pb.ErrorReason
	text   string
	retry  time.Duration
}

func (f *failure) Error() string { return f.text }

func fail(code connect.Code, reason pb.ErrorReason, text string) error {
	return &failure{code: code, reason: reason, text: text}
}
func invalid(text string) error {
	return fail(connect.CodeInvalidArgument, pb.ErrorReason_ERROR_REASON_INVALID_INPUT, text)
}
func unauthenticated() error {
	return fail(connect.CodeUnauthenticated, pb.ErrorReason_ERROR_REASON_UNAUTHENTICATED, "authentication failed")
}
func denied(text string) error {
	return fail(connect.CodePermissionDenied, pb.ErrorReason_ERROR_REASON_PERMISSION_DENIED, text)
}
func notFound() error {
	return fail(connect.CodeNotFound, pb.ErrorReason_ERROR_REASON_NOT_FOUND, "resource not found")
}
func exists(text string) error {
	return fail(connect.CodeAlreadyExists, pb.ErrorReason_ERROR_REASON_ALREADY_EXISTS, text)
}
func idempotencyConflict() error {
	return fail(connect.CodeAlreadyExists, pb.ErrorReason_ERROR_REASON_IDEMPOTENCY_CONFLICT, "idempotency key was used with a different request")
}
func stale() error {
	return fail(connect.CodeAborted, pb.ErrorReason_ERROR_REASON_VERSION_CONFLICT, "version is stale; read the current version and retry")
}
func quota(text string) error {
	return fail(connect.CodeResourceExhausted, pb.ErrorReason_ERROR_REASON_QUOTA_EXHAUSTED, text)
}
func unavailable() error {
	return fail(connect.CodeUnavailable, pb.ErrorReason_ERROR_REASON_DEPENDENCY_UNAVAILABLE, "a dependency is unavailable; the operation was not applied")
}

// render converts an error to a Connect error carrying the typed detail and trace ID.
func render(e error, trace string) *connect.Error {
	var f *failure
	if !errors.As(e, &f) {
		f = unavailable().(*failure)
	}
	ce := connect.NewError(f.code, errors.New(f.text))
	detail := &pb.ErrorDetail{Reason: f.reason, TraceId: trace}
	if f.retry > 0 {
		detail.RetryAfter = durationpb.New(f.retry)
	}
	if d, err := connect.NewErrorDetail(detail); err == nil {
		ce.AddDetail(d)
	}
	ce.Meta().Set("X-Devbase-Trace-Id", trace)
	if f.retry > 0 {
		seconds := int((f.retry + time.Second - 1) / time.Second)
		ce.Meta().Set("Retry-After", itoa(seconds))
	}
	return ce
}

func itoa(n int) string {
	if n <= 0 {
		return "1"
	}
	var b [20]byte
	i := len(b)
	for n > 0 {
		i--
		b[i] = byte('0' + n%10)
		n /= 10
	}
	return string(b[i:])
}
