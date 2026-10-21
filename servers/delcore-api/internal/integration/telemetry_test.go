// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package integration

import (
	"bytes"
	"context"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/delcore-api/internal/catalog"
	"github.com/delinoio/apps/servers/delcore-api/internal/config"
	"github.com/delinoio/apps/servers/delcore-api/internal/events"
	"github.com/delinoio/apps/servers/delcore-api/internal/telemetry"
	"github.com/jackc/pgx/v5"
	"github.com/prometheus/client_golang/prometheus"
	"go.opentelemetry.io/otel"
	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/sdk/trace/tracetest"
)

// lockedBuffer collects logs written concurrently by handlers and the worker.
type lockedBuffer struct {
	mu  sync.Mutex
	buf bytes.Buffer
}

func (b *lockedBuffer) Write(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buf.Write(p)
}

func (b *lockedBuffer) String() string {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buf.String()
}

// observation captures logs, metrics, and spans for one test.
type observation struct {
	logs     *lockedBuffer
	logger   *slog.Logger
	registry *prometheus.Registry
	metrics  *telemetry.Metrics
	spans    *tracetest.SpanRecorder
}

func observe(t *testing.T) *observation {
	t.Helper()
	o := &observation{logs: &lockedBuffer{}, registry: prometheus.NewRegistry(), spans: tracetest.NewSpanRecorder()}
	o.logger = slog.New(slog.NewJSONHandler(o.logs, &slog.HandlerOptions{Level: slog.LevelDebug}))
	o.metrics = telemetry.New(o.registry)
	previous := otel.GetTracerProvider()
	provider := sdktrace.NewTracerProvider(sdktrace.WithSpanProcessor(o.spans))
	otel.SetTracerProvider(provider)
	t.Cleanup(func() {
		otel.SetTracerProvider(previous)
		provider.Shutdown(context.Background())
	})
	return o
}

// counter returns the value of the counter series with exactly these labels.
func (o *observation) counter(t *testing.T, name string, labels map[string]string) float64 {
	t.Helper()
	families, err := o.registry.Gather()
	if err != nil {
		t.Fatal(err)
	}
	for _, family := range families {
		if family.GetName() != name {
			continue
		}
		for _, metric := range family.GetMetric() {
			matched := len(metric.GetLabel()) == len(labels)
			for _, pair := range metric.GetLabel() {
				matched = matched && labels[pair.GetName()] == pair.GetValue()
			}
			if matched {
				return metric.GetCounter().GetValue()
			}
		}
	}
	return 0
}

// assertBounded checks that every series uses only bounded label names and that
// no label value, log line, or span attribute contains a sensitive value.
// Logs may name values in loggable, such as the calling service ID.
func (o *observation) assertBounded(t *testing.T, loggable []string, sensitive ...string) {
	t.Helper()
	allowed := map[string]bool{"outcome": true, "reason": true, "kind": true, "limit": true, "category": true}
	families, err := o.registry.Gather()
	if err != nil {
		t.Fatal(err)
	}
	for _, family := range families {
		for _, metric := range family.GetMetric() {
			for _, pair := range metric.GetLabel() {
				if !allowed[pair.GetName()] {
					t.Fatalf("%s uses unbounded label %s", family.GetName(), pair.GetName())
				}
				for _, value := range sensitive {
					if strings.Contains(pair.GetValue(), value) {
						t.Fatalf("%s label %s exposes a sensitive value", family.GetName(), pair.GetName())
					}
				}
			}
		}
	}
	logs := o.logs.String()
	for _, value := range sensitive {
		if strings.Contains(logs, value) && !slices.Contains(loggable, value) {
			t.Fatalf("logs expose sensitive value %q", value)
		}
	}
	for _, span := range o.spans.Ended() {
		for _, attribute := range span.Attributes() {
			for _, value := range sensitive {
				if strings.Contains(attribute.Value.Emit(), value) {
					t.Fatalf("span %s attribute %s exposes a sensitive value", span.Name(), attribute.Key)
				}
			}
		}
		for _, event := range span.Events() {
			for _, attribute := range event.Attributes {
				for _, value := range sensitive {
					if strings.Contains(attribute.Value.Emit(), value) {
						t.Fatalf("span %s event exposes a sensitive value", span.Name())
					}
				}
			}
		}
	}
}

func (o *observation) spanCount(name, outcome string) int {
	n := 0
	for _, span := range o.spans.Ended() {
		if span.Name() != name {
			continue
		}
		for _, attribute := range span.Attributes() {
			if attribute.Key == "delcore.outcome" && attribute.Value.AsString() == outcome {
				n++
			}
		}
	}
	return n
}

type auditRow struct{ actor, action, reason, tenant string }

func auditRows(t *testing.T, f *meterFixture, query string, args ...any) []auditRow {
	t.Helper()
	rows, err := f.db.Pool.Query(context.Background(), query, args...)
	if err != nil {
		t.Fatal(err)
	}
	collected, err := pgx.CollectRows(rows, func(row pgx.CollectableRow) (auditRow, error) {
		var a auditRow
		err := row.Scan(&a.actor, &a.action, &a.reason, &a.tenant)
		return a, err
	})
	if err != nil {
		t.Fatal(err)
	}
	return collected
}

func TestMeteringTelemetryRedactionBoundedLabelsAndAudit(t *testing.T) {
	o := observe(t)
	f := newObservedMeterFixture(t, func(c *config.Config) { c.MaxMeterGroups, c.MaxMeterGroupValues = 2, 2 }, o.logger, o.metrics)
	ctx := context.Background()
	occurred := time.Now().UTC().Add(-time.Minute)
	sensitive := []string{f.tenant, f.service, "secret-key-one", "secret-key-two", "secret-key-three", "secret-key-four", "secret-key-five", "secret-region-a", "secret-region-b", "secret-region-c", "secret-user-1", "secret-user-2", "secret-user-3"}
	event := func(region, user string) map[string]*pb.AttributeValue {
		return request{region: region, outcome: "success", user: user, bytes: "7", latency: "1"}.wire()
	}
	ingest := func(eventKey string, attributes map[string]*pb.AttributeValue) error {
		_, err := f.ingest("service", eventKey, "request", occurred, attributes)
		return err
	}
	if err := ingest("secret-key-one", event("secret-region-a", "secret-user-1")); err != nil {
		t.Fatal(err)
	}
	if err := ingest("secret-key-one", event("secret-region-a", "secret-user-1")); err != nil {
		t.Fatal("identical replay rejected", err)
	}
	if err := ingest("secret-key-two", event("secret-region-a", "secret-user-2")); err != nil {
		t.Fatal(err)
	}
	if err := ingest("secret-key-three", event("secret-region-a", "secret-user-3")); reasonOf(err) != pb.ErrorReason_ERROR_REASON_CARDINALITY_EXCEEDED {
		t.Fatalf("distinct-value bound not enforced: %v", err)
	}
	if err := ingest("secret-key-four", event("secret-region-b", "secret-user-1")); err != nil {
		t.Fatal(err)
	}
	if err := ingest("secret-key-five", event("secret-region-c", "secret-user-1")); reasonOf(err) != pb.ErrorReason_ERROR_REASON_CARDINALITY_EXCEEDED {
		t.Fatalf("group bound not enforced: %v", err)
	}
	if _, err := f.correct("secret-key-one", 1, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_REPLACE, "request", event("secret-region-b", "secret-user-1")); err != nil {
		t.Fatal(err)
	}
	if _, err := f.correct("secret-key-one", 1, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_CANCEL, "", nil); reasonOf(err) != pb.ErrorReason_ERROR_REASON_STALE_REVISION {
		t.Fatalf("stale correction accepted: %v", err)
	}
	f.process(t)
	// A permanent processing failure is audited as system work and recovered
	// by an audited operator retry.
	if _, err := f.correct("secret-key-two", 1, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_CANCEL, "", nil); err != nil {
		t.Fatal(err)
	}
	f.worker.MeteringApplied = func(context.Context, pgx.Tx, events.Applied) error {
		return fmt.Errorf("simulated inconsistent state: %w", events.ErrPermanent)
	}
	f.process(t)
	var failed string
	if err := f.db.Pool.QueryRow(ctx, "SELECT id::text FROM jobs WHERE kind='metering' AND state='failed'").Scan(&failed); err != nil {
		t.Fatal(err)
	}
	if _, err := f.client.RetryJob(ctx, req(&pb.RetryJobRequest{RequestKey: key(), JobId: failed, Reason: "repaired metering state"}, "owner")); err != nil {
		t.Fatal(err)
	}
	f.worker.MeteringApplied = nil
	f.process(t)

	for _, c := range []struct {
		name   string
		labels map[string]string
		want   float64
	}{
		{"delcore_metering_ingestions_total", map[string]string{"outcome": "accepted", "reason": "none"}, 3},
		{"delcore_metering_ingestions_total", map[string]string{"outcome": "replayed", "reason": "none"}, 1},
		{"delcore_metering_ingestions_total", map[string]string{"outcome": "rejected", "reason": "cardinality_exceeded"}, 2},
		{"delcore_metering_cardinality_rejections_total", map[string]string{"limit": "groups"}, 1},
		{"delcore_metering_cardinality_rejections_total", map[string]string{"limit": "distinct_values"}, 1},
		{"delcore_metering_corrections_total", map[string]string{"kind": "replace", "outcome": "accepted", "reason": "none"}, 1},
		{"delcore_metering_corrections_total", map[string]string{"kind": "cancel", "outcome": "rejected", "reason": "stale_revision"}, 1},
		{"delcore_metering_corrections_total", map[string]string{"kind": "cancel", "outcome": "accepted", "reason": "none"}, 1},
		{"delcore_metering_jobs_total", map[string]string{"outcome": "applied", "category": "none"}, 5},
		{"delcore_metering_jobs_total", map[string]string{"outcome": "failed", "category": "metering_invalid"}, 1},
		{"delcore_operator_job_retries_total", map[string]string{"kind": "metering"}, 1},
	} {
		if got := o.counter(t, c.name, c.labels); got != c.want {
			t.Fatalf("%s%v = %v, want %v", c.name, c.labels, got, c.want)
		}
	}
	if o.spanCount(telemetry.SpanEventIngest, "accepted") != 3 || o.spanCount(telemetry.SpanEventIngest, "rejected") != 2 || o.spanCount(telemetry.SpanEventCorrect, "accepted") != 2 || o.spanCount(telemetry.SpanMeteringProcess, "failed") != 1 || o.spanCount(telemetry.SpanMeteringProcess, "applied") != 5 {
		t.Fatal("missing ingestion, correction, or processing spans")
	}
	o.assertBounded(t, []string{f.service}, sensitive...)

	// Ingestion writes no audit rows; corrections, permanent failures, and
	// operator recovery write one each, attributed to their actor.
	if rows := auditRows(t, f, "SELECT actor_id,action,reason,COALESCE(tenant_id::text,'') FROM audit WHERE action LIKE 'event.%' ORDER BY id"); len(rows) != 2 ||
		rows[0].actor != "service:"+f.service || rows[0].tenant != f.tenant || rows[0].reason != "kind=replace revision=2 matched_meters=6" || rows[1].reason != "kind=cancel revision=2 matched_meters=0" {
		t.Fatalf("correction audit = %+v", rows)
	}
	if rows := auditRows(t, f, "SELECT actor_id,action,reason,COALESCE(tenant_id::text,'') FROM audit WHERE action='metering.failed'"); len(rows) != 1 || rows[0].actor != "system:metering" || rows[0].tenant != f.tenant || rows[0].reason != "category=metering_invalid attempts=1" {
		t.Fatalf("permanent failure audit = %+v", rows)
	}
	if rows := auditRows(t, f, "SELECT actor_id,action,reason,COALESCE(tenant_id::text,'') FROM audit WHERE action='job.retry'"); len(rows) != 1 || !strings.HasPrefix(rows[0].actor, "user:") || rows[0].reason != "repaired metering state" {
		t.Fatalf("operator recovery audit = %+v", rows)
	}
	all := auditRows(t, f, "SELECT actor_id,action,reason,COALESCE(tenant_id::text,'') FROM audit")
	for _, row := range all {
		for _, value := range sensitive[2:] {
			if strings.Contains(row.reason, value) {
				t.Fatalf("audit reason exposes %q", value)
			}
		}
	}
}

func TestCatalogReconciliationTelemetryAndAudit(t *testing.T) {
	o := observe(t)
	db, client, _ := setup(t)
	ctx := context.Background()
	root := catalogFixture(t)
	apply := func() (catalog.Result, error) { return catalog.Sync(ctx, db, root, o.logger, o.metrics) }
	audits := func() []auditRow {
		t.Helper()
		return auditRows(t, &meterFixture{db: db}, "SELECT actor_id,action,reason,COALESCE(tenant_id::text,'') FROM audit WHERE action='catalog.reconcile' ORDER BY id")
	}
	first, err := apply()
	if err != nil || !first.Changed || first.EntitiesCreated == 0 || first.VersionsCreated == 0 {
		t.Fatalf("first application = %+v, %v", first, err)
	}
	second, err := apply()
	if err != nil || second.Changed || second.EntitiesCreated != 0 || second.VersionsCreated != 0 {
		t.Fatalf("unchanged application = %+v, %v", second, err)
	}
	if rows := audits(); len(rows) != 1 || rows[0].actor != "system:catalog" || rows[0].tenant != "" || !strings.HasPrefix(rows[0].reason, fmt.Sprintf("files=%d ", first.Files)) {
		t.Fatalf("catalog audit = %+v", rows)
	}
	// Restoring operator drift is a change and is audited once more.
	if _, err = client.PutService(ctx, req(&pb.PutServiceRequest{RequestKey: key(), Reason: "test operator drift", Service: &pb.Service{Key: "example-api", Name: "Drifted", ClientId: "example-api-local", Capabilities: []string{"execute"}, Scopes: []string{"authorize"}}}, "owner")); err != nil {
		t.Fatal(err)
	}
	restored, err := apply()
	if err != nil || !restored.Changed || restored.EntitiesCreated != 0 {
		t.Fatalf("drift restoration = %+v, %v", restored, err)
	}
	if err = os.WriteFile(filepath.Join(root, "broken.yaml"), []byte("kind: ["), 0600); err != nil {
		t.Fatal(err)
	}
	if _, err = apply(); err == nil {
		t.Fatal("invalid catalog accepted")
	}
	if rows := audits(); len(rows) != 2 {
		t.Fatalf("catalog audit rows = %d, want one per changing application", len(rows))
	}
	for outcome, want := range map[string]float64{"applied": 2, "unchanged": 1, "rejected": 1} {
		if got := o.counter(t, "delcore_catalog_reconciliations_total", map[string]string{"outcome": outcome}); got != want {
			t.Fatalf("catalog %s = %v, want %v", outcome, got, want)
		}
	}
	if o.spanCount(telemetry.SpanCatalogReconcile, "applied") != 2 || o.spanCount(telemetry.SpanCatalogReconcile, "rejected") != 1 {
		t.Fatal("catalog spans missing")
	}
	if !strings.Contains(o.logs.String(), first.Correlation) {
		t.Fatal("catalog log lacks its correlation ID")
	}
	o.assertBounded(t, nil, "example-api-local")
}

// TestCatalogReconciliationDetectsDelimiterShiftedChanges proves that the
// change fingerprint keeps field boundaries: moving a comma between two
// adjacent columns changes catalog state and must be reported and audited.
func TestCatalogReconciliationDetectsDelimiterShiftedChanges(t *testing.T) {
	o := observe(t)
	db, _, _ := setup(t)
	ctx := context.Background()
	root := catalogFixture(t)
	setCatalogText(t, root, "01-service.yaml", strings.Replace(catalogText(t, root, "01-service.yaml"), "name: Example API", `name: "Example, API"`, 1))
	if _, err := catalog.Sync(ctx, db, root, o.logger, o.metrics); err != nil {
		t.Fatal(err)
	}
	// Drift whose delimiter-joined text equals the declared state.
	if _, err := db.Pool.Exec(ctx, "UPDATE services SET name='Example',client_id=' API,example-api-local' WHERE client_id='example-api-local'"); err != nil {
		t.Fatal(err)
	}
	restored, err := catalog.Sync(ctx, db, root, o.logger, o.metrics)
	if err != nil || !restored.Changed {
		t.Fatalf("delimiter-shifted restoration = %+v, %v", restored, err)
	}
	var name string
	if err = db.Pool.QueryRow(ctx, "SELECT name FROM services WHERE client_id='example-api-local'").Scan(&name); err != nil || name != "Example, API" {
		t.Fatalf("restored service name = %q, %v", name, err)
	}
	if rows := auditRows(t, &meterFixture{db: db}, "SELECT actor_id,action,reason,COALESCE(tenant_id::text,'') FROM audit WHERE action='catalog.reconcile'"); len(rows) != 2 {
		t.Fatalf("catalog audit rows = %d, want one per changing application", len(rows))
	}
}
