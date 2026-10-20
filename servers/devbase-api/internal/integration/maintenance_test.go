// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package integration

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	pb "github.com/delinoio/apps/protos/devbase/v1"
	"github.com/delinoio/apps/servers/devbase-api/internal/delcore"
	"github.com/delinoio/apps/servers/devbase-api/internal/maintenance"
)

// seed creates a tenant with a project, a key and users, and returns its identifiers.
func (f *fixture) seed(name string) (tenant, project string) {
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, name)
	f.key(login(owner), p.Id, crud)
	f.user(login(owner), p.Id, "user-"+name)
	return tenant, p.Id
}

func TestCleanupDeletesOnlyFinalizedTenants(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	deleted, deletedProject := f.seed("deleted")
	states := map[string]delcore.State{}
	for _, name := range []string{"active", "blocked", "deleting", "unknown"} {
		tenant, _ := f.seed(name)
		states[name] = map[string]delcore.State{"active": delcore.StateActive, "blocked": delcore.StateBlocked, "deleting": delcore.StateDeleting, "unknown": delcore.StateUnknown}[name]
		f.fake.setState(tenant, states[name])
	}
	f.fake.setState(deleted, delcore.StateDeleted)
	before := f.count("SELECT count(*) FROM projects")
	f.worker().Cycle(ctx)
	if f.count("SELECT count(*) FROM projects") != before-1 || f.count("SELECT count(*) FROM projects WHERE tenant_id=$1", deleted) != 0 {
		t.Fatal("cleanup did not remove exactly the finalized tenant")
	}
	for _, table := range []string{"api_keys", "end_users"} {
		if n := f.count("SELECT count(*) FROM "+table+" WHERE project_id=$1", deletedProject); n != 0 {
			t.Fatalf("%s retains rows of the deleted tenant", table)
		}
	}
	if f.count("SELECT count(*) FROM end_users") != 4 || f.count("SELECT count(*) FROM api_keys") != 4 {
		t.Fatal("cleanup removed other tenants' data")
	}
	// Replay payloads of the deleted tenant are scrubbed; others are intact.
	if n := f.count("SELECT count(*) FROM replays WHERE tenant_id=$1 AND operation NOT LIKE 'Delete%' AND (response IS NOT NULL OR NOT scrubbed)", deleted); n != 0 {
		t.Fatal("deleted tenant's replay payloads remain", n)
	}
	if n := f.count("SELECT count(*) FROM replays WHERE tenant_id<>$1 AND scrubbed", deleted); n != 0 {
		t.Fatal("other tenants' replays were scrubbed", n)
	}
	var actor, action string
	if e := f.db.Pool.QueryRow(ctx, "SELECT actor,action FROM audit_events WHERE target_id=$1 AND action='tenant.cleanup'", deletedProject).Scan(&actor, &action); e != nil || actor != "system:cleanup" {
		t.Fatal("cleanup is not audited", e)
	}
	// A second cycle is a no-op.
	f.worker().Cycle(ctx)
	if f.count("SELECT count(*) FROM projects") != before-1 {
		t.Fatal("repeated cycle changed data")
	}
}

func TestCleanupNeverActsOnStateFailures(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, _ := f.seed("pending")
	f.fake.setState(tenant, delcore.StateDeleted)
	before := f.counter(t, "devbase_cleanup_failures_total")
	f.fake.mu.Lock()
	f.fake.stateErr = errors.New("delcore down")
	f.fake.mu.Unlock()
	f.worker().Cycle(ctx)
	if f.count("SELECT count(*) FROM projects") != 1 {
		t.Fatal("a failed state check deleted data")
	}
	if f.counter(t, "devbase_cleanup_failures_total") != before+1 {
		t.Fatal("cleanup failure was not counted")
	}
	if !strings.Contains(f.logs.String(), "tenant cleanup failed") {
		t.Fatal("cleanup failure was not logged")
	}
	// The next cycle retries and succeeds.
	f.fake.mu.Lock()
	f.fake.stateErr = nil
	f.fake.mu.Unlock()
	f.worker().Cycle(ctx)
	if f.count("SELECT count(*) FROM projects") != 0 {
		t.Fatal("retry did not clean up")
	}
}

func TestCleanupBatchesManyTenants(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	total := maintenance.TenantBatch*2 + 5
	for i := 0; i < total; i++ {
		tenant, owner, _ := f.tenant()
		if _, e := f.db.Pool.Exec(ctx, "INSERT INTO projects(id,tenant_id,name,created_at,updated_at) VALUES($1,$2,'p',now(),now())", newID(), tenant); e != nil {
			t.Fatal(e)
		}
		_ = owner
		if i%2 == 0 {
			f.fake.setState(tenant, delcore.StateDeleted)
		}
	}
	f.worker().Cycle(ctx)
	if got := f.count("SELECT count(*) FROM projects"); got != total/2 {
		t.Fatalf("expected %d surviving projects, got %d", total/2, got)
	}
}

func TestReplayAndAuditRetention(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	f.project(login(owner), tenant, "Retention")
	if f.count("SELECT count(*) FROM replays") != 1 || f.count("SELECT count(*) FROM audit_events") != 1 {
		t.Fatal("fixture records missing")
	}
	f.clock.advance(23 * time.Hour)
	f.worker().Cycle(ctx)
	if f.count("SELECT count(*) FROM replays") != 1 {
		t.Fatal("replay purged before 24 hours")
	}
	f.clock.advance(2 * time.Hour)
	f.worker().Cycle(ctx)
	if f.count("SELECT count(*) FROM replays") != 0 {
		t.Fatal("expired replay retained")
	}
	if f.count("SELECT count(*) FROM audit_events") != 1 {
		t.Fatal("audit purged early")
	}
	f.clock.advance(maintenance.AuditRetention - 26*time.Hour)
	f.worker().Cycle(ctx)
	if f.count("SELECT count(*) FROM audit_events") != 1 {
		t.Fatal("audit purged before 90 days")
	}
	f.clock.advance(2 * time.Hour)
	f.worker().Cycle(ctx)
	if f.count("SELECT count(*) FROM audit_events WHERE action='project.create'") != 0 {
		t.Fatal("expired audit retained")
	}
}

func TestMinimalAuditSurvivesUserDeletion(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, "Audit")
	u := f.user(login(owner), p.Id, "audited")
	if _, e := f.client.DeleteEndUser(ctx, as(login(owner), &pb.DeleteEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, EndUserId: u.Id, Version: 1})); e != nil {
		t.Fatal(e)
	}
	if f.count("SELECT count(*) FROM audit_events WHERE target_id=$1", u.Id) != 2 {
		t.Fatal("create and delete audit records must remain until retention expires")
	}
}

// counter reads an unlabelled counter from the registry.
func (f *fixture) counter(t *testing.T, name string) float64 {
	t.Helper()
	families, e := f.gather.Gather()
	if e != nil {
		t.Fatal(e)
	}
	for _, family := range families {
		if family.GetName() == name && len(family.GetMetric()) == 1 {
			return family.GetMetric()[0].GetCounter().GetValue()
		}
	}
	return 0
}
