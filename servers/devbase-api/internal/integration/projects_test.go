// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package integration

import (
	"context"
	"strings"
	"sync"
	"testing"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/devbase/v1"
	"github.com/delinoio/apps/servers/devbase-api/internal/config"
	"github.com/delinoio/apps/servers/devbase-api/internal/delcore"
)

func TestProjectLifecycleForOwnerAndAdmin(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, admin := f.tenant()
	for _, user := range []string{owner, admin} {
		p := f.project(login(user), tenant, "Project "+user[:8])
		got, e := f.client.GetProject(ctx, as(login(user), &pb.GetProjectRequest{ProjectId: p.Id}))
		if e != nil || got.Msg.Project.Name != p.Name || got.Msg.Project.Version != 1 {
			t.Fatal("get project", e)
		}
		upd, e := f.client.UpdateProject(ctx, as(login(user), &pb.UpdateProjectRequest{IdempotencyKey: idem(), ProjectId: p.Id, Name: p.Name + " v2", Version: 1}))
		if e != nil || upd.Msg.Project.Version != 2 {
			t.Fatal("update project", e)
		}
		if _, e = f.client.UpdateProject(ctx, as(login(user), &pb.UpdateProjectRequest{IdempotencyKey: idem(), ProjectId: p.Id, Name: "late", Version: 1})); code(e) != connect.CodeAborted {
			t.Fatal("stale update accepted", e)
		}
		if _, e = f.client.DeleteProject(ctx, as(login(user), &pb.DeleteProjectRequest{IdempotencyKey: idem(), ProjectId: p.Id, Version: 1})); code(e) != connect.CodeAborted {
			t.Fatal("stale delete accepted", e)
		}
		if _, e = f.client.DeleteProject(ctx, as(login(user), &pb.DeleteProjectRequest{IdempotencyKey: idem(), ProjectId: p.Id, Version: 2})); e != nil {
			t.Fatal("delete project", e)
		}
		if _, e = f.client.GetProject(ctx, as(login(user), &pb.GetProjectRequest{ProjectId: p.Id})); code(e) != connect.CodeNotFound {
			t.Fatal("deleted project readable", e)
		}
	}
	if n := f.count("SELECT count(*) FROM audit_events WHERE action LIKE 'project.%'"); n != 6 {
		t.Fatalf("expected 6 project audit events, got %d", n)
	}
}

func TestProjectNamesAndValidation(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	other, otherOwner, _ := f.tenant()
	f.project(login(owner), tenant, "Alpha")
	f.project(login(otherOwner), other, "alpha")
	_, e := f.client.CreateProject(ctx, as(login(owner), &pb.CreateProjectRequest{IdempotencyKey: idem(), TenantId: tenant, Name: "ALPHA"}))
	if code(e) != connect.CodeAlreadyExists {
		t.Fatal("case-insensitive duplicate accepted", e)
	}
	for name, bad := range map[string]string{"empty": "", "leading": " x", "trailing": "x\n", "long": strings.Repeat("a", 257)} {
		_, e := f.client.CreateProject(ctx, as(login(owner), &pb.CreateProjectRequest{IdempotencyKey: idem(), TenantId: tenant, Name: bad}))
		if code(e) != connect.CodeInvalidArgument {
			t.Fatalf("%s name accepted: %v", name, e)
		}
	}
	f.project(login(owner), tenant, strings.Repeat("é", 128))
	if _, e = f.client.CreateProject(ctx, as(login(owner), &pb.CreateProjectRequest{IdempotencyKey: "short", TenantId: tenant, Name: "ok"})); code(e) != connect.CodeInvalidArgument {
		t.Fatal("short idempotency key accepted", e)
	}
	if _, e = f.client.CreateProject(ctx, as(login(owner), &pb.CreateProjectRequest{IdempotencyKey: idem(), TenantId: "tenant", Name: "ok"})); code(e) != connect.CodeInvalidArgument {
		t.Fatal("malformed tenant accepted", e)
	}
}

func TestAuthorizationMatrixAndIsolation(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	other, otherOwner, _ := f.tenant()
	f.fake.grant("member", tenant, "member")
	f.fake.grant("billing", tenant, "billing")
	p := f.project(login(owner), tenant, "Isolated")
	for _, user := range []string{"member", "billing", otherOwner, "stranger"} {
		if _, e := f.client.GetProject(ctx, as(login(user), &pb.GetProjectRequest{ProjectId: p.Id})); code(e) != connect.CodePermissionDenied {
			t.Fatalf("%s read a project: %v", user, e)
		}
		if _, e := f.client.CreateProject(ctx, as(login(user), &pb.CreateProjectRequest{IdempotencyKey: idem(), TenantId: tenant, Name: "x" + user})); code(e) != connect.CodePermissionDenied {
			t.Fatalf("%s created a project: %v", user, e)
		}
		if _, e := f.client.IssueApiKey(ctx, as(login(user), &pb.IssueApiKeyRequest{IdempotencyKey: idem(), ProjectId: p.Id, Permission: pb.ApiKeyPermission_API_KEY_PERMISSION_CRUD})); code(e) != connect.CodePermissionDenied {
			t.Fatalf("%s issued a key: %v", user, e)
		}
	}
	if _, e := f.client.ListProjects(ctx, as(login(otherOwner), &pb.ListProjectsRequest{TenantId: tenant})); code(e) != connect.CodePermissionDenied {
		t.Fatal("cross-tenant list", e)
	}
	// Invalid authentication.
	for _, token := range []string{"", "garbage", "dbk_bad"} {
		r := as(token, &pb.GetProjectRequest{ProjectId: p.Id})
		if token == "" {
			r.Header().Del("Authorization")
		}
		if _, e := f.client.GetProject(ctx, r); code(e) != connect.CodeUnauthenticated {
			t.Fatalf("token %q: %v", token, e)
		}
	}
	// Project API keys never manage projects or keys, and never cross projects.
	other2 := f.project(login(otherOwner), other, "Other")
	secret, _ := f.key(login(owner), p.Id, pb.ApiKeyPermission_API_KEY_PERMISSION_CRUD)
	if _, e := f.client.GetProject(ctx, as(secret, &pb.GetProjectRequest{ProjectId: p.Id})); code(e) != connect.CodePermissionDenied {
		t.Fatal("key managed a project", e)
	}
	if _, e := f.client.IssueApiKey(ctx, as(secret, &pb.IssueApiKeyRequest{IdempotencyKey: idem(), ProjectId: p.Id, Permission: pb.ApiKeyPermission_API_KEY_PERMISSION_CRUD})); code(e) != connect.CodePermissionDenied {
		t.Fatal("key issued a key", e)
	}
	if _, e := f.client.ListApiKeys(ctx, as(secret, &pb.ListApiKeysRequest{ProjectId: p.Id})); code(e) != connect.CodePermissionDenied {
		t.Fatal("key listed keys", e)
	}
	_ = other2
}

func TestNonexistentProjectIsNotFound(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	_, owner, _ := f.tenant()
	_, e := f.client.GetProject(ctx, as(login(owner), &pb.GetProjectRequest{ProjectId: newID()}))
	if code(e) != connect.CodeNotFound {
		t.Fatal(e)
	}
	_, e = f.client.GetProject(ctx, as("garbage", &pb.GetProjectRequest{ProjectId: newID()}))
	if code(e) != connect.CodeUnauthenticated {
		t.Fatal("invalid token on a missing project:", e)
	}
	// The unverified token never reaches the fake as a role grant.
}

func TestTenantStatePolicy(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, "States")
	_, apiKey := f.key(login(owner), p.Id, pb.ApiKeyPermission_API_KEY_PERMISSION_CRUD)

	for _, state := range []delcore.State{delcore.StateBlocked, delcore.StateDeleting} {
		f.fake.setState(tenant, state)
		// Management reads, key revocation and deletion remain available to the Owner.
		if _, e := f.client.GetProject(ctx, as(login(owner), &pb.GetProjectRequest{ProjectId: p.Id})); e != nil {
			t.Fatal(state, "management read", e)
		}
		if _, e := f.client.ListProjects(ctx, as(login(owner), &pb.ListProjectsRequest{TenantId: tenant})); e != nil {
			t.Fatal(state, "list", e)
		}
		if _, e := f.client.GetApiKey(ctx, as(login(owner), &pb.GetApiKeyRequest{ProjectId: p.Id, ApiKeyId: apiKey.Id})); e != nil {
			t.Fatal(state, "key read", e)
		}
		if _, e := f.client.ListApiKeys(ctx, as(login(owner), &pb.ListApiKeysRequest{ProjectId: p.Id})); e != nil {
			t.Fatal(state, "key list", e)
		}
		// Creation, modification and issuance are denied.
		if _, e := f.client.CreateProject(ctx, as(login(owner), &pb.CreateProjectRequest{IdempotencyKey: idem(), TenantId: tenant, Name: "no"})); code(e) != connect.CodePermissionDenied {
			t.Fatal(state, "create allowed", e)
		}
		if _, e := f.client.UpdateProject(ctx, as(login(owner), &pb.UpdateProjectRequest{IdempotencyKey: idem(), ProjectId: p.Id, Name: "no", Version: 1})); code(e) != connect.CodePermissionDenied {
			t.Fatal(state, "update allowed", e)
		}
		if _, e := f.client.IssueApiKey(ctx, as(login(owner), &pb.IssueApiKeyRequest{IdempotencyKey: idem(), ProjectId: p.Id, Permission: pb.ApiKeyPermission_API_KEY_PERMISSION_READ_ONLY})); code(e) != connect.CodePermissionDenied {
			t.Fatal(state, "issue allowed", e)
		}
	}
	// UNKNOWN and DELETED deny everything, including management reads.
	for _, state := range []delcore.State{delcore.StateUnknown, delcore.StateDeleted} {
		f.fake.setState(tenant, state)
		if _, e := f.client.GetProject(ctx, as(login(owner), &pb.GetProjectRequest{ProjectId: p.Id})); code(e) != connect.CodePermissionDenied {
			t.Fatal(state, "read allowed", e)
		}
		if _, e := f.client.DeleteProject(ctx, as(login(owner), &pb.DeleteProjectRequest{IdempotencyKey: idem(), ProjectId: p.Id, Version: 1})); code(e) != connect.CodePermissionDenied {
			t.Fatal(state, "delete allowed", e)
		}
	}
	// A state-check failure is unavailable, never access.
	f.fake.setState(tenant, delcore.StateActive)
	f.fake.mu.Lock()
	f.fake.stateErr = delcore.ErrUnavailable
	f.fake.mu.Unlock()
	if _, e := f.client.GetProject(ctx, as(login(owner), &pb.GetProjectRequest{ProjectId: p.Id})); code(e) != connect.CodeUnavailable {
		t.Fatal("state failure did not deny", e)
	}
	f.fake.mu.Lock()
	f.fake.stateErr = nil
	f.fake.mu.Unlock()
	// Revocation and deletion work while blocked.
	f.fake.setState(tenant, delcore.StateBlocked)
	if _, e := f.client.RevokeApiKey(ctx, as(login(owner), &pb.RevokeApiKeyRequest{IdempotencyKey: idem(), ProjectId: p.Id, ApiKeyId: apiKey.Id})); e != nil {
		t.Fatal("revoke while blocked", e)
	}
	if _, e := f.client.DeleteProject(ctx, as(login(owner), &pb.DeleteProjectRequest{IdempotencyKey: idem(), ProjectId: p.Id, Version: 1})); e != nil {
		t.Fatal("delete while blocked", e)
	}
}

func TestDependencyFailuresDenyAndDoNotCommit(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	f.fake.setFail(delcore.ErrUnavailable)
	_, e := f.client.CreateProject(ctx, as(login(owner), &pb.CreateProjectRequest{IdempotencyKey: idem(), TenantId: tenant, Name: "Never"}))
	if code(e) != connect.CodeUnavailable {
		t.Fatal(e)
	}
	var detail *pb.ErrorDetail
	for _, d := range e.(*connect.Error).Details() {
		if v, err := d.Value(); err == nil {
			detail, _ = v.(*pb.ErrorDetail)
		}
	}
	if detail == nil || detail.TraceId == "" || detail.Reason != pb.ErrorReason_ERROR_REASON_DEPENDENCY_UNAVAILABLE {
		t.Fatal("unavailable error lacks a typed trace ID", detail)
	}
	if n := f.count("SELECT count(*) FROM projects"); n != 0 {
		t.Fatal("an uncommitted mutation left data", n)
	}
	f.fake.setFail(nil)
	f.project(login(owner), tenant, "Now it works")
}

func TestProjectLimitUnderConcurrency(t *testing.T) {
	f := setup(t, func(l *config.Limits) { l.MaxProjectsPerTenant = 5 })
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	var wg sync.WaitGroup
	results := make(chan error, 20)
	for i := 0; i < 20; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_, e := f.client.CreateProject(ctx, as(login(owner), &pb.CreateProjectRequest{IdempotencyKey: idem(), TenantId: tenant, Name: "p" + newID()}))
			results <- e
		}()
	}
	wg.Wait()
	close(results)
	ok, exhausted := 0, 0
	for e := range results {
		switch code(e) {
		case 0:
			ok++
		case connect.CodeResourceExhausted:
			exhausted++
		default:
			t.Fatal(e)
		}
	}
	if ok != 5 || exhausted != 15 || f.count("SELECT count(*) FROM projects") != 5 {
		t.Fatalf("limit not enforced atomically: ok=%d exhausted=%d", ok, exhausted)
	}
}

func TestDuplicateProjectNamesRace(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	var wg sync.WaitGroup
	results := make(chan error, 10)
	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_, e := f.client.CreateProject(ctx, as(login(owner), &pb.CreateProjectRequest{IdempotencyKey: idem(), TenantId: tenant, Name: "Same"}))
			results <- e
		}()
	}
	wg.Wait()
	close(results)
	ok := 0
	for e := range results {
		if e == nil {
			ok++
		} else if code(e) != connect.CodeAlreadyExists {
			t.Fatal(e)
		}
	}
	if ok != 1 {
		t.Fatalf("expected exactly one winner, got %d", ok)
	}
}

func TestProjectPagination(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	want := map[string]bool{}
	for i := 0; i < 7; i++ {
		want[f.project(login(owner), tenant, "p"+string(rune('a'+i))).Id] = true
	}
	var seen []string
	token := ""
	for pages := 0; ; pages++ {
		r, e := f.client.ListProjects(ctx, as(login(owner), &pb.ListProjectsRequest{TenantId: tenant, PageSize: 3, PageToken: token}))
		if e != nil {
			t.Fatal(e)
		}
		for _, p := range r.Msg.Projects {
			seen = append(seen, p.Id)
		}
		if token = r.Msg.NextPageToken; token == "" {
			if pages != 2 {
				t.Fatalf("expected 3 pages, got %d", pages+1)
			}
			break
		}
	}
	if len(seen) != 7 {
		t.Fatal("pages did not cover all projects", len(seen))
	}
	for _, id := range seen {
		if !want[id] {
			t.Fatal("unexpected project", id)
		}
		delete(want, id)
	}
	for _, bad := range []*pb.ListProjectsRequest{{TenantId: tenant, PageSize: 101}, {TenantId: tenant, PageSize: -1}, {TenantId: tenant, PageToken: "garbage"}, {TenantId: tenant, PageToken: "MTIz"}} {
		if _, e := f.client.ListProjects(ctx, as(login(owner), bad)); code(e) != connect.CodeInvalidArgument {
			t.Fatalf("bad page request accepted: %v", bad)
		}
	}
	r, e := f.client.ListProjects(ctx, as(login(owner), &pb.ListProjectsRequest{TenantId: tenant}))
	if e != nil || len(r.Msg.Projects) != 7 || r.Msg.NextPageToken != "" {
		t.Fatal("default page", e)
	}
}

// Name uniqueness ignores case through an explicit ICU collation, not the database locale, and
// keeps accents distinct.
func TestProjectNamesAreUniqueIgnoringCaseWithoutDatabaseLocale(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	create := func(name string) error {
		_, e := f.client.CreateProject(ctx, as(login(owner), &pb.CreateProjectRequest{IdempotencyKey: idem(), TenantId: tenant, Name: name}))
		return e
	}
	for _, pair := range [][2]string{{"Alpha", "aLPHA"}, {"INDIGO", "indigo"}, {"Éclair", "éclair"}, {"Ωmega", "ωMEGA"}} {
		if e := create(pair[0]); e != nil {
			t.Fatal(pair[0], e)
		}
		if e := create(pair[1]); code(e) != connect.CodeAlreadyExists {
			t.Fatalf("%q was accepted next to %q: %v", pair[1], pair[0], e)
		}
	}
	// Accents are letters, not case: a different name stays allowed.
	if e := create("Eclair"); e != nil {
		t.Fatal("an unaccented name conflicted with an accented one", e)
	}
	// Renaming into a case-insensitive duplicate conflicts too.
	other := f.project(login(owner), tenant, "Zeta")
	if _, e := f.client.UpdateProject(ctx, as(login(owner), &pb.UpdateProjectRequest{IdempotencyKey: idem(), ProjectId: other.Id, Name: "ALPHA", Version: other.Version})); code(e) != connect.CodeAlreadyExists {
		t.Fatal("a rename into a case-insensitive duplicate was accepted", e)
	}
}
