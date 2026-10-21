// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package integration

import (
	"context"
	"io"
	"log/slog"
	"testing"
	"time"

	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/delcore-api/internal/worker"
)

// Retention keeps everything that pending work, allowed corrections, and
// accounting still need. Raw attribute maps of a finally deleted tenant are
// cleared at the raw-usage cutoff; identities, contributions, and replay
// protection remain until the later accounting cutoff.
func TestMeteringRetentionBoundariesAndRawAttributeRemoval(t *testing.T) {
	f := newMeterFixture(t, nil)
	ctx := context.Background()
	personal := f.tenant
	occurred := time.Now().UTC().Add(-time.Minute)
	r := request{region: "eu", outcome: "success", user: "personal-user", bytes: "1", latency: "1"}
	if _, err := f.ingest("service", "active", "request", occurred, r.wire()); err != nil {
		t.Fatal(err)
	}
	f.process(t)
	cfg := f.cfg
	cfg.DeletionGraceSeconds = 1
	cfg.RawUsageRetentionSeconds = 3600
	cfg.AccountingRetentionSeconds = 7200
	cfg.AuditRetentionSeconds = 7200
	w := &worker.Worker{DB: f.db, Config: cfg, Log: slog.New(slog.NewTextHandler(io.Discard, nil))}
	future := time.Now().UTC().Add(48 * time.Hour)
	// Active tenants keep raw content and remain correctable within lateness.
	if err := w.Maintain(ctx, future.Add(10*time.Hour)); err != nil {
		t.Fatal(err)
	}
	if f.count(t, "SELECT count(*) FROM metering_event_revisions WHERE tenant_id=$1 AND attributes<>'{}'::jsonb", personal) != 1 {
		t.Fatal("retention changed an active tenant's events")
	}
	if _, err := f.correct("active", 1, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_CANCEL, "", nil); err != nil {
		t.Fatal("retention blocked an allowed correction", err)
	}

	org, err := f.client.CreateTenant(ctx, req(&pb.CreateTenantRequest{RequestKey: key(), Name: "Retained organization"}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	f.tenant = org.Msg.Tenant.Id
	deletedUser := request{region: "eu", outcome: "success", user: "deleted-user", bytes: "1", latency: "1"}
	accepted, err := f.ingest("service", "deleted", "request", occurred, deletedUser.wire())
	if err != nil {
		t.Fatal(err)
	}
	if _, err = f.client.DeleteTenant(ctx, req(&pb.DeleteTenantRequest{RequestKey: key(), TenantId: f.tenant}, "owner")); err != nil {
		t.Fatal(err)
	}
	f.process(t)
	if err = w.Maintain(ctx, future); err != nil {
		t.Fatal(err)
	}
	var deletedAt time.Time
	if err = f.db.Pool.QueryRow(ctx, "SELECT deleted_at FROM tenants WHERE id=$1 AND deleted_at IS NOT NULL", f.tenant).Scan(&deletedAt); err != nil {
		t.Fatal("deletion did not finish", err)
	}
	raw := func() int {
		return f.count(t, "SELECT count(*) FROM metering_event_revisions WHERE tenant_id=$1 AND attributes<>'{}'::jsonb", f.tenant)
	}
	// Unfinished metering work holds raw content even after the cutoff.
	held := key()
	if _, err = f.db.Pool.Exec(ctx, `INSERT INTO jobs(id,tenant_id,kind,dedup_key,payload,available_at,created_at,updated_at) VALUES($1::uuid,$2,'metering',$1::text,jsonb_build_object('event_id',$3::text),now(),now(),now())`, held, f.tenant, accepted.EventId); err != nil {
		t.Fatal(err)
	}
	if err = w.Maintain(ctx, deletedAt.Add(90*time.Minute)); err != nil {
		t.Fatal(err)
	}
	if raw() != 1 {
		t.Fatal("raw attributes removed while metering work was unfinished")
	}
	if _, err = f.db.Pool.Exec(ctx, "UPDATE jobs SET state='done' WHERE id=$1", held); err != nil {
		t.Fatal(err)
	}
	if err = w.Maintain(ctx, deletedAt.Add(90*time.Minute)); err != nil {
		t.Fatal(err)
	}
	if raw() != 0 {
		t.Fatal("raw attributes outlived raw-usage retention after final deletion")
	}
	if f.count(t, "SELECT count(*) FROM metering_contributions WHERE tenant_id=$1", f.tenant) == 0 || f.count(t, "SELECT count(*) FROM metering_events WHERE id=$1", accepted.EventId) != 1 {
		t.Fatal("accounting evidence removed at the raw-usage cutoff")
	}
	if again, err := f.ingest("service", "deleted", "request", occurred, deletedUser.wire()); err != nil || again.EventId != accepted.EventId {
		t.Fatalf("replay protection lost after attribute removal: %v %v", again, err)
	}
	// Only controlled retention may clear attributes, and nothing else changes.
	if _, err = f.db.Pool.Exec(ctx, "UPDATE metering_event_revisions SET attributes='{}'::jsonb WHERE tenant_id=$1", personal); err == nil {
		t.Fatal("attribute removal outside retention accepted")
	}
	tx, err := f.db.Pool.Begin(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if _, err = tx.Exec(ctx, "SELECT set_config('delcore.retention','on',true)"); err != nil {
		t.Fatal(err)
	}
	if _, err = tx.Exec(ctx, "UPDATE metering_event_revisions SET event_type='changed' WHERE tenant_id=$1", f.tenant); err == nil {
		t.Fatal("retention changed revision content other than attributes")
	}
	tx.Rollback(ctx)
	if err = w.Maintain(ctx, deletedAt.Add(150*time.Minute)); err != nil {
		t.Fatal(err)
	}
	if f.count(t, "SELECT count(*) FROM metering_events WHERE tenant_id=$1", f.tenant) != 0 || f.count(t, "SELECT count(*) FROM metering_replay WHERE event_id=$1", accepted.EventId) != 1 {
		t.Fatal("accounting cutoff did not retain only replay identities")
	}
}
