// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package integration

import (
	"context"
	"sync"
	"testing"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/devbase/v1"
	"github.com/delinoio/apps/servers/devbase-api/internal/api"
	"github.com/delinoio/apps/servers/devbase-api/internal/config"
	"github.com/delinoio/apps/servers/devbase-api/internal/delcore"
)

func TestReplayReturnsThePriorResult(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	req := &pb.CreateProjectRequest{IdempotencyKey: idem(), TenantId: tenant, Name: "Once"}
	first, e := f.client.CreateProject(ctx, as(login(owner), req))
	if e != nil {
		t.Fatal(e)
	}
	again, e := f.client.CreateProject(ctx, as(login(owner), req))
	if e != nil || again.Msg.Project.Id != first.Msg.Project.Id || again.Msg.Project.Version != 1 {
		t.Fatal("replay did not return the original result", e)
	}
	if f.count("SELECT count(*) FROM projects") != 1 || f.count("SELECT count(*) FROM audit_events WHERE action='project.create'") != 1 {
		t.Fatal("replay applied or audited the change twice")
	}
	// Different input with the same key is an error, as is a different operation.
	changed := &pb.CreateProjectRequest{IdempotencyKey: req.IdempotencyKey, TenantId: tenant, Name: "Different"}
	if _, e = f.client.CreateProject(ctx, as(login(owner), changed)); code(e) != connect.CodeAlreadyExists {
		t.Fatal("differing input accepted", e)
	}
	if _, e = f.client.UpdateProject(ctx, as(login(owner), &pb.UpdateProjectRequest{IdempotencyKey: req.IdempotencyKey, ProjectId: first.Msg.Project.Id, Name: "Once", Version: 1})); code(e) != connect.CodeAlreadyExists {
		t.Fatal("different operation accepted", e)
	}
	// A separate request with a duplicate name is also an error.
	if _, e = f.client.CreateProject(ctx, as(login(owner), &pb.CreateProjectRequest{IdempotencyKey: idem(), TenantId: tenant, Name: "once"})); code(e) != connect.CodeAlreadyExists {
		t.Fatal("duplicate name accepted", e)
	}
}

func TestReplayExpiresAfter24Hours(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	req := &pb.CreateProjectRequest{IdempotencyKey: idem(), TenantId: tenant, Name: "Expiring"}
	if _, e := f.client.CreateProject(ctx, as(login(owner), req)); e != nil {
		t.Fatal(e)
	}
	f.clock.advance(api.ReplayRetention - time.Minute)
	if _, e := f.client.CreateProject(ctx, as(login(owner), req)); e != nil {
		t.Fatal("replay lost before 24 hours", e)
	}
	f.clock.advance(2 * time.Minute)
	// After expiry the key is a new request, which now conflicts on the existing name.
	if _, e := f.client.CreateProject(ctx, as(login(owner), req)); code(e) != connect.CodeAlreadyExists {
		t.Fatal("expired key still replayed", e)
	}
	if f.count("SELECT count(*) FROM projects") != 1 {
		t.Fatal("expired replay created a duplicate")
	}
}

func TestReplayIsScopedToTheCaller(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, admin := f.tenant()
	key := idem()
	first, e := f.client.CreateProject(ctx, as(login(owner), &pb.CreateProjectRequest{IdempotencyKey: key, TenantId: tenant, Name: "Scoped"}))
	if e != nil {
		t.Fatal(e)
	}
	// Another authorized caller with the same key executes independently and cannot see the first result.
	second, e := f.client.CreateProject(ctx, as(login(admin), &pb.CreateProjectRequest{IdempotencyKey: key, TenantId: tenant, Name: "Scoped Two"}))
	if e != nil || second.Msg.Project.Id == first.Msg.Project.Id {
		t.Fatal("replay leaked across callers", e)
	}
	// An unauthorized caller cannot obtain the replay result.
	f.fake.grant("member", tenant, "member")
	if _, e = f.client.CreateProject(ctx, as(login("member"), &pb.CreateProjectRequest{IdempotencyKey: key, TenantId: tenant, Name: "Scoped"})); code(e) != connect.CodePermissionDenied {
		t.Fatal("unauthorized replay", e)
	}
	// Another tenant with the same caller and key is a conflict, not a replay.
	other, _, _ := f.tenant()
	f.fake.grant(owner, other, "owner")
	if _, e = f.client.CreateProject(ctx, as(login(owner), &pb.CreateProjectRequest{IdempotencyKey: key, TenantId: other, Name: "Scoped"})); code(e) != connect.CodeAlreadyExists {
		t.Fatal("cross-tenant replay", e)
	}
}

func TestReplayRechecksCurrentAuthority(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, admin := f.tenant()
	req := &pb.CreateProjectRequest{IdempotencyKey: idem(), TenantId: tenant, Name: "Rechecked"}
	if _, e := f.client.CreateProject(ctx, as(login(admin), req)); e != nil {
		t.Fatal(e)
	}
	f.fake.grant(admin, tenant, "member") // demoted
	if _, e := f.client.CreateProject(ctx, as(login(admin), req)); code(e) != connect.CodePermissionDenied {
		t.Fatal("replay bypassed current authorization", e)
	}
	f.fake.grant(admin, tenant, "admin")
	f.fake.setState(tenant, delcore.StateBlocked)
	if _, e := f.client.CreateProject(ctx, as(login(admin), req)); code(e) != connect.CodePermissionDenied {
		t.Fatal("replay bypassed tenant state", e)
	}
	f.fake.setState(tenant, delcore.StateActive)
	if _, e := f.client.CreateProject(ctx, as(login(admin), req)); e != nil {
		t.Fatal("replay failed after authority returned", e)
	}
	_ = owner
}

func TestConcurrentSameKeyExecutesOnce(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	req := &pb.CreateProjectRequest{IdempotencyKey: idem(), TenantId: tenant, Name: "Racing"}
	var wg sync.WaitGroup
	ids := make(chan string, 12)
	for i := 0; i < 12; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			r, e := f.client.CreateProject(ctx, as(login(owner), req))
			if e != nil {
				t.Error(e)
				return
			}
			ids <- r.Msg.Project.Id
		}()
	}
	wg.Wait()
	close(ids)
	seen := map[string]bool{}
	for id := range ids {
		seen[id] = true
	}
	if len(seen) != 1 || f.count("SELECT count(*) FROM projects") != 1 {
		t.Fatalf("same-key retries executed %d times", len(seen))
	}
}

func TestDeletedProjectReplays(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	create := &pb.CreateProjectRequest{IdempotencyKey: idem(), TenantId: tenant, Name: "Gone"}
	made, e := f.client.CreateProject(ctx, as(login(owner), create))
	if e != nil {
		t.Fatal(e)
	}
	del := &pb.DeleteProjectRequest{IdempotencyKey: idem(), ProjectId: made.Msg.Project.Id, Version: 1}
	if _, e = f.client.DeleteProject(ctx, as(login(owner), del)); e != nil {
		t.Fatal(e)
	}
	// A repeated delete is idempotent for the authorized caller.
	if _, e = f.client.DeleteProject(ctx, as(login(owner), del)); e != nil {
		t.Fatal("repeated delete failed", e)
	}
	// Replaying the creation never resurrects the project.
	if _, e = f.client.CreateProject(ctx, as(login(owner), create)); code(e) != connect.CodeNotFound {
		t.Fatal("deleted project creation replayed", e)
	}
	if f.count("SELECT count(*) FROM projects") != 0 {
		t.Fatal("project resurrected")
	}
	// The scrubbed record keeps no payload.
	if n := f.count("SELECT count(*) FROM replays WHERE operation='CreateProject' AND (response IS NOT NULL OR length(fingerprint)>0)"); n != 0 {
		t.Fatal("scrubbed replay retains data", n)
	}
	// Repeated delete is re-authorized with current authority.
	f.fake.grant("member", tenant, "member")
	if _, e = f.client.DeleteProject(ctx, as(login("member"), del)); code(e) != connect.CodeNotFound && code(e) != connect.CodePermissionDenied {
		t.Fatal("another caller replayed a delete", e)
	}
	f.fake.grant(owner, tenant, "member")
	if _, e = f.client.DeleteProject(ctx, as(login(owner), del)); code(e) != connect.CodePermissionDenied {
		t.Fatal("repeated delete skipped current authorization", e)
	}
}

func TestDeleteReplayKeyReusedForAnotherMissingProjectConflicts(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	made, e := f.client.CreateProject(ctx, as(login(owner), &pb.CreateProjectRequest{IdempotencyKey: idem(), TenantId: tenant, Name: "Gone"}))
	if e != nil {
		t.Fatal(e)
	}
	key := idem()
	if _, e = f.client.DeleteProject(ctx, as(login(owner), &pb.DeleteProjectRequest{IdempotencyKey: key, ProjectId: made.Msg.Project.Id, Version: 1})); e != nil {
		t.Fatal(e)
	}
	// The key already belongs to the first delete, so a delete of another missing
	// project is a key conflict rather than a missing project.
	other := &pb.DeleteProjectRequest{IdempotencyKey: key, ProjectId: newID(), Version: 1}
	if _, e = f.client.DeleteProject(ctx, as(login(owner), other)); code(e) != connect.CodeAlreadyExists {
		t.Fatal("a key reused for another missing project did not conflict", e)
	}
	// A key the caller never used still reports the missing project.
	other.IdempotencyKey = idem()
	if _, e = f.client.DeleteProject(ctx, as(login(owner), other)); code(e) != connect.CodeNotFound {
		t.Fatal("an unused key did not report the missing project", e)
	}
}

func TestScrubbedReplayKeyReusedForAnotherProjectConflicts(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	first := f.project(login(owner), tenant, "First")
	second := f.project(login(owner), tenant, "Second")
	key := idem()
	issue := func(project string) error {
		_, e := f.client.IssueApiKey(ctx, as(login(owner), &pb.IssueApiKeyRequest{IdempotencyKey: key, ProjectId: project, Permission: pb.ApiKeyPermission_API_KEY_PERMISSION_READ_ONLY}))
		return e
	}
	if e := issue(first.Id); e != nil {
		t.Fatal(e)
	}
	if _, e := f.client.DeleteProject(ctx, as(login(owner), &pb.DeleteProjectRequest{IdempotencyKey: idem(), ProjectId: first.Id, Version: 1})); e != nil {
		t.Fatal(e)
	}
	if n := f.count("SELECT count(*) FROM replays WHERE idempotency_key=$1 AND scrubbed", key); n != 1 {
		t.Fatal("the issuance record was not scrubbed", n)
	}
	// The scrubbed record has no fingerprint, but its project must still conflict.
	if e := issue(second.Id); code(e) != connect.CodeAlreadyExists {
		t.Fatal("a scrubbed key reused for another project did not conflict", e)
	}
	// The same project still reports the scrubbed outcome.
	if e := issue(first.Id); code(e) != connect.CodeNotFound {
		t.Fatal("a scrubbed replay did not report not_found", e)
	}
}

func TestManagementRateLimitIsPerTenantWithRetryTiming(t *testing.T) {
	f := setup(t, func(l *config.Limits) { l.ManagementRate = 2; l.ManagementBurst = 4 })
	ctx := context.Background()
	tenant, owner, admin := f.tenant()
	other, otherOwner, _ := f.tenant()
	p := f.project(login(owner), tenant, "Limited") // consumes 1 token
	// Different callers in the same tenant share the bucket.
	users := []string{owner, admin, owner}
	for _, user := range users {
		if _, e := f.client.GetProject(ctx, as(login(user), &pb.GetProjectRequest{ProjectId: p.Id})); e != nil {
			t.Fatal(e)
		}
	}
	_, e := f.client.GetProject(ctx, as(login(admin), &pb.GetProjectRequest{ProjectId: p.Id}))
	if code(e) != connect.CodeResourceExhausted {
		t.Fatal("tenant bucket not shared across callers", e)
	}
	ce := e.(*connect.Error)
	if ce.Meta().Get("Retry-After") == "" || ce.Meta().Get("X-Devbase-Trace-Id") == "" {
		t.Fatal("rate limit lacks retry timing or trace")
	}
	var detail *pb.ErrorDetail
	for _, d := range ce.Details() {
		if v, err := d.Value(); err == nil {
			detail, _ = v.(*pb.ErrorDetail)
		}
	}
	if detail == nil || detail.RetryAfter == nil || detail.RetryAfter.AsDuration() <= 0 || detail.Reason != pb.ErrorReason_ERROR_REASON_RATE_LIMITED {
		t.Fatal("typed retry timing missing", detail)
	}
	// Another tenant is unaffected.
	f.project(login(otherOwner), other, "Independent")
	f.clock.advance(time.Second)
	if _, e = f.client.GetProject(ctx, as(login(owner), &pb.GetProjectRequest{ProjectId: p.Id})); e != nil {
		t.Fatal("bucket did not refill", e)
	}
}
