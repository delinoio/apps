// SPDX-License-Identifier: FSL-1.1-ALv2

package telemetry

import (
	"errors"
	"testing"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/prometheus/client_golang/prometheus"
)

func typedError(reason pb.ErrorReason) error {
	err := connect.NewError(connect.CodeInvalidArgument, errors.New("tenant 0190 event secret"))
	detail, _ := connect.NewErrorDetail(&pb.ErrorDetail{Reason: reason, CorrelationId: "0190"})
	err.AddDetail(detail)
	return err
}

func TestReasonUsesOnlyTypedReasons(t *testing.T) {
	for err, want := range map[error]string{
		nil: "none",
		typedError(pb.ErrorReason_ERROR_REASON_CARDINALITY_EXCEEDED): "cardinality_exceeded",
		typedError(pb.ErrorReason_ERROR_REASON_STALE_REVISION):       "stale_revision",
		typedError(pb.ErrorReason(9999)):                             "other",
		typedError(pb.ErrorReason_ERROR_REASON_UNSPECIFIED):          "other",
		errors.New("raw message with secret"):                        "other",
	} {
		if got := Reason(err); got != want {
			t.Fatalf("Reason(%v) = %q, want %q", err, got, want)
		}
	}
}

func TestLabelsAreBounded(t *testing.T) {
	registry := prometheus.NewRegistry()
	m := New(registry)
	m.MeteringJob(OutcomeFailed, "category-from-a-future-release", time.Millisecond)
	m.MeteringJob(OutcomeApplied, "", time.Millisecond)
	m.OperatorRetry("01900000-0000-7000-8000-000000000001")
	m.OperatorRetry("metering")
	families, err := registry.Gather()
	if err != nil {
		t.Fatal(err)
	}
	seen := map[string]bool{}
	for _, family := range families {
		for _, metric := range family.GetMetric() {
			for _, pair := range metric.GetLabel() {
				seen[family.GetName()+":"+pair.GetName()+"="+pair.GetValue()] = true
			}
		}
	}
	for _, want := range []string{
		"delcore_metering_jobs_total:category=other",
		"delcore_metering_jobs_total:category=none",
		"delcore_operator_job_retries_total:kind=other",
		"delcore_operator_job_retries_total:kind=metering",
	} {
		if !seen[want] {
			t.Fatalf("missing bounded series %s in %v", want, seen)
		}
	}
	for series := range seen {
		if len(series) > 0 && containsID(series) {
			t.Fatalf("identifier became a label value: %s", series)
		}
	}
}

func containsID(s string) bool {
	for i := 0; i+8 <= len(s); i++ {
		if s[i:i+8] == "01900000" {
			return true
		}
	}
	return false
}

func TestNilMetricsRecordNothing(t *testing.T) {
	var m *Metrics
	m.CatalogApplied(OutcomeApplied, time.Second)
	m.EventIngested(OutcomeAccepted, nil)
	m.EventCorrected(CorrectionCancel, OutcomeRejected, errors.New("x"))
	m.CardinalityRejected(LimitGroups)
	m.MeteringJob(OutcomeDeferred, "metering_transient", time.Second)
	m.OperatorRetry("metering")
}
