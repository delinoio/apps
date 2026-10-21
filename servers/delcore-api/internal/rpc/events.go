// SPDX-License-Identifier: FSL-1.1-ALv2

package rpc

import (
	"context"
	"errors"
	"fmt"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/delcore-api/internal/events"
	"github.com/delinoio/apps/servers/delcore-api/internal/metering"
	"github.com/delinoio/apps/servers/delcore-api/internal/telemetry"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	"go.opentelemetry.io/otel/trace"
	"google.golang.org/protobuf/types/known/timestamppb"
)

func (s *Server) eventPolicy() events.Policy {
	return events.Policy{
		Lateness:       time.Duration(s.Config.UsageLatenessSeconds) * time.Second,
		ReportingGrace: time.Duration(s.Config.ReportingGraceSeconds) * time.Second,
		Limits:         metering.Limits{MaxGroupsPerMeter: int64(s.Config.MaxMeterGroups), MaxDistinctValuesPerGroup: int64(s.Config.MaxMeterGroupValues)},
	}
}

// eventError maps domain rejections to Connect codes and typed reasons.
func eventError(err error) error {
	var rejection *events.Error
	if !errors.As(err, &rejection) {
		return err
	}
	switch rejection.Reason {
	case events.ReasonInvalidAttribute:
		return reasoned(connect.CodeInvalidArgument, pb.ErrorReason_ERROR_REASON_INVALID_ATTRIBUTE, rejection.Message)
	case events.ReasonNoMatchingMeter:
		return reasoned(connect.CodeInvalidArgument, pb.ErrorReason_ERROR_REASON_NO_MATCHING_METER, rejection.Message)
	case events.ReasonCardinalityExceeded:
		return reasoned(connect.CodeResourceExhausted, pb.ErrorReason_ERROR_REASON_CARDINALITY_EXCEEDED, rejection.Message)
	case events.ReasonIdentityConflict:
		return reasoned(connect.CodeAlreadyExists, pb.ErrorReason_ERROR_REASON_EVENT_IDENTITY_CONFLICT, rejection.Message)
	case events.ReasonStaleRevision:
		return reasoned(connect.CodeAborted, pb.ErrorReason_ERROR_REASON_STALE_REVISION, rejection.Message)
	case events.ReasonNotFound:
		return reasoned(connect.CodeNotFound, pb.ErrorReason_ERROR_REASON_EVENT_NOT_FOUND, rejection.Message)
	case events.ReasonCancelled:
		return reasoned(connect.CodeFailedPrecondition, pb.ErrorReason_ERROR_REASON_EVENT_CANCELLED, rejection.Message)
	case events.ReasonWindowClosed:
		return reasoned(connect.CodeFailedPrecondition, pb.ErrorReason_ERROR_REASON_REPORTING_WINDOW_CLOSED, rejection.Message)
	case events.ReasonForbidden:
		return forbidden()
	case events.ReasonPeriodUnavailable:
		return precondition(rejection.Message)
	default:
		return invalid(rejection.Message)
	}
}

// attributes converts the flat wire map. A number keeps at most the public
// decimal input length before canonicalization.
func attributes(in map[string]*pb.AttributeValue) (map[string]metering.Value, error) {
	if len(in) > events.MaxAttributes {
		return nil, reasoned(connect.CodeInvalidArgument, pb.ErrorReason_ERROR_REASON_INVALID_ATTRIBUTE, "too many attributes")
	}
	out := make(map[string]metering.Value, len(in))
	for name, value := range in {
		switch v := value.GetValue().(type) {
		case *pb.AttributeValue_Text:
			out[name] = metering.String(v.Text)
		case *pb.AttributeValue_Boolean:
			out[name] = metering.Boolean(v.Boolean)
		case *pb.AttributeValue_Number:
			if len(v.Number) > events.MaxNumberBytes {
				return nil, reasoned(connect.CodeInvalidArgument, pb.ErrorReason_ERROR_REASON_INVALID_ATTRIBUTE, "numeric attributes contain at most 64 characters")
			}
			number, err := metering.Number(v.Number)
			if err != nil {
				return nil, reasoned(connect.CodeInvalidArgument, pb.ErrorReason_ERROR_REASON_INVALID_ATTRIBUTE, "numeric attributes must be decimal strings")
			}
			out[name] = number
		default:
			return nil, reasoned(connect.CodeInvalidArgument, pb.ErrorReason_ERROR_REASON_INVALID_ATTRIBUTE, "attribute values must be flat scalars")
		}
	}
	return out, nil
}

func wireAttributes(in map[string]metering.Value) map[string]*pb.AttributeValue {
	out := make(map[string]*pb.AttributeValue, len(in))
	for name, value := range in {
		switch value.Kind {
		case metering.ValueString:
			out[name] = &pb.AttributeValue{Value: &pb.AttributeValue_Text{Text: value.Text}}
		case metering.ValueBool:
			out[name] = &pb.AttributeValue{Value: &pb.AttributeValue_Boolean{Boolean: value.Text == "true"}}
		case metering.ValueNumber:
			out[name] = &pb.AttributeValue{Value: &pb.AttributeValue_Number{Number: value.Text}}
		}
	}
	return out
}

// IngestEvent persists an event and its processing work before acknowledging
// it. The response does not promise updated aggregates.
func (s *Server) IngestEvent(ctx context.Context, r *connect.Request[pb.IngestEventRequest]) (*connect.Response[pb.IngestEventResponse], error) {
	ctx, span := telemetry.Tracer().Start(ctx, telemetry.SpanEventIngest)
	defer span.End()
	o := &pb.IngestEventResponse{}
	var result eventResult
	response, err := run(s, ctx, r, o, func(c *call) error {
		result.correlation = c.correlation
		if e := c.serviceScope("usage:write"); e != nil {
			return e
		}
		v := r.Msg.Event
		if v == nil || v.Id != "" || !canonical(v.TenantId) || v.ServiceId != c.service || v.OccurredAt == nil || v.OccurredAt.CheckValid() != nil {
			return invalid("invalid event attribution or occurrence")
		}
		values, e := attributes(v.Attributes)
		if e != nil {
			return e
		}
		accepted, e := events.Ingest(c.ctx, c.tx, c.now, s.eventPolicy(), events.Input{TenantID: v.TenantId, ServiceID: c.service, Key: v.EventKey, Type: v.EventType, OccurredAt: v.OccurredAt.AsTime(), Attributes: values})
		if e != nil {
			errors.As(e, &result.rejection)
			return eventError(e)
		}
		result.executed, result.accepted = true, accepted
		o.EventId, o.Revision = accepted.EventID, accepted.Revision
		s.Log.InfoContext(c.ctx, "metering event accepted", "category", "event_ingestion", "correlation_id", c.correlation, "service_id", c.service, "revision", accepted.Revision, "replayed", accepted.Replayed, "matched_meters", accepted.Meters)
		return nil
	})
	outcome := result.outcome(err)
	s.Metrics.EventIngested(outcome, err)
	s.observeEvent(span, result, outcome, err)
	return response, err
}

// CorrectEvent appends a replacement or cancellation with compare-and-set on
// the latest revision. Each new correction writes one audit record attributed
// to the calling service; replays and rejections write none.
func (s *Server) CorrectEvent(ctx context.Context, r *connect.Request[pb.CorrectEventRequest]) (*connect.Response[pb.CorrectEventResponse], error) {
	ctx, span := telemetry.Tracer().Start(ctx, telemetry.SpanEventCorrect)
	defer span.End()
	o := &pb.CorrectEventResponse{}
	var result eventResult
	kind := telemetry.CorrectionInvalid
	response, err := run(s, ctx, r, o, func(c *call) error {
		result.correlation = c.correlation
		if e := c.serviceScope("usage:write"); e != nil {
			return e
		}
		q := r.Msg
		if !canonical(q.TenantId) {
			return invalid("invalid tenant ID")
		}
		correction := events.Correction{TenantID: q.TenantId, ServiceID: c.service, Key: q.EventKey, ExpectedRevision: q.ExpectedRevision, Type: q.EventType}
		switch q.Kind {
		case pb.EventCorrectionKind_EVENT_CORRECTION_KIND_REPLACE:
			correction.Kind = metering.Replace
		case pb.EventCorrectionKind_EVENT_CORRECTION_KIND_CANCEL:
			correction.Kind = metering.Cancel
		default:
			return invalid("correction kind must be replace or cancel")
		}
		kind = telemetry.CorrectionKind(correction.Kind)
		values, e := attributes(q.Attributes)
		if e != nil {
			return e
		}
		correction.Attributes = values
		accepted, e := events.Correct(c.ctx, c.tx, c.now, s.eventPolicy(), correction)
		if e != nil {
			errors.As(e, &result.rejection)
			return eventError(e)
		}
		result.executed, result.accepted = true, accepted
		o.EventId, o.Revision = accepted.EventID, accepted.Revision
		if !accepted.Replayed {
			// Counts and the revision only; never the event key or content.
			if e = c.audit(q.TenantId, "event.correct", fmt.Sprintf("kind=%s revision=%d matched_meters=%d", correction.Kind, accepted.Revision, accepted.Meters)); e != nil {
				return e
			}
		}
		s.Log.InfoContext(c.ctx, "metering correction accepted", "category", "event_correction", "correlation_id", c.correlation, "service_id", c.service, "kind", string(correction.Kind), "revision", accepted.Revision, "replayed", accepted.Replayed, "matched_meters", accepted.Meters)
		return nil
	})
	outcome := result.outcome(err)
	s.Metrics.EventCorrected(kind, outcome, err)
	span.SetAttributes(attribute.String("delcore.correction.kind", string(kind)))
	s.observeEvent(span, result, outcome, err)
	return response, err
}

// eventResult carries the safe facts of one ingestion or correction request
// out of its transaction for metrics and tracing.
type eventResult struct {
	correlation string
	executed    bool
	accepted    events.Accepted
	rejection   *events.Error
}

func (r eventResult) outcome(err error) telemetry.Outcome {
	switch {
	case err != nil:
		return telemetry.OutcomeRejected
	// A request-key replay returns the cached response without running the
	// handler; an event-identity replay runs it but has no new effect.
	case !r.executed || r.accepted.Replayed:
		return telemetry.OutcomeReplayed
	default:
		return telemetry.OutcomeAccepted
	}
}

// observeEvent records cardinality rejections and annotates the span with
// bounded values only: no tenant, event, or service IDs and no attributes.
func (s *Server) observeEvent(span trace.Span, r eventResult, outcome telemetry.Outcome, err error) {
	if err != nil && r.rejection != nil && r.rejection.Reason == events.ReasonCardinalityExceeded {
		switch r.rejection.Limit {
		case events.LimitGroups:
			s.Metrics.CardinalityRejected(telemetry.LimitGroups)
		case events.LimitDistinctValues:
			s.Metrics.CardinalityRejected(telemetry.LimitDistinctValues)
		}
	}
	span.SetAttributes(
		attribute.String("delcore.correlation_id", r.correlation),
		attribute.String("delcore.outcome", string(outcome)),
		attribute.String("delcore.error_reason", telemetry.Reason(err)),
	)
	if r.executed {
		span.SetAttributes(attribute.Int("delcore.event.revision", int(r.accepted.Revision)), attribute.Int("delcore.event.matched_meters", r.accepted.Meters))
	}
	if err != nil {
		span.SetStatus(codes.Error, telemetry.Reason(err))
	}
}

// GetEvent lets the owning service re-read the latest revision before a
// correction. Read or write usage scope is sufficient.
func (s *Server) GetEvent(ctx context.Context, r *connect.Request[pb.GetEventRequest]) (*connect.Response[pb.GetEventResponse], error) {
	o := &pb.GetEventResponse{}
	return run(s, ctx, r, o, func(c *call) error {
		if c.serviceScope("usage:read") != nil {
			if e := c.serviceScope("usage:write"); e != nil {
				return e
			}
		}
		if !canonical(r.Msg.TenantId) {
			return invalid("invalid tenant ID")
		}
		snapshot, e := events.Get(c.ctx, c.tx, r.Msg.TenantId, c.service, r.Msg.EventKey)
		if e != nil {
			return eventError(e)
		}
		o.Event = &pb.Event{Id: snapshot.ID, TenantId: snapshot.TenantID, ServiceId: snapshot.ServiceID, EventKey: snapshot.Key, EventType: snapshot.Type, OccurredAt: timestamppb.New(snapshot.OccurredAt), Attributes: wireAttributes(snapshot.Attributes)}
		o.Revision, o.AppliedRevision, o.Cancelled = snapshot.Revision, snapshot.AppliedRevision, snapshot.Cancelled
		return nil
	})
}
