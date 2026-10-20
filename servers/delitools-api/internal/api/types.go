// SPDX-License-Identifier: FSL-1.1-ALv2
package api

import (
	"connectrpc.com/connect"
	"context"
	"errors"
	core "github.com/delinoio/apps/protos/delcore/v1"
	pb "github.com/delinoio/apps/protos/delitools/v1"
	"github.com/google/uuid"
	"strings"
	"time"
)

const Retention = 30 * 24 * time.Hour
const ExtractionModel = "openai/gpt-6-luna"

func ID() string { return uuid.Must(uuid.NewV7()).String() }
func validID(s string) bool {
	u, e := uuid.Parse(s)
	return e == nil && u.Version() == 7 && u.String() == s
}
func capability(c pb.Capability) string {
	switch c {
	case pb.Capability_CAPABILITY_TRANSLATOR:
		return "translator"
	case pb.Capability_CAPABILITY_EXTRACTOR:
		return "extractor"
	}
	return ""
}
func fault(reason pb.ErrorReason, id string) error {
	code := connect.CodeInternal
	switch reason {
	case pb.ErrorReason_ERROR_REASON_INVALID_INPUT, pb.ErrorReason_ERROR_REASON_INVALID_SCHEMA, pb.ErrorReason_ERROR_REASON_INVALID_MODEL:
		code = connect.CodeInvalidArgument
	case pb.ErrorReason_ERROR_REASON_UNAUTHENTICATED:
		code = connect.CodeUnauthenticated
	case pb.ErrorReason_ERROR_REASON_FORBIDDEN:
		code = connect.CodePermissionDenied
	case pb.ErrorReason_ERROR_REASON_BILLING_BLOCKED, pb.ErrorReason_ERROR_REASON_PRICE_UNAVAILABLE, pb.ErrorReason_ERROR_REASON_INPUT_CONFLICT, pb.ErrorReason_ERROR_REASON_EXTRACTION_IMPOSSIBLE:
		code = connect.CodeFailedPrecondition
	case pb.ErrorReason_ERROR_REASON_LIMIT:
		code = connect.CodeResourceExhausted
	case pb.ErrorReason_ERROR_REASON_PROCESSING:
		code = connect.CodeAborted
	case pb.ErrorReason_ERROR_REASON_ALREADY_PROCESSED:
		code = connect.CodeAlreadyExists
	case pb.ErrorReason_ERROR_REASON_UNKNOWN_OUTCOME:
		code = connect.CodeUnknown
	case pb.ErrorReason_ERROR_REASON_INVALID_OUTPUT:
		code = connect.CodeDataLoss
	case pb.ErrorReason_ERROR_REASON_PROVIDER:
		code = connect.CodeUnavailable
	case pb.ErrorReason_ERROR_REASON_TIMEOUT:
		code = connect.CodeDeadlineExceeded
	case pb.ErrorReason_ERROR_REASON_CANCELED:
		code = connect.CodeCanceled
	}
	e := connect.NewError(code, errors.New(strings.ToLower(strings.ReplaceAll(strings.TrimPrefix(reason.String(), "ERROR_REASON_"), "_", " "))))
	d, _ := connect.NewErrorDetail(&pb.ErrorDetail{Reason: reason, RequestId: id})
	e.AddDetail(d)
	e.Meta().Set("X-DeliTools-Request-Id", id)
	return e
}

func storageFault(err error, id string) error {
	switch {
	case errors.Is(err, context.Canceled):
		return fault(pb.ErrorReason_ERROR_REASON_CANCELED, id)
	case errors.Is(err, context.DeadlineExceeded):
		return fault(pb.ErrorReason_ERROR_REASON_TIMEOUT, id)
	default:
		return fault(pb.ErrorReason_ERROR_REASON_INTERNAL, id)
	}
}

type Authority interface {
	User(context.Context, string) (string, error)
	Operator(context.Context, string) (string, error)
	Check(context.Context, string, string, pb.Capability, core.ConsumerAuthorization, bool) error
	Report(context.Context, *core.Usage, string) error
}
type Counts struct {
	Input      uint64 `json:"input"`
	Output     uint64 `json:"output"`
	CacheRead  uint64 `json:"cache_read"`
	CacheWrite uint64 `json:"cache_write"`
}
type Tier struct {
	Cost string  `json:"cost"`
	Min  uint64  `json:"min"`
	Max  *uint64 `json:"max,omitempty"`
}
type Price struct {
	Input           string `json:"input"`
	Output          string `json:"output"`
	CacheRead       string `json:"input_cache_read"`
	CacheWrite      string `json:"input_cache_write"`
	InputTiers      []Tier `json:"input_tiers,omitempty"`
	OutputTiers     []Tier `json:"output_tiers,omitempty"`
	CacheReadTiers  []Tier `json:"input_cache_read_tiers,omitempty"`
	CacheWriteTiers []Tier `json:"input_cache_write_tiers,omitempty"`
}
type Result struct {
	Content      string
	Usage        *Counts
	GenerationID string
	Transient    bool
	Unknown      bool
}
type Gateway interface {
	Catalog(context.Context) (map[string]Price, error)
	Execute(context.Context, string, string, map[string]any, func(string) error) (Result, error)
	Lookup(context.Context, string, string) (*Counts, error)
}
