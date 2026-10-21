// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package integration

import (
	"context"
	"errors"
	"io"
	"log/slog"
	"strings"
	"testing"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/protos/delcore/v1/delcorev1connect"
	"github.com/delinoio/apps/servers/delcore-api/internal/accounting"
	"github.com/delinoio/apps/servers/delcore-api/internal/config"
	"github.com/delinoio/apps/servers/delcore-api/internal/events"
	"github.com/delinoio/apps/servers/delcore-api/internal/postgres"
	"github.com/delinoio/apps/servers/delcore-api/internal/worker"
	"github.com/jackc/pgx/v5"
	"google.golang.org/protobuf/types/known/timestamppb"
)

// quantityEvent expresses a former quantity report on an untyped meter under
// the catalog compatibility rule: the event type is the meter key and the
// quantity is the numeric "quantity" attribute.
func quantityEvent(tenant, service, meter, eventKey, quantity string, at time.Time) *pb.IngestEventRequest {
	return &pb.IngestEventRequest{RequestKey: key(), Event: &pb.Event{TenantId: tenant, ServiceId: service, EventKey: eventKey, EventType: meter, OccurredAt: timestamppb.New(at), Attributes: map[string]*pb.AttributeValue{events.CompatibilityAttribute: num(quantity)}}}
}

// processMetering drains ready metering work, including its rating step.
func processMetering(t *testing.T, w *worker.Worker) {
	t.Helper()
	if _, err := w.ProcessMetering(context.Background(), 10000); err != nil {
		t.Fatal(err)
	}
}

// ratingFixture is a tenant with a recorded billing period, a typed grouped
// sum meter, typed non-additive meters, and an untyped compatibility meter.
type ratingFixture struct {
	db      *postgres.Store
	client  delcorev1connect.DelcoreServiceClient
	cfg     config.Config
	w       *worker.Worker
	plan    string
	user    string
	tenant  string
	service string
	start   time.Time
	end     time.Time
}

func newRatingFixture(t *testing.T, start, end time.Time, postpaid bool) *ratingFixture {
	t.Helper()
	db, client, cfg := setup(t)
	ctx := context.Background()
	f := &ratingFixture{db: db, client: client, cfg: cfg, start: start, end: end}
	f.w = &worker.Worker{DB: db, Config: cfg, Log: slog.New(slog.NewTextHandler(io.Discard, nil))}
	f.user, f.tenant = bootstrap(t, client)
	if err := db.Pool.QueryRow(ctx, "SELECT plan_id::text FROM tenants WHERE id=$1", f.tenant).Scan(&f.plan); err != nil {
		t.Fatal(err)
	}
	service, err := client.PutService(ctx, req(&pb.PutServiceRequest{RequestKey: key(), Reason: "rated service", Service: &pb.Service{Key: "rated", Name: "rated", ClientId: "service", Capabilities: []string{"execute"}, Scopes: []string{"authorize", "usage:write", "usage:read"}}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	f.service = service.Msg.Service.Id
	for _, m := range []*pb.Meter{
		{Name: "tokens", EventType: "completion", Aggregation: pb.MeterAggregation_METER_AGGREGATION_SUM, ValueAttribute: "tokens", GroupBy: []string{"model"}},
		{Name: "seats", EventType: "seat", Aggregation: pb.MeterAggregation_METER_AGGREGATION_MAX, ValueAttribute: "seats"},
		{Name: "latency", EventType: "probe", Aggregation: pb.MeterAggregation_METER_AGGREGATION_AVG, ValueAttribute: "ms"},
		{Name: "unpriced", EventType: "ping", Aggregation: pb.MeterAggregation_METER_AGGREGATION_COUNT},
		{Name: "units"},
	} {
		m.ServiceId = f.service
		if _, err = client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "rated meter", Meter: m}, "owner")); err != nil {
			t.Fatal(err)
		}
	}
	if _, err = db.Pool.Exec(ctx, "INSERT INTO billing_periods(tenant_id,starts_at,ends_at,postpaid) VALUES($1,$2,$3,$4)", f.tenant, start, end, postpaid); err != nil {
		t.Fatal(err)
	}
	if _, err = db.Pool.Exec(ctx, "UPDATE tenants SET period_start=$2,period_end=$3 WHERE id=$1", f.tenant, start, end); err != nil {
		t.Fatal(err)
	}
	return f
}

// price creates a usage price on the default plan and entitles the tenant to
// it from at.
func (f *ratingFixture) price(t *testing.T, meter, unit, included string, at time.Time) string {
	t.Helper()
	ctx := context.Background()
	price, err := f.client.CreatePrice(ctx, req(&pb.CreatePriceRequest{RequestKey: key(), Reason: "rated price", Price: &pb.Price{PlanId: f.plan, ServiceId: f.service, Meter: meter, UnitPrice: unit, Included: included, Currency: "USD"}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	if _, err = f.db.Pool.Exec(ctx, "UPDATE entitlements SET ends_at=$3 WHERE tenant_id=$1 AND ends_at IS NULL AND price_id IN (SELECT id FROM prices WHERE meter=$2)", f.tenant, meter, at); err != nil {
		t.Fatal(err)
	}
	if _, err = f.db.Pool.Exec(ctx, "INSERT INTO entitlements(id,tenant_id,price_id,starts_at) VALUES($1,$2,$3,$4)", key(), f.tenant, price.Msg.Price.Id, at); err != nil {
		t.Fatal(err)
	}
	return price.Msg.Price.Id
}

func (f *ratingFixture) grant(t *testing.T, meter, amount string, at time.Time) {
	t.Helper()
	if _, err := f.db.Pool.Exec(context.Background(), "INSERT INTO allowance_grants(id,tenant_id,service_id,meter,amount,period_start,period_end,applied_at,source_key) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)", key(), f.tenant, f.service, meter, amount, f.start, f.end, at, "fixture:"+key()); err != nil {
		t.Fatal(err)
	}
}

func (f *ratingFixture) ingest(t *testing.T, eventKey, eventType string, at time.Time, attributes map[string]*pb.AttributeValue) string {
	t.Helper()
	response, err := f.client.IngestEvent(context.Background(), req(&pb.IngestEventRequest{RequestKey: key(), Event: &pb.Event{TenantId: f.tenant, ServiceId: f.service, EventKey: eventKey, EventType: eventType, OccurredAt: timestamppb.New(at), Attributes: attributes}}, "service"))
	if err != nil {
		t.Fatal(err)
	}
	return response.Msg.EventId
}

func (f *ratingFixture) correct(t *testing.T, eventKey string, expected int32, eventType string, attributes map[string]*pb.AttributeValue) {
	t.Helper()
	kind := pb.EventCorrectionKind_EVENT_CORRECTION_KIND_REPLACE
	if eventType == "" {
		kind = pb.EventCorrectionKind_EVENT_CORRECTION_KIND_CANCEL
	}
	if _, err := f.client.CorrectEvent(context.Background(), req(&pb.CorrectEventRequest{RequestKey: key(), TenantId: f.tenant, EventKey: eventKey, ExpectedRevision: expected, Kind: kind, EventType: eventType, Attributes: attributes}, "service")); err != nil {
		t.Fatal(err)
	}
}

func (f *ratingFixture) decimal(t *testing.T, query string, args ...any) string {
	t.Helper()
	var text string
	if err := f.db.Pool.QueryRow(context.Background(), query, args...).Scan(&text); err != nil {
		t.Fatal(err)
	}
	d, err := accounting.Parse(text)
	if err != nil {
		t.Fatal(text, err)
	}
	return d.String()
}

// charged is the amount charged for one meter period by usage, correction,
// and postpaid entries.
func (f *ratingFixture) charged(t *testing.T, meter string) string {
	t.Helper()
	return f.decimal(t, "SELECT (-COALESCE(sum(amount),0))::text FROM ledger WHERE tenant_id=$1 AND meter=$2 AND period_start=$3 AND kind IN ('usage','postpaid','correction')", f.tenant, meter, f.start)
}

func (f *ratingFixture) entries(t *testing.T, meter string) []string {
	t.Helper()
	rows, err := f.db.Pool.Query(context.Background(), "SELECT kind||':'||amount::text FROM ledger WHERE tenant_id=$1 AND meter=$2 ORDER BY created_at,id", f.tenant, meter)
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()
	var out []string
	for rows.Next() {
		var entry string
		if err = rows.Scan(&entry); err != nil {
			t.Fatal(err)
		}
		kind, amount, _ := strings.Cut(entry, ":")
		d, err := accounting.Parse(amount)
		if err != nil {
			t.Fatal(err)
		}
		out = append(out, kind+":"+d.String())
	}
	if err = rows.Err(); err != nil {
		t.Fatal(err)
	}
	return out
}

func expectEntries(t *testing.T, got []string, want ...string) {
	t.Helper()
	if strings.Join(got, ",") != strings.Join(want, ",") {
		t.Fatalf("ledger entries = %v, want %v", got, want)
	}
}

func tokens(model, n string) map[string]*pb.AttributeValue {
	return map[string]*pb.AttributeValue{"model": text(model), "tokens": num(n)}
}

func TestRatingClampsGroupsSharesIncludedUsageAndAppendsDifferences(t *testing.T) {
	now := time.Now().UTC().Truncate(time.Second)
	f := newRatingFixture(t, now.Add(-12*time.Hour), now.Add(12*time.Hour), false)
	f.price(t, "tokens", "0.5", "10", f.start)
	f.grant(t, "tokens", "10", f.start)
	at := now.Add(-time.Hour)
	// Groups -5 and 20 with 10 included usage bill 10 units: the negative
	// group never offsets the positive one, and the allowance is shared once.
	f.ingest(t, "e1", "completion", at, tokens("a", "-5"))
	f.ingest(t, "e2", "completion", at.Add(time.Second), tokens("b", "20"))
	processMetering(t, f.w)
	expectEntries(t, f.entries(t, "tokens"), "usage:-5")
	if got := f.decimal(t, "SELECT charge::text FROM metering_rated_periods WHERE tenant_id=$1", f.tenant); got != "5" {
		t.Fatalf("rated charge = %s", got)
	}
	// An increase appends usage; a replacement decrease, a cancellation, and a
	// replacement that moves a group append corrections.
	f.ingest(t, "e3", "completion", at.Add(2*time.Second), tokens("a", "8"))
	processMetering(t, f.w)
	f.correct(t, "e2", 1, "completion", tokens("b", "12"))
	processMetering(t, f.w)
	f.correct(t, "e3", 1, "", nil)
	processMetering(t, f.w)
	f.correct(t, "e1", 1, "completion", tokens("c", "4"))
	processMetering(t, f.w)
	expectEntries(t, f.entries(t, "tokens"), "usage:-5", "usage:-1.5", "correction:4", "correction:1.5", "correction:-2")
	if got := f.charged(t, "tokens"); got != "3" {
		t.Fatalf("charged = %s, want 3", got)
	}
	if got := f.decimal(t, "SELECT remaining_included::text FROM metering_rated_periods WHERE tenant_id=$1", f.tenant); got != "0" {
		t.Fatalf("remaining included = %s", got)
	}
	// Every entry is attributed to the original period, service and meter.
	if n := f.count(t, "SELECT count(*) FROM ledger WHERE tenant_id=$1 AND meter='tokens' AND service_id=$2 AND period_start=$3 AND period_end=$4 AND usage_id IS NULL AND source_key LIKE 'rating:%'", f.tenant, f.service, f.start, f.end); n != 5 {
		t.Fatalf("attributed entries = %d", n)
	}
}

func TestPaidAuthorizationUsesSharedIncludedUsageOfClampedGroups(t *testing.T) {
	now := time.Now().UTC().Truncate(time.Second)
	f := newRatingFixture(t, now.Add(-12*time.Hour), now.Add(12*time.Hour), false)
	f.price(t, "tokens", "1", "10", f.start)
	f.grant(t, "tokens", "10", f.start)
	allowed := func() bool {
		t.Helper()
		decision, err := f.client.Authorize(context.Background(), req(&pb.AuthorizeRequest{TenantId: f.tenant, UserId: f.user, Capability: "execute", Paid: true}, "service"))
		if err != nil {
			t.Fatal(err)
		}
		return decision.Msg.Allowed
	}
	if !allowed() {
		t.Fatal("unused included usage was denied")
	}
	at := now.Add(-time.Hour)
	f.ingest(t, "a", "completion", at, tokens("a", "6"))
	f.ingest(t, "b", "completion", at, tokens("b", "-3"))
	processMetering(t, f.w)
	if !allowed() {
		t.Fatal("remaining included usage was denied")
	}
	// Groups 6, -3 and 4 consume all ten included units because the negative
	// group cannot offset the others. The zero credit balance then blocks.
	f.ingest(t, "c", "completion", at, tokens("c", "4"))
	processMetering(t, f.w)
	if allowed() {
		t.Fatal("exhausted shared allowance was allowed")
	}
	if len(f.entries(t, "tokens")) != 0 {
		t.Fatal("included usage was charged")
	}
	// A correction that lowers a group restores the remaining allowance.
	f.correct(t, "c", 1, "completion", tokens("c", "3"))
	processMetering(t, f.w)
	if !allowed() {
		t.Fatal("restored allowance was denied")
	}
}

func (f *ratingFixture) count(t *testing.T, query string, args ...any) int {
	t.Helper()
	var n int
	if err := f.db.Pool.QueryRow(context.Background(), query, args...).Scan(&n); err != nil {
		t.Fatal(err)
	}
	return n
}

func TestRatingPinsPricesAcrossPlanChangesWithProratedAllowance(t *testing.T) {
	now := time.Now().UTC().Truncate(time.Second)
	f := newRatingFixture(t, now.Add(-12*time.Hour), now.Add(12*time.Hour), false)
	ctx := context.Background()
	old := f.price(t, "units", "1", "4", f.start)
	f.grant(t, "units", "4", f.start)
	f.ingest(t, "before", "units", now.Add(-4*time.Hour), map[string]*pb.AttributeValue{"quantity": num("10")})
	processMetering(t, f.w)
	expectEntries(t, f.entries(t, "units"), "usage:-6")

	// An upgrade three hours ago switches the price and grants the prorated
	// allowance difference: 3 * 15h / 24h = 1.875 units.
	changed := now.Add(-3 * time.Hour)
	previous, err := postgres.CurrentAllowances(ctx, mustTx(t, f.db), f.tenant)
	if err != nil {
		t.Fatal(err)
	}
	upgraded := f.price(t, "units", "3", "7", changed)
	if err = f.db.Transaction(ctx, func(tx pgx.Tx) error {
		return postgres.GrantAllowances(ctx, tx, f.tenant, f.start, f.end, changed, "upgrade:"+f.tenant, previous)
	}); err != nil {
		t.Fatal(err)
	}
	if got := f.decimal(t, "SELECT sum(amount)::text FROM allowance_grants WHERE tenant_id=$1 AND meter='units'", f.tenant); got != "5.875" {
		t.Fatalf("granted allowance = %s", got)
	}
	f.ingest(t, "after", "units", now.Add(-2*time.Hour), map[string]*pb.AttributeValue{"quantity": num("5")})
	processMetering(t, f.w)
	// The old segment cannot use the later grant: 6 units at 1 plus
	// (5 - 1.875) units at 3.
	if got := f.charged(t, "units"); got != "15.375" {
		t.Fatalf("mixed-price charge = %s", got)
	}
	// A correction keeps the original event's price.
	f.correct(t, "before", 1, "units", map[string]*pb.AttributeValue{"quantity": num("8")})
	processMetering(t, f.w)
	if got := f.charged(t, "units"); got != "13.375" {
		t.Fatalf("corrected charge = %s", got)
	}
	// Late usage that occurred before the change uses the old price.
	late := f.ingest(t, "late", "units", now.Add(-210*time.Minute), map[string]*pb.AttributeValue{"quantity": num("1")})
	processMetering(t, f.w)
	if got := f.charged(t, "units"); got != "14.375" {
		t.Fatalf("late charge = %s", got)
	}
	var pinned string
	if err = f.db.Pool.QueryRow(ctx, "SELECT price_id::text FROM metering_event_prices WHERE event_id=$1", late).Scan(&pinned); err != nil || pinned != old {
		t.Fatalf("late event price = %s, want %s (%v)", pinned, old, err)
	}
	if f.count(t, "SELECT count(*) FROM metering_event_prices ep JOIN metering_events e ON e.id=ep.event_id WHERE e.event_key='after' AND ep.price_id=$1", upgraded) != 1 {
		t.Fatal("upgraded usage was not attributed to the new price")
	}
	expectEntries(t, f.entries(t, "units"), "usage:-6", "usage:-9.375", "correction:2", "usage:-1")
}

func mustTx(t *testing.T, db *postgres.Store) pgx.Tx {
	t.Helper()
	tx, err := db.Pool.Begin(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { tx.Rollback(context.Background()) })
	return tx
}

func TestRatingNonAdditiveDecreasesAreCorrections(t *testing.T) {
	now := time.Now().UTC().Truncate(time.Second)
	f := newRatingFixture(t, now.Add(-12*time.Hour), now.Add(12*time.Hour), false)
	f.price(t, "seats", "2", "1", f.start)
	f.grant(t, "seats", "1", f.start)
	f.price(t, "latency", "0.1", "0", f.start)
	at := now.Add(-time.Hour)
	seats := func(n string) map[string]*pb.AttributeValue { return map[string]*pb.AttributeValue{"seats": num(n)} }
	f.ingest(t, "s1", "seat", at, seats("3"))
	f.ingest(t, "s2", "seat", at.Add(time.Second), seats("2"))
	processMetering(t, f.w)
	// The maximum is 3 and the second event changes nothing.
	expectEntries(t, f.entries(t, "seats"), "usage:-4")
	f.correct(t, "s1", 1, "", nil)
	processMetering(t, f.w)
	expectEntries(t, f.entries(t, "seats"), "usage:-4", "correction:2")

	// A new event that lowers an average is a decrease and is corrected.
	ms := func(n string) map[string]*pb.AttributeValue { return map[string]*pb.AttributeValue{"ms": num(n)} }
	f.ingest(t, "p1", "probe", at, ms("20"))
	f.ingest(t, "p2", "probe", at.Add(time.Second), ms("40"))
	processMetering(t, f.w)
	f.ingest(t, "p3", "probe", at.Add(2*time.Second), ms("0"))
	processMetering(t, f.w)
	expectEntries(t, f.entries(t, "latency"), "usage:-2", "usage:-1", "correction:1")

	// A matched meter without a usage price is metered but never rated.
	f.ingest(t, "ping", "ping", at, nil)
	processMetering(t, f.w)
	if f.count(t, "SELECT count(*) FROM metering_aggregates a JOIN catalog_entities ce ON ce.id=a.meter_id WHERE ce.key='unpriced'") != 1 || len(f.entries(t, "unpriced")) != 0 || f.count(t, "SELECT count(*) FROM metering_event_prices ep JOIN catalog_entities ce ON ce.id=ep.meter_id WHERE ce.key='unpriced'") != 0 {
		t.Fatal("unpriced usage was rated")
	}
}

func TestRatingPostpaidReversesUnpaidChargesBeforeRefundingCredit(t *testing.T) {
	now := time.Now().UTC().Truncate(time.Second)
	f := newRatingFixture(t, now.Add(-12*time.Hour), now.Add(12*time.Hour), true)
	f.price(t, "units", "1", "0", f.start)
	if _, err := f.db.Pool.Exec(context.Background(), "INSERT INTO ledger(id,tenant_id,kind,amount,source_key,created_at) VALUES($1,$2,'credit',2,$1::uuid::text,now())", key(), f.tenant); err != nil {
		t.Fatal(err)
	}
	f.ingest(t, "u", "units", now.Add(-time.Hour), map[string]*pb.AttributeValue{"quantity": num("5")})
	processMetering(t, f.w)
	// Credit covers two units; the rest continues as unpaid postpaid usage.
	expectEntries(t, f.entries(t, "units"), "postpaid:-3", "usage:-2")
	f.correct(t, "u", 1, "units", map[string]*pb.AttributeValue{"quantity": num("1")})
	processMetering(t, f.w)
	// The decrease first reverses unpaid postpaid usage and only then returns
	// prepaid credit, so a correction cannot mint spendable credit.
	expectEntries(t, f.entries(t, "units"), "postpaid:-3", "usage:-2", "postpaid:3", "correction:1")
	if got := f.decimal(t, "SELECT sum(amount)::text FROM ledger WHERE tenant_id=$1 AND kind IN ('credit','usage','refund','correction')", f.tenant); got != "1" {
		t.Fatalf("balance = %s", got)
	}
}

func TestRatingLateUsageInAClosedPeriodBecomesTheNextBill(t *testing.T) {
	now := time.Now().UTC().Truncate(time.Second)
	f := newRatingFixture(t, now.Add(-20*time.Hour), now.Add(-2*time.Hour), true)
	f.price(t, "units", "0.3333", "0", f.start)
	f.ingest(t, "first", "units", now.Add(-10*time.Hour), map[string]*pb.AttributeValue{"quantity": num("1")})
	processMetering(t, f.w)
	ctx := context.Background()
	if err := f.w.Maintain(ctx, now); err != nil {
		t.Fatal(err)
	}
	// The closed period total is rounded once, half-up, and settled.
	if got := f.charged(t, "units"); got != "0.33" {
		t.Fatalf("rounded charge = %s", got)
	}
	if n := f.count(t, "SELECT count(*) FROM settlements WHERE tenant_id=$1 AND period_start=$2 AND amount=0.33", f.tenant, f.start); n != 1 {
		t.Fatalf("first settlement lines = %d", n)
	}
	// Late usage keeps its original period, is rated exactly, is rounded on
	// the new period total, and becomes the next settlement.
	f.ingest(t, "late", "units", now.Add(-3*time.Hour), map[string]*pb.AttributeValue{"quantity": num("1")})
	processMetering(t, f.w)
	if got := f.charged(t, "units"); got != "0.6666" {
		t.Fatalf("late exact charge = %s", got)
	}
	if err := f.w.Maintain(ctx, now); err != nil {
		t.Fatal(err)
	}
	if got := f.charged(t, "units"); got != "0.67" {
		t.Fatalf("re-rounded charge = %s", got)
	}
	if n := f.count(t, "SELECT count(*) FROM settlements WHERE tenant_id=$1 AND period_start=$2 AND amount=0.34", f.tenant, f.start); n != 1 {
		t.Fatalf("late usage was not billed separately in its original period: %d", n)
	}
	if n := f.count(t, "SELECT count(*) FROM jobs WHERE tenant_id=$1 AND kind='settlement'", f.tenant); n != 2 {
		t.Fatalf("settlement jobs = %d", n)
	}
	// Repeated maintenance neither rounds nor settles again.
	if err := f.w.Maintain(ctx, now); err != nil {
		t.Fatal(err)
	}
	if got := f.charged(t, "units"); got != "0.67" || f.count(t, "SELECT count(*) FROM jobs WHERE tenant_id=$1 AND kind='settlement'", f.tenant) != 2 {
		t.Fatal("maintenance repeated accounting effects")
	}
}

func TestRatingRollbackRetryAndReplayAreIdempotent(t *testing.T) {
	now := time.Now().UTC().Truncate(time.Second)
	f := newRatingFixture(t, now.Add(-12*time.Hour), now.Add(12*time.Hour), false)
	f.price(t, "units", "2", "0", f.start)
	ctx := context.Background()
	at := now.Add(-time.Hour)
	request := quantityEvent(f.tenant, f.service, "units", "once", "3", at)
	accepted, err := f.client.IngestEvent(ctx, req(request, "service"))
	if err != nil {
		t.Fatal(err)
	}
	// A crash after rating but before commit rolls back aggregates and ledger.
	f.w.MeteringApplied = func(context.Context, pgx.Tx, events.Applied) error { return errors.New("simulated crash after rating") }
	processMetering(t, f.w)
	if len(f.entries(t, "units")) != 0 || f.count(t, "SELECT count(*) FROM metering_rated_periods WHERE tenant_id=$1", f.tenant) != 0 {
		t.Fatal("rolled back rating left ledger effects")
	}
	f.w.MeteringApplied = nil
	if _, err = f.db.Pool.Exec(ctx, "UPDATE jobs SET available_at=now()-interval '1 second' WHERE kind='metering' AND state='pending'"); err != nil {
		t.Fatal(err)
	}
	processMetering(t, f.w)
	expectEntries(t, f.entries(t, "units"), "usage:-6")
	if f.count(t, "SELECT count(*) FROM ledger WHERE source_key=$1", "rating:"+accepted.Msg.EventId+":1:"+meterID(t, f, "units")) != 1 {
		t.Fatal("rating source key does not name the event, revision, and meter")
	}
	// Replayed ingestion, redelivered work, and repeated processing add nothing.
	replay := quantityEvent(f.tenant, f.service, "units", "once", "3", at)
	if again, err := f.client.IngestEvent(ctx, req(replay, "service")); err != nil || again.Msg.EventId != accepted.Msg.EventId {
		t.Fatalf("replay = %v %v", again, err)
	}
	if _, err = f.db.Pool.Exec(ctx, `INSERT INTO jobs(id,tenant_id,kind,dedup_key,payload,available_at,created_at,updated_at) VALUES($1,$2,'metering',$3,jsonb_build_object('event_id',$4::text),now()-interval '1 second',now(),now())`, key(), f.tenant, "redelivery:"+accepted.Msg.EventId, accepted.Msg.EventId); err != nil {
		t.Fatal(err)
	}
	processMetering(t, f.w)
	processMetering(t, f.w)
	expectEntries(t, f.entries(t, "units"), "usage:-6")
	// Aggregates that disagree with their contributions are a permanent
	// failure, classified like other inconsistent metering state.
	if _, err = f.db.Pool.Exec(ctx, "UPDATE metering_aggregates SET value=value+1,sum=sum+1 WHERE tenant_id=$1", f.tenant); err != nil {
		t.Fatal(err)
	}
	f.correct(t, "once", 1, "units", map[string]*pb.AttributeValue{"quantity": num("4")})
	processMetering(t, f.w)
	if f.count(t, "SELECT count(*) FROM jobs WHERE kind='metering' AND state='failed' AND error_category='metering_invalid'") != 1 {
		t.Fatal("inconsistent rating input was not a permanent failure")
	}
	expectEntries(t, f.entries(t, "units"), "usage:-6")
}

func meterID(t *testing.T, f *ratingFixture, meter string) string {
	t.Helper()
	var id string
	if err := f.db.Pool.QueryRow(context.Background(), "SELECT id::text FROM catalog_entities WHERE kind='meter' AND scope=$1 AND key=$2", f.service, meter).Scan(&id); err != nil {
		t.Fatal(err)
	}
	return id
}

func TestUntypedMeterCompatibilityDefinitionIsImmutable(t *testing.T) {
	now := time.Now().UTC().Truncate(time.Second)
	f := newRatingFixture(t, now.Add(-12*time.Hour), now.Add(12*time.Hour), false)
	ctx := context.Background()
	// An untyped meter can be typed while no event used it.
	if _, err := f.client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "untyped", Meter: &pb.Meter{ServiceId: f.service, Name: "spare"}}, "owner")); err != nil {
		t.Fatal(err)
	}
	if _, err := f.client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "type unused meter", Meter: &pb.Meter{ServiceId: f.service, Name: "spare", EventType: "spare-event", Aggregation: pb.MeterAggregation_METER_AGGREGATION_COUNT}}, "owner")); err != nil {
		t.Fatal(err)
	}
	f.ingest(t, "compat", "units", now.Add(-time.Hour), map[string]*pb.AttributeValue{"quantity": num("2")})
	if _, err := f.client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "retype used meter", Meter: &pb.Meter{ServiceId: f.service, Name: "units", EventType: "units", Aggregation: pb.MeterAggregation_METER_AGGREGATION_COUNT}}, "owner")); connect.CodeOf(err) != connect.CodeInvalidArgument {
		t.Fatalf("used untyped meter changed its definition: %v", err)
	}
	if _, err := f.client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "declare compatibility definition", Meter: &pb.Meter{ServiceId: f.service, Name: "units", EventType: "units", Aggregation: pb.MeterAggregation_METER_AGGREGATION_SUM, ValueAttribute: "quantity"}}, "owner")); err != nil {
		t.Fatalf("compatibility definition rejected: %v", err)
	}
	if _, err := f.client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "retype declared meter", Meter: &pb.Meter{ServiceId: f.service, Name: "units", EventType: "units", Aggregation: pb.MeterAggregation_METER_AGGREGATION_COUNT}}, "owner")); connect.CodeOf(err) != connect.CodeInvalidArgument {
		t.Fatalf("declared meter changed its definition: %v", err)
	}
	// A non-numeric quantity is invalid for the compatibility definition.
	if _, err := f.client.IngestEvent(ctx, req(&pb.IngestEventRequest{RequestKey: key(), Event: &pb.Event{TenantId: f.tenant, ServiceId: f.service, EventKey: "bad", EventType: "units", OccurredAt: timestamppb.New(now.Add(-time.Hour)), Attributes: map[string]*pb.AttributeValue{"quantity": text("2")}}}, "service")); reasonOf(err) != pb.ErrorReason_ERROR_REASON_INVALID_ATTRIBUTE {
		t.Fatalf("non-numeric quantity accepted: %v", err)
	}
}
