// SPDX-License-Identifier: FSL-1.1-ALv2

// Package telemetry owns the bounded Prometheus metrics and trace names of
// catalog reconciliation, event ingestion, corrections, asynchronous metering
// processing, and operator recovery. Every label value comes from a fixed
// enum or allowlist; tenant, user, event, service, and provider identifiers
// and raw event content never become label or span attribute values. A nil
// *Metrics records nothing, so tests and tools can omit it.
package telemetry

import (
	"errors"
	"strings"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/prometheus/client_golang/prometheus"
	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/trace"
)

// TracerName identifies Delcore spans.
const TracerName = "github.com/delinoio/apps/servers/delcore-api"

// Span names. They are stable operation names, never request data.
const (
	SpanCatalogReconcile = "delcore.catalog.reconcile"
	SpanEventIngest      = "delcore.event.ingest"
	SpanEventCorrect     = "delcore.event.correct"
	SpanMeteringProcess  = "delcore.metering.process"
)

// Tracer returns the Delcore tracer from the global provider at call time, so
// the provider configured at startup or by a test applies.
func Tracer() trace.Tracer { return otel.Tracer(TracerName) }

// Outcome is a bounded application outcome label.
type Outcome string

const (
	// OutcomeApplied: a catalog application changed state, or metering work
	// materialized an event transition.
	OutcomeApplied Outcome = "applied"
	// OutcomeUnchanged: a catalog application found the declared state.
	OutcomeUnchanged Outcome = "unchanged"
	// OutcomeRejected: validation, admission, or the database rejected the request.
	OutcomeRejected Outcome = "rejected"
	// OutcomeAccepted: a new event or correction was durably accepted.
	OutcomeAccepted Outcome = "accepted"
	// OutcomeReplayed: a repeated request returned its earlier result without effects.
	OutcomeReplayed Outcome = "replayed"
	// OutcomeDeferred: metering work failed transiently and waits for a retry.
	OutcomeDeferred Outcome = "deferred"
	// OutcomeFailed: metering work failed permanently and waits for an operator.
	OutcomeFailed Outcome = "failed"
)

// LimitKind names the cardinality bound that rejected an event.
type LimitKind string

const (
	LimitGroups         LimitKind = "groups"
	LimitDistinctValues LimitKind = "distinct_values"
)

// CorrectionKind is the bounded correction label.
type CorrectionKind string

const (
	CorrectionReplace CorrectionKind = "replace"
	CorrectionCancel  CorrectionKind = "cancel"
	CorrectionInvalid CorrectionKind = "invalid"
)

// ReasonNone labels a successful request.
const ReasonNone = "none"

// Reason returns the bounded typed error reason of a Delcore RPC error, such
// as "cardinality_exceeded". It never includes the error message.
func Reason(err error) string {
	if err == nil {
		return ReasonNone
	}
	var ce *connect.Error
	if errors.As(err, &ce) {
		for _, detail := range ce.Details() {
			value, e := detail.Value()
			if e != nil {
				continue
			}
			if typed, ok := value.(*pb.ErrorDetail); ok {
				if _, known := pb.ErrorReason_name[int32(typed.Reason)]; known && typed.Reason != pb.ErrorReason_ERROR_REASON_UNSPECIFIED {
					return strings.ToLower(strings.TrimPrefix(typed.Reason.String(), "ERROR_REASON_"))
				}
			}
		}
	}
	return "other"
}

// Job failure categories recorded for metering work. Other values map to "other".
var meteringCategories = map[string]bool{"": true, "metering_transient": true, "invalid_payload": true, "metering_invalid": true, "retry_exhausted": true}

// Job kinds that operators can retry. Other values map to "other".
var jobKinds = map[string]bool{"email": true, "webhook": true, "metering": true, "topup": true, "auto_topup": true, "settlement": true, "subscription": true, "cancel": true}

func bounded(value string, allowed map[string]bool) string {
	if allowed[value] {
		if value == "" {
			return ReasonNone
		}
		return value
	}
	return "other"
}

// Metrics holds the counters and histograms added for catalog and metering
// operations. Gauges for backlog, failures, and lag stay in the server
// process loop, which samples them from PostgreSQL.
type Metrics struct {
	catalogApplications *prometheus.CounterVec
	catalogDuration     prometheus.Histogram
	ingestions          *prometheus.CounterVec
	corrections         *prometheus.CounterVec
	cardinality         *prometheus.CounterVec
	meteringJobs        *prometheus.CounterVec
	meteringDuration    *prometheus.HistogramVec
	operatorRetries     *prometheus.CounterVec
}

// New registers the metrics with reg.
func New(reg prometheus.Registerer) *Metrics {
	m := &Metrics{
		catalogApplications: prometheus.NewCounterVec(prometheus.CounterOpts{Name: "delcore_catalog_reconciliations_total", Help: "Development catalog applications by outcome."}, []string{"outcome"}),
		catalogDuration:     prometheus.NewHistogram(prometheus.HistogramOpts{Name: "delcore_catalog_reconciliation_duration_seconds", Help: "Duration of development catalog applications.", Buckets: prometheus.DefBuckets}),
		ingestions:          prometheus.NewCounterVec(prometheus.CounterOpts{Name: "delcore_metering_ingestions_total", Help: "Event ingestion requests by outcome and typed error reason."}, []string{"outcome", "reason"}),
		corrections:         prometheus.NewCounterVec(prometheus.CounterOpts{Name: "delcore_metering_corrections_total", Help: "Event correction requests by kind, outcome, and typed error reason."}, []string{"kind", "outcome", "reason"}),
		cardinality:         prometheus.NewCounterVec(prometheus.CounterOpts{Name: "delcore_metering_cardinality_rejections_total", Help: "Events rejected by a cardinality admission bound."}, []string{"limit"}),
		meteringJobs:        prometheus.NewCounterVec(prometheus.CounterOpts{Name: "delcore_metering_jobs_total", Help: "Metering job attempts by outcome and failure category."}, []string{"outcome", "category"}),
		meteringDuration:    prometheus.NewHistogramVec(prometheus.HistogramOpts{Name: "delcore_metering_processing_duration_seconds", Help: "Duration of metering job attempts by outcome.", Buckets: prometheus.DefBuckets}, []string{"outcome"}),
		operatorRetries:     prometheus.NewCounterVec(prometheus.CounterOpts{Name: "delcore_operator_job_retries_total", Help: "Failed jobs returned to processing by an operator, by job kind."}, []string{"kind"}),
	}
	reg.MustRegister(m.catalogApplications, m.catalogDuration, m.ingestions, m.corrections, m.cardinality, m.meteringJobs, m.meteringDuration, m.operatorRetries)
	return m
}

// CatalogApplied records one development catalog application.
func (m *Metrics) CatalogApplied(outcome Outcome, elapsed time.Duration) {
	if m == nil {
		return
	}
	m.catalogApplications.WithLabelValues(string(outcome)).Inc()
	m.catalogDuration.Observe(elapsed.Seconds())
}

// EventIngested records one IngestEvent result.
func (m *Metrics) EventIngested(outcome Outcome, err error) {
	if m == nil {
		return
	}
	m.ingestions.WithLabelValues(string(outcome), Reason(err)).Inc()
}

// EventCorrected records one CorrectEvent result.
func (m *Metrics) EventCorrected(kind CorrectionKind, outcome Outcome, err error) {
	if m == nil {
		return
	}
	m.corrections.WithLabelValues(string(kind), string(outcome), Reason(err)).Inc()
}

// CardinalityRejected records an event rejected by an admission bound.
func (m *Metrics) CardinalityRejected(limit LimitKind) {
	if m == nil {
		return
	}
	m.cardinality.WithLabelValues(string(limit)).Inc()
}

// MeteringJob records one metering job attempt.
func (m *Metrics) MeteringJob(outcome Outcome, category string, elapsed time.Duration) {
	if m == nil {
		return
	}
	m.meteringJobs.WithLabelValues(string(outcome), bounded(category, meteringCategories)).Inc()
	m.meteringDuration.WithLabelValues(string(outcome)).Observe(elapsed.Seconds())
}

// OperatorRetry records a failed job returned to processing by an operator.
func (m *Metrics) OperatorRetry(kind string) {
	if m == nil {
		return
	}
	m.operatorRetries.WithLabelValues(bounded(kind, jobKinds)).Inc()
}
