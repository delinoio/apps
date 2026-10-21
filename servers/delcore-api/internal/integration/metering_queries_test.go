// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package integration

import (
	"context"
	"encoding/base64"
	"errors"
	"fmt"
	"sort"
	"strings"
	"testing"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/delcore-api/internal/events"
	"github.com/jackc/pgx/v5"
	"google.golang.org/protobuf/types/known/timestamppb"
)

type aggregateQuery struct {
	token, tenant, service, meter string
	at                            *timestamppb.Timestamp
	size                          int32
}

func (f *featureFixture) aggregates(q aggregateQuery, after string) (*pb.ListMeterAggregatesResponse, error) {
	response, err := f.client.ListMeterAggregates(context.Background(), req(&pb.ListMeterAggregatesRequest{TenantId: q.tenant, ServiceId: q.service, MeterKey: q.meter, PeriodAt: q.at, Page: &pb.Page{Size: q.size, AfterId: after}}, q.token))
	if err != nil {
		return nil, err
	}
	return response.Msg, nil
}

// allAggregates follows cursors and returns aggregate values by group model.
func (f *featureFixture) allAggregates(t *testing.T, q aggregateQuery) (map[string]*pb.MeterAggregate, int) {
	t.Helper()
	out := map[string]*pb.MeterAggregate{}
	pages, after := 0, ""
	for {
		page, err := f.aggregates(q, after)
		if err != nil {
			t.Fatal(err)
		}
		pages++
		if q.size > 0 && len(page.Aggregates) > int(q.size) {
			t.Fatalf("page exceeded its size: %d", len(page.Aggregates))
		}
		for _, aggregate := range page.Aggregates {
			name := aggregate.GroupValues["model"].GetText()
			if out[name] != nil {
				t.Fatalf("group %q repeated across pages", name)
			}
			out[name] = aggregate
		}
		if page.NextAfterId == "" {
			return out, pages
		}
		after = page.NextAfterId
	}
}

func TestMeterAggregateQueries(t *testing.T) {
	f := newFeatureFixture(t)
	for i, name := range []string{"m1", "m1", "m2", "m3", "m4", "m5"} {
		f.ingest(t, "service", f.alpha, fmt.Sprintf("query-call-%d", i), "call", model(name))
	}
	for i, ms := range []string{"100", "0", "0"} {
		f.ingest(t, "service", f.alpha, fmt.Sprintf("query-sample-%d", i), "sample", map[string]*pb.AttributeValue{"model": attrText("avg"), "ms": attrNumber(ms)})
	}
	f.ingest(t, "service", f.beta, "beta-call", "call", model("m1"))
	f.ingest(t, "other", f.alpha, "rival-ignored", "call", model("m1"))

	// Accepted events are invisible until asynchronous processing applies them.
	listed, err := f.aggregates(aggregateQuery{token: "service", tenant: f.alpha, meter: "calls"}, "")
	if err != nil || len(listed.Aggregates) != 0 {
		t.Fatalf("unprocessed events were listed: %v %v", listed, err)
	}
	f.process(t)

	q := aggregateQuery{token: "owner", tenant: f.alpha, service: f.service, meter: "calls", size: 2}
	groups, pages := f.allAggregates(t, q)
	if pages != 3 || len(groups) != 5 {
		t.Fatalf("pagination returned %d groups in %d pages", len(groups), pages)
	}
	for name, aggregate := range groups {
		want := "1"
		if name == "m1" {
			want = "2"
		}
		if aggregate.Value != want || fmt.Sprint(aggregate.EventCount) != want || aggregate.Sum != "0" || aggregate.DistinctCount != 0 || aggregate.UpdatedAt == nil {
			t.Fatalf("group %s = %v", name, aggregate)
		}
	}
	first, err := f.aggregates(q, "")
	if err != nil {
		t.Fatal(err)
	}
	if first.MeterId == "" || first.Aggregation != pb.MeterAggregation_METER_AGGREGATION_COUNT || !first.PeriodStart.AsTime().Equal(monthStart(f.start)) || !first.PeriodEnd.AsTime().Equal(monthStart(f.start).AddDate(0, 1, 0)) {
		t.Fatalf("aggregate period metadata = %v", first)
	}
	// Services read their own meters without naming the service.
	serviceGroups, _ := f.allAggregates(t, aggregateQuery{token: "service", tenant: f.alpha, meter: "calls"})
	if len(serviceGroups) != 5 || serviceGroups["m1"].Value != "2" {
		t.Fatalf("service view = %v", serviceGroups)
	}
	// The rival's same-named meter is a different meter of its own.
	rivalGroups, _ := f.allAggregates(t, aggregateQuery{token: "other", tenant: f.alpha, meter: "calls"})
	if len(rivalGroups) != 1 || rivalGroups["m1"].Value != "1" {
		t.Fatalf("rival view = %v", rivalGroups)
	}
	average, _ := f.allAggregates(t, aggregateQuery{token: "service", tenant: f.alpha, meter: "latency"})
	if a := average["avg"]; a == nil || a.Value != "33.333333" || a.Sum != "100" || a.EventCount != 3 {
		t.Fatalf("average aggregate = %v", average)
	}
	// Another billing period has no aggregates.
	next := monthStart(time.Now())
	empty, err := f.aggregates(aggregateQuery{token: "owner", tenant: f.alpha, service: f.service, meter: "calls", at: timestamppb.New(next.Add(time.Hour))}, "")
	if err != nil || len(empty.Aggregates) != 0 || !empty.PeriodStart.AsTime().Equal(next) {
		t.Fatalf("next period = %v %v", empty, err)
	}

	stranger := "stranger"
	for label, tc := range map[string]struct {
		q    aggregateQuery
		code connect.Code
	}{
		"non-member":           {aggregateQuery{token: stranger, tenant: f.alpha, service: f.service, meter: "calls"}, connect.CodePermissionDenied},
		"member without role":  {aggregateQuery{token: "member", tenant: f.alpha, service: f.service, meter: "calls"}, connect.CodePermissionDenied},
		"other service meter":  {aggregateQuery{token: "other", tenant: f.alpha, service: f.service, meter: "calls"}, connect.CodePermissionDenied},
		"service without read": {aggregateQuery{token: "noscope", tenant: f.alpha, meter: "calls"}, connect.CodePermissionDenied},
		"unknown own meter":    {aggregateQuery{token: "other", tenant: f.alpha, meter: "latency"}, connect.CodeInvalidArgument},
		"invalid meter key":    {aggregateQuery{token: "service", tenant: f.alpha, meter: "Calls!"}, connect.CodeInvalidArgument},
		"invalid service":      {aggregateQuery{token: "owner", tenant: f.alpha, service: "service", meter: "calls"}, connect.CodeInvalidArgument},
		"invalid tenant":       {aggregateQuery{token: "service", tenant: "tenant", meter: "calls"}, connect.CodeInvalidArgument},
		"oversized page":       {aggregateQuery{token: "service", tenant: f.alpha, meter: "calls", size: 101}, connect.CodeInvalidArgument},
		"invalid period":       {aggregateQuery{token: "service", tenant: f.alpha, meter: "calls", at: &timestamppb.Timestamp{Nanos: -1}}, connect.CodeInvalidArgument},
	} {
		if _, err := f.aggregates(tc.q, ""); connect.CodeOf(err) != tc.code {
			t.Fatalf("%s: %v", label, err)
		}
	}
	// Cursors must decode to group-key text: "AA" is a NUL byte and "_w" is
	// invalid UTF-8, which PostgreSQL text cannot hold.
	for _, cursor := range []string{"!!", strings.Repeat("A", 1<<17), "AA", "_w", base64.RawURLEncoding.EncodeToString([]byte("m\x00"))} {
		if _, err := f.aggregates(aggregateQuery{token: "service", tenant: f.alpha, meter: "calls"}, cursor); connect.CodeOf(err) != connect.CodeInvalidArgument {
			t.Fatalf("cursor accepted: %v", err)
		}
	}
	_, err = f.aggregates(aggregateQuery{token: stranger, tenant: f.alpha, service: f.service, meter: "calls"}, "")
	var ce *connect.Error
	if !errors.As(err, &ce) || ce.Meta().Get("X-Delcore-Correlation-Id") == "" || typedReason(err) != pb.ErrorReason_ERROR_REASON_FORBIDDEN {
		t.Fatalf("query errors lack typed correlation metadata: %v", err)
	}
}

func (f *featureFixture) status(token, tenant string) (*pb.GetMeteringStatusResponse, error) {
	response, err := f.client.GetMeteringStatus(context.Background(), req(&pb.GetMeteringStatusRequest{TenantId: tenant}, token))
	if err != nil {
		return nil, err
	}
	return response.Msg, nil
}

type statusWant struct {
	unapplied, pending, failed, lag int64
	failures                        map[pb.MeteringFailureCategory]int64
	last                            pb.MeteringFailureCategory
}

func expectStatus(t *testing.T, label string, got *pb.GetMeteringStatusResponse, err error, want statusWant) {
	t.Helper()
	if err != nil {
		t.Fatalf("%s: %v", label, err)
	}
	failures := map[pb.MeteringFailureCategory]int64{}
	for _, failure := range got.Failures {
		failures[failure.Category] = failure.Jobs
	}
	if want.failures == nil {
		want.failures = map[pb.MeteringFailureCategory]int64{}
	}
	if got.UnappliedEvents != want.unapplied || got.PendingJobs != want.pending || got.FailedJobs != want.failed || got.LagSeconds != want.lag || fmt.Sprint(failures) != fmt.Sprint(want.failures) || got.LastFailureCategory != want.last {
		t.Fatalf("%s: status = %v, want %+v", label, got, want)
	}
	if (got.PendingJobs > 0) != (got.OldestPendingAt != nil) || (len(got.Failures) > 0) != (got.LastFailureAt != nil) {
		t.Fatalf("%s: timestamps = %v", label, got)
	}
}

func TestMeteringStatusLagAndFailures(t *testing.T) {
	f := newFeatureFixture(t)
	ctx := context.Background()
	accepted := f.start
	first := f.ingest(t, "service", f.alpha, "status-1", "call", model("a"))
	f.ingest(t, "service", f.alpha, "status-2", "call", model("b"))
	f.ingest(t, "service", f.beta, "status-3", "call", model("a"))
	f.ingest(t, "other", f.alpha, "status-rival", "call", model("a"))
	f.setClock(accepted.Add(90 * time.Second))

	got, err := f.status("owner", f.alpha)
	expectStatus(t, "tenant", got, err, statusWant{unapplied: 3, pending: 3, lag: 90})
	if !got.OldestPendingAt.AsTime().Equal(accepted) {
		t.Fatalf("oldest pending = %v", got.OldestPendingAt.AsTime())
	}
	got, err = f.status("service", f.alpha)
	expectStatus(t, "service view", got, err, statusWant{unapplied: 2, pending: 2, lag: 90})
	got, err = f.status("other", f.beta)
	expectStatus(t, "other service view", got, err, statusWant{})
	got, err = f.status("owner", "")
	expectStatus(t, "service-wide", got, err, statusWant{unapplied: 4, pending: 4, lag: 90})
	for label, tc := range map[string]struct {
		token, tenant string
		code          connect.Code
	}{
		"member service-wide": {"member", "", connect.CodePermissionDenied},
		"stranger tenant":     {"stranger", f.alpha, connect.CodePermissionDenied},
		"service-wide token":  {"service", "", connect.CodeInvalidArgument},
		"service no scope":    {"noscope", f.alpha, connect.CodePermissionDenied},
		"operator bad tenant": {"owner", "tenant", connect.CodeInvalidArgument},
	} {
		if _, err := f.status(tc.token, tc.tenant); connect.CodeOf(err) != tc.code {
			t.Fatalf("%s: %v", label, err)
		}
	}

	// Transient failures stay pending with a visible category.
	f.worker.MeteringApplied = func(context.Context, pgx.Tx, events.Applied) error {
		return errors.New("simulated dependency failure")
	}
	f.process(t)
	got, err = f.status("owner", "")
	expectStatus(t, "transient", got, err, statusWant{unapplied: 4, pending: 4, lag: 90, failures: map[pb.MeteringFailureCategory]int64{pb.MeteringFailureCategory_METERING_FAILURE_CATEGORY_TRANSIENT: 4}, last: pb.MeteringFailureCategory_METERING_FAILURE_CATEGORY_TRANSIENT})

	// Permanent failures leave pending work and wait for an operator.
	if _, err = f.db.Pool.Exec(ctx, "UPDATE jobs SET available_at=now()-interval '1 second' WHERE kind='metering' AND state='pending'"); err != nil {
		t.Fatal(err)
	}
	f.worker.MeteringApplied = func(context.Context, pgx.Tx, events.Applied) error {
		return fmt.Errorf("simulated invalid state: %w", events.ErrPermanent)
	}
	f.process(t)
	if _, err = f.db.Pool.Exec(ctx, `INSERT INTO jobs(id,tenant_id,kind,dedup_key,payload,available_at,created_at,updated_at) VALUES($1,$2,'metering',$3,'{}',now()-interval '1 second',$4,$4)`, key(), f.alpha, "invalid-payload", accepted); err != nil {
		t.Fatal(err)
	}
	f.process(t)
	invalidState := pb.MeteringFailureCategory_METERING_FAILURE_CATEGORY_INVALID_STATE
	invalidPayload := pb.MeteringFailureCategory_METERING_FAILURE_CATEGORY_INVALID_PAYLOAD
	got, err = f.status("owner", "")
	expectStatus(t, "failed service-wide", got, err, statusWant{unapplied: 4, failed: 5, failures: map[pb.MeteringFailureCategory]int64{invalidState: 4, invalidPayload: 1}, last: invalidPayload})
	got, err = f.status("owner", f.alpha)
	expectStatus(t, "failed tenant", got, err, statusWant{unapplied: 3, failed: 4, failures: map[pb.MeteringFailureCategory]int64{invalidState: 3, invalidPayload: 1}, last: invalidPayload})
	// The service view excludes work it cannot attribute to its events.
	got, err = f.status("service", f.alpha)
	expectStatus(t, "failed service view", got, err, statusWant{unapplied: 2, failed: 2, failures: map[pb.MeteringFailureCategory]int64{invalidState: 2}, last: invalidState})

	// Operators recover failed work with the existing job interfaces.
	jobs, err := f.client.ListJobs(ctx, req(&pb.ListJobsRequest{Page: &pb.Page{Size: 100}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	var failed []string
	for _, j := range jobs.Msg.Jobs {
		if j.Kind == events.JobKind && j.State == pb.JobState_JOB_STATE_FAILED && j.ErrorCategory == "metering_invalid" {
			failed = append(failed, j.Id)
		}
	}
	sort.Strings(failed)
	if len(failed) != 4 {
		t.Fatalf("failed metering jobs = %v", failed)
	}
	for _, id := range failed {
		if _, err = f.client.RetryJob(ctx, req(&pb.RetryJobRequest{RequestKey: key(), JobId: id, Reason: "fixed metering state"}, "owner")); err != nil {
			t.Fatal(err)
		}
	}
	f.worker.MeteringApplied = nil
	f.process(t)
	got, err = f.status("owner", f.alpha)
	expectStatus(t, "recovered tenant", got, err, statusWant{failed: 1, failures: map[pb.MeteringFailureCategory]int64{invalidPayload: 1}, last: invalidPayload})
	got, err = f.status("service", f.alpha)
	expectStatus(t, "recovered service view", got, err, statusWant{})
	snapshot, err := f.client.GetEvent(ctx, req(&pb.GetEventRequest{TenantId: f.alpha, EventKey: "status-1"}, "service"))
	if err != nil || snapshot.Msg.AppliedRevision != 1 || snapshot.Msg.Event.Id != first.EventId {
		t.Fatalf("recovered event = %v %v", snapshot, err)
	}

	// Exhausted retries are reported with their own category.
	f.ingest(t, "service", f.beta, "status-exhausted", "call", model("c"))
	f.worker.MeteringApplied = func(context.Context, pgx.Tx, events.Applied) error { return errors.New("dependency unavailable") }
	for range f.cfg.RetryLimit {
		if _, err = f.db.Pool.Exec(ctx, "UPDATE jobs SET available_at=now()-interval '1 second' WHERE kind='metering' AND state='pending'"); err != nil {
			t.Fatal(err)
		}
		f.process(t)
	}
	got, err = f.status("owner", f.beta)
	expectStatus(t, "exhausted", got, err, statusWant{unapplied: 1, failed: 1, failures: map[pb.MeteringFailureCategory]int64{pb.MeteringFailureCategory_METERING_FAILURE_CATEGORY_RETRY_EXHAUSTED: 1}, last: pb.MeteringFailureCategory_METERING_FAILURE_CATEGORY_RETRY_EXHAUSTED})
}
