// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package integration

import (
	"context"
	"strings"
	"sync"
	"testing"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/devbase/v1"
	"github.com/delinoio/apps/servers/devbase-api/internal/config"
	"github.com/delinoio/apps/servers/devbase-api/internal/delcore"
	"google.golang.org/protobuf/types/known/fieldmaskpb"
	"google.golang.org/protobuf/types/known/structpb"
	"google.golang.org/protobuf/types/known/timestamppb"
)

const (
	readOnly = pb.ApiKeyPermission_API_KEY_PERMISSION_READ_ONLY
	crud     = pb.ApiKeyPermission_API_KEY_PERMISSION_CRUD
)

func mask(paths ...string) *fieldmaskpb.FieldMask { return &fieldmaskpb.FieldMask{Paths: paths} }

func meta(m map[string]any) *structpb.Struct {
	s, e := structpb.NewStruct(m)
	if e != nil {
		panic(e)
	}
	return s
}

func TestEndUserLifecycleWithLoginAndKeys(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, admin := f.tenant()
	p := f.project(login(owner), tenant, "Directory")
	secret, _ := f.key(login(owner), p.Id, crud)
	for _, token := range []string{login(owner), login(admin), secret} {
		external := "ext-" + newID()
		created, e := f.client.CreateEndUser(ctx, as(token, &pb.CreateEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, ExternalId: external, Name: "Ada", Email: "ada@example.com", Metadata: meta(map[string]any{"plan": "pro"})}))
		if e != nil {
			t.Fatal("create", e)
		}
		u := created.Msg.EndUser
		if !canonical(u.Id) || u.Version != 1 || u.ProjectId != p.Id || u.Metadata.Fields["plan"].GetStringValue() != "pro" {
			t.Fatal("unexpected user", u)
		}
		byID, e := f.client.GetEndUser(ctx, as(token, &pb.GetEndUserRequest{ProjectId: p.Id, EndUserId: u.Id}))
		byExternal, e2 := f.client.LookupEndUser(ctx, as(token, &pb.LookupEndUserRequest{ProjectId: p.Id, ExternalId: external}))
		if e != nil || e2 != nil || byID.Msg.EndUser.Id != u.Id || byExternal.Msg.EndUser.Id != u.Id {
			t.Fatal("lookups", e, e2)
		}
		upd, e := f.client.UpdateEndUser(ctx, as(token, &pb.UpdateEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, EndUserId: u.Id, Name: "Ada L.", UpdateMask: mask("name"), Version: 1}))
		if e != nil || upd.Msg.EndUser.Version != 2 || upd.Msg.EndUser.Name != "Ada L." || upd.Msg.EndUser.Email != "ada@example.com" || upd.Msg.EndUser.Metadata.Fields["plan"].GetStringValue() != "pro" {
			t.Fatal("partial update changed other fields", e, upd)
		}
		if _, e = f.client.DeleteEndUser(ctx, as(token, &pb.DeleteEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, EndUserId: u.Id, Version: 2})); e != nil {
			t.Fatal("delete", e)
		}
		if _, e = f.client.GetEndUser(ctx, as(token, &pb.GetEndUserRequest{ProjectId: p.Id, EndUserId: u.Id})); code(e) != connect.CodeNotFound {
			t.Fatal("deleted user readable", e)
		}
	}
	if n := f.count("SELECT count(*) FROM end_users"); n != 0 {
		t.Fatal("rows remain", n)
	}
	if n := f.count("SELECT user_count FROM projects WHERE id=$1", p.Id); n != 0 {
		t.Fatal("quota counter drifted", n)
	}
}

func TestExternalIDRules(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	a := f.project(login(owner), tenant, "A")
	b := f.project(login(owner), tenant, "B")
	first := f.user(login(owner), a.Id, "Case")
	// Duplicate within a project fails; the same ID in another project and a different case are fine.
	if _, e := f.client.CreateEndUser(ctx, as(login(owner), &pb.CreateEndUserRequest{IdempotencyKey: idem(), ProjectId: a.Id, ExternalId: "Case"})); code(e) != connect.CodeAlreadyExists {
		t.Fatal("duplicate external ID accepted", e)
	}
	f.user(login(owner), b.Id, "Case")
	f.user(login(owner), a.Id, "case")
	if f.count("SELECT user_count FROM projects WHERE id=$1", a.Id) != 2 {
		t.Fatal("failed creation changed the counter")
	}
	// Duplicate emails are allowed.
	for _, external := range []string{"d1", "d2"} {
		if _, e := f.client.CreateEndUser(ctx, as(login(owner), &pb.CreateEndUserRequest{IdempotencyKey: idem(), ProjectId: a.Id, ExternalId: external, Email: "same@example.com"})); e != nil {
			t.Fatal(e)
		}
	}
	// An external ID can change without changing the UUID, and the old ID becomes free.
	upd, e := f.client.UpdateEndUser(ctx, as(login(owner), &pb.UpdateEndUserRequest{IdempotencyKey: idem(), ProjectId: a.Id, EndUserId: first.Id, ExternalId: "Renamed", UpdateMask: mask("external_id"), Version: 1}))
	if e != nil || upd.Msg.EndUser.Id != first.Id || upd.Msg.EndUser.ExternalId != "Renamed" {
		t.Fatal(e)
	}
	if _, e = f.client.LookupEndUser(ctx, as(login(owner), &pb.LookupEndUserRequest{ProjectId: a.Id, ExternalId: "Case"})); code(e) != connect.CodeNotFound {
		t.Fatal("old external ID still resolves", e)
	}
	reused := f.user(login(owner), a.Id, "Case")
	if reused.Id == first.Id || reused.Id == "" {
		t.Fatal("reused external ID kept the old identity")
	}
	// Renaming onto an existing ID conflicts.
	if _, e = f.client.UpdateEndUser(ctx, as(login(owner), &pb.UpdateEndUserRequest{IdempotencyKey: idem(), ProjectId: a.Id, EndUserId: first.Id, ExternalId: "Case", UpdateMask: mask("external_id"), Version: 2})); code(e) != connect.CodeAlreadyExists {
		t.Fatal("rename onto an existing ID accepted", e)
	}
	// Deleted external IDs are reusable immediately with a new UUID.
	if _, e = f.client.DeleteEndUser(ctx, as(login(owner), &pb.DeleteEndUserRequest{IdempotencyKey: idem(), ProjectId: a.Id, EndUserId: reused.Id, Version: 1})); e != nil {
		t.Fatal(e)
	}
	again := f.user(login(owner), a.Id, "Case")
	if again.Id == reused.Id || again.Id == first.Id {
		t.Fatal("deleted identity carried over")
	}
}

func TestEndUserValidation(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, "Validation")
	create := func(r *pb.CreateEndUserRequest) error {
		r.IdempotencyKey, r.ProjectId = idem(), p.Id
		_, e := f.client.CreateEndUser(ctx, as(login(owner), r))
		return e
	}
	big := func(n int) string { return strings.Repeat("x", n) }
	for name, r := range map[string]*pb.CreateEndUserRequest{
		"empty id":         {ExternalId: ""},
		"leading space":    {ExternalId: " a"},
		"trailing space":   {ExternalId: "a\t"},
		"long id":          {ExternalId: big(257)},
		"long name":        {ExternalId: "n1", Name: big(257)},
		"bad email":        {ExternalId: "e1", Email: "not-an-email"},
		"long email":       {ExternalId: "e2", Email: big(250) + "@b.cd"},
		"oversize meta":    {ExternalId: "m1", Metadata: meta(map[string]any{"k": big(16 << 10)})},
		"oversize request": {ExternalId: "r1", Name: big(256), Metadata: meta(map[string]any{"a": big(10 << 10), "b": big(10 << 10), "c": big(10 << 10), "d": big(10 << 10), "e": big(10 << 10), "f": big(10 << 10), "g": big(10 << 10)})},
	} {
		if e := create(r); code(e) != connect.CodeInvalidArgument {
			t.Errorf("%s accepted: %v", name, e)
		}
	}
	// Boundaries are accepted: 256-byte ID and name, 254-byte email, 16 KiB metadata.
	local := strings.Repeat("a", 64)
	email254 := local + "@" + strings.Repeat("b", 63) + "." + strings.Repeat("c", 63) + "." + strings.Repeat("d", 57) + ".com"
	if len(email254) != 254 {
		t.Fatalf("test email has %d bytes", len(email254))
	}
	fits := meta(map[string]any{"k": big(16<<10 - len(`{"k":""}`))})
	if e := create(&pb.CreateEndUserRequest{ExternalId: big(256), Name: big(256), Email: email254, Metadata: fits}); e != nil {
		t.Fatal("boundary values rejected", e)
	}
	// Multi-byte characters count as bytes, not characters.
	if e := create(&pb.CreateEndUserRequest{ExternalId: strings.Repeat("é", 129)}); code(e) != connect.CodeInvalidArgument {
		t.Fatal("258-byte ID accepted", e)
	}
	if e := create(&pb.CreateEndUserRequest{ExternalId: strings.Repeat("é", 128)}); e != nil {
		t.Fatal(e)
	}
	// Lookups validate too.
	if _, e := f.client.LookupEndUser(ctx, as(login(owner), &pb.LookupEndUserRequest{ProjectId: p.Id, ExternalId: " "})); code(e) != connect.CodeInvalidArgument {
		t.Fatal(e)
	}
	if _, e := f.client.GetEndUser(ctx, as(login(owner), &pb.GetEndUserRequest{ProjectId: p.Id, EndUserId: "x"})); code(e) != connect.CodeInvalidArgument {
		t.Fatal(e)
	}
}

func TestFieldMasksVersionsAndClears(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, "Masks")
	created, e := f.client.CreateEndUser(ctx, as(login(owner), &pb.CreateEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, ExternalId: "m", Name: "Name", Email: "m@example.com", Metadata: meta(map[string]any{"a": 1, "b": 2})}))
	if e != nil {
		t.Fatal(e)
	}
	u := created.Msg.EndUser
	update := func(r *pb.UpdateEndUserRequest) (*pb.EndUser, error) {
		r.IdempotencyKey, r.ProjectId, r.EndUserId = idem(), p.Id, u.Id
		out, e := f.client.UpdateEndUser(ctx, as(login(owner), r))
		if e != nil {
			return nil, e
		}
		return out.Msg.EndUser, nil
	}
	for name, r := range map[string]*pb.UpdateEndUserRequest{
		"empty mask":     {Name: "x", Version: 1},
		"unknown path":   {UpdateMask: mask("version"), Version: 1},
		"id path":        {UpdateMask: mask("id"), Version: 1},
		"clear required": {UpdateMask: mask("external_id"), Version: 1},
		"bad email":      {UpdateMask: mask("email"), Email: "nope", Version: 1},
		"long name":      {UpdateMask: mask("name"), Name: strings.Repeat("n", 257), Version: 1},
	} {
		if _, e := update(r); code(e) != connect.CodeInvalidArgument {
			t.Errorf("%s accepted: %v", name, e)
		}
	}
	// Unmasked fields in the request are ignored.
	got, e := update(&pb.UpdateEndUserRequest{UpdateMask: mask("name"), Name: "New", Email: "ignored@example.com", Version: 1})
	if e != nil || got.Name != "New" || got.Email != "m@example.com" {
		t.Fatal("unmasked field applied", got, e)
	}
	// Name and email can be cleared; metadata is replaced as a whole.
	got, e = update(&pb.UpdateEndUserRequest{UpdateMask: mask("name", "email"), Version: 2})
	if e != nil || got.Name != "" || got.Email != "" || got.Version != 3 {
		t.Fatal("clear failed", got, e)
	}
	got, e = update(&pb.UpdateEndUserRequest{UpdateMask: mask("metadata"), Metadata: meta(map[string]any{"c": 3}), Version: 3})
	if e != nil || len(got.Metadata.Fields) != 1 || got.Metadata.Fields["c"].GetNumberValue() != 3 {
		t.Fatal("metadata was merged instead of replaced", got, e)
	}
	got, e = update(&pb.UpdateEndUserRequest{UpdateMask: mask("metadata"), Metadata: &structpb.Struct{}, Version: 4})
	if e != nil || len(got.Metadata.Fields) != 0 {
		t.Fatal("empty object did not clear metadata", got, e)
	}
	// Stale versions are aborted for updates and deletes, and nothing changes.
	if _, e = update(&pb.UpdateEndUserRequest{UpdateMask: mask("name"), Name: "stale", Version: 1}); code(e) != connect.CodeAborted {
		t.Fatal("stale update accepted", e)
	}
	if _, e = f.client.DeleteEndUser(ctx, as(login(owner), &pb.DeleteEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, EndUserId: u.Id, Version: 1})); code(e) != connect.CodeAborted {
		t.Fatal("stale delete accepted", e)
	}
	// Unknown users and another project's users are not found.
	other := f.project(login(owner), tenant, "Other")
	if _, e = f.client.UpdateEndUser(ctx, as(login(owner), &pb.UpdateEndUserRequest{IdempotencyKey: idem(), ProjectId: other.Id, EndUserId: u.Id, UpdateMask: mask("name"), Version: 5})); code(e) != connect.CodeNotFound {
		t.Fatal("updated through another project", e)
	}
	if _, e = f.client.DeleteEndUser(ctx, as(login(owner), &pb.DeleteEndUserRequest{IdempotencyKey: idem(), ProjectId: other.Id, EndUserId: u.Id, Version: 5})); code(e) != connect.CodeNotFound {
		t.Fatal("deleted through another project", e)
	}
}

func TestKeyPermissionsExpiryAndRevocation(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	other, otherOwner, _ := f.tenant()
	p := f.project(login(owner), tenant, "Keys")
	q := f.project(login(otherOwner), other, "Foreign")
	reader, _ := f.key(login(owner), p.Id, readOnly)
	writer, writerKey := f.key(login(owner), p.Id, crud)
	u := f.user(login(owner), p.Id, "target")

	// Read-only keys read but never mutate.
	if _, e := f.client.GetEndUser(ctx, as(reader, &pb.GetEndUserRequest{ProjectId: p.Id, EndUserId: u.Id})); e != nil {
		t.Fatal(e)
	}
	if _, e := f.client.ListEndUsers(ctx, as(reader, &pb.ListEndUsersRequest{ProjectId: p.Id})); e != nil {
		t.Fatal(e)
	}
	mutations := []func(string) error{
		func(token string) error {
			_, e := f.client.CreateEndUser(ctx, as(token, &pb.CreateEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, ExternalId: "w"}))
			return e
		},
		func(token string) error {
			_, e := f.client.UpdateEndUser(ctx, as(token, &pb.UpdateEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, EndUserId: u.Id, UpdateMask: mask("name"), Name: "x", Version: 1}))
			return e
		},
		func(token string) error {
			_, e := f.client.DeleteEndUser(ctx, as(token, &pb.DeleteEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, EndUserId: u.Id, Version: 1}))
			return e
		},
	}
	for i, mutate := range mutations {
		if e := mutate(reader); code(e) != connect.CodePermissionDenied {
			t.Fatalf("read-only key mutated (%d): %v", i, e)
		}
	}
	if f.count("SELECT count(*) FROM end_users") != 1 {
		t.Fatal("read-only key changed data")
	}
	// A key never crosses its project, even into a project it could otherwise read.
	if _, e := f.client.GetEndUser(ctx, as(writer, &pb.GetEndUserRequest{ProjectId: q.Id, EndUserId: u.Id})); code(e) != connect.CodeNotFound {
		t.Fatal("key crossed projects", e)
	}
	if _, e := f.client.CreateEndUser(ctx, as(writer, &pb.CreateEndUserRequest{IdempotencyKey: idem(), ProjectId: q.Id, ExternalId: "x"})); code(e) != connect.CodeNotFound {
		t.Fatal("key wrote across projects", e)
	}
	// Unknown, malformed and wrong-secret keys are unauthenticated.
	wrong := writer[:len(writer)-1] + map[bool]string{true: "A", false: "B"}[writer[len(writer)-1] != 'A']
	for _, token := range []string{wrong, "dbk_" + strings.Repeat("0", 32) + "_" + strings.Repeat("A", 43), "dbk_short"} {
		if _, e := f.client.GetEndUser(ctx, as(token, &pb.GetEndUserRequest{ProjectId: p.Id, EndUserId: u.Id})); code(e) != connect.CodeUnauthenticated {
			t.Fatalf("token accepted: %v", e)
		}
	}
	// Expiry: a key stops working at its expiry time.
	exp, e := f.client.IssueApiKey(ctx, as(login(owner), &pb.IssueApiKeyRequest{IdempotencyKey: idem(), ProjectId: p.Id, Permission: crud, ExpireTime: timestamppb.New(f.clock.now().Add(time.Hour))}))
	if e != nil {
		t.Fatal(e)
	}
	if _, e = f.client.ListEndUsers(ctx, as(exp.Msg.Secret, &pb.ListEndUsersRequest{ProjectId: p.Id})); e != nil {
		t.Fatal(e)
	}
	f.clock.advance(time.Hour + time.Second)
	if _, e = f.client.ListEndUsers(ctx, as(exp.Msg.Secret, &pb.ListEndUsersRequest{ProjectId: p.Id})); code(e) != connect.CodeUnauthenticated {
		t.Fatal("expired key accepted", e)
	}
	// Revocation takes effect immediately and permanently.
	if _, e = f.client.RevokeApiKey(ctx, as(login(owner), &pb.RevokeApiKeyRequest{IdempotencyKey: idem(), ProjectId: p.Id, ApiKeyId: writerKey.Id})); e != nil {
		t.Fatal(e)
	}
	if _, e = f.client.ListEndUsers(ctx, as(writer, &pb.ListEndUsersRequest{ProjectId: p.Id})); code(e) != connect.CodeUnauthenticated {
		t.Fatal("revoked key accepted", e)
	}
	if e := mutations[0](writer); code(e) != connect.CodeUnauthenticated {
		t.Fatal("revoked key mutated", e)
	}
}

func TestRevocationAndInFlightKeyWork(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, "Revoke")
	secret, key := f.key(login(owner), p.Id, crud)
	var wg sync.WaitGroup
	results := make(chan error, 40)
	for i := 0; i < 40; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			_, e := f.client.CreateEndUser(ctx, as(secret, &pb.CreateEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, ExternalId: "u" + newID()}))
			results <- e
		}(i)
	}
	if _, e := f.client.RevokeApiKey(ctx, as(login(owner), &pb.RevokeApiKeyRequest{IdempotencyKey: idem(), ProjectId: p.Id, ApiKeyId: key.Id})); e != nil {
		t.Fatal(e)
	}
	wg.Wait()
	close(results)
	committed := 0
	for e := range results {
		switch code(e) {
		case 0:
			committed++
		case connect.CodeUnauthenticated:
		default:
			t.Fatal(e)
		}
	}
	// Work that committed before the revocation is kept; nothing starts afterward.
	if got := f.count("SELECT count(*) FROM end_users"); got != committed {
		t.Fatalf("committed=%d rows=%d", committed, got)
	}
	if _, e := f.client.CreateEndUser(ctx, as(secret, &pb.CreateEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, ExternalId: "after"})); code(e) != connect.CodeUnauthenticated {
		t.Fatal("work started after revocation", e)
	}
}

func TestTenantStateForKeysAndDeletion(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, "State")
	secret, _ := f.key(login(owner), p.Id, crud)
	u := f.user(login(owner), p.Id, "kept")
	for _, state := range []delcore.State{delcore.StateBlocked, delcore.StateDeleting, delcore.StateUnknown, delcore.StateDeleted} {
		f.fake.setState(tenant, state)
		if _, e := f.client.GetEndUser(ctx, as(secret, &pb.GetEndUserRequest{ProjectId: p.Id, EndUserId: u.Id})); code(e) != connect.CodePermissionDenied {
			t.Fatal(state, "key read allowed", e)
		}
		if _, e := f.client.CreateEndUser(ctx, as(secret, &pb.CreateEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, ExternalId: "no"})); code(e) != connect.CodePermissionDenied {
			t.Fatal(state, "key write allowed", e)
		}
		// General user reads and writes are denied for login tokens as well.
		if _, e := f.client.GetEndUser(ctx, as(login(owner), &pb.GetEndUserRequest{ProjectId: p.Id, EndUserId: u.Id})); code(e) != connect.CodePermissionDenied {
			t.Fatal(state, "login read allowed", e)
		}
		if _, e := f.client.UpdateEndUser(ctx, as(login(owner), &pb.UpdateEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, EndUserId: u.Id, UpdateMask: mask("name"), Name: "x", Version: 1})); code(e) != connect.CodePermissionDenied {
			t.Fatal(state, "login update allowed", e)
		}
	}
	f.fake.mu.Lock()
	f.fake.stateErr = delcore.ErrUnavailable
	f.fake.mu.Unlock()
	f.fake.setState(tenant, delcore.StateActive)
	if _, e := f.client.GetEndUser(ctx, as(secret, &pb.GetEndUserRequest{ProjectId: p.Id, EndUserId: u.Id})); code(e) != connect.CodeUnavailable {
		t.Fatal("key access survived a state failure", e)
	}
	f.fake.mu.Lock()
	f.fake.stateErr = nil
	f.fake.mu.Unlock()
	// Owners and Admins may still delete users while the tenant is blocked or deleting.
	for _, state := range []delcore.State{delcore.StateBlocked, delcore.StateDeleting} {
		f.fake.setState(tenant, state)
		v := f.userAt(t, tenant, owner, p.Id)
		if _, e := f.client.DeleteEndUser(ctx, as(login(owner), &pb.DeleteEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, EndUserId: v.Id, Version: 1})); e != nil {
			t.Fatal(state, "delete denied", e)
		}
		f.fake.setState(tenant, delcore.StateActive)
	}
	// Billing conditions never matter: the fake exposes only operator and deletion state.
	f.fake.setState(tenant, delcore.StateActive)
	if _, e := f.client.GetEndUser(ctx, as(secret, &pb.GetEndUserRequest{ProjectId: p.Id, EndUserId: u.Id})); e != nil {
		t.Fatal(e)
	}
}

// userAt creates a user while the tenant is active and leaves the previous state intact.
func (f *fixture) userAt(t *testing.T, tenant, owner, project string) *pb.EndUser {
	t.Helper()
	f.fake.mu.Lock()
	previous := f.fake.states[tenant]
	f.fake.mu.Unlock()
	f.fake.setState(tenant, delcore.StateActive)
	u := f.user(login(owner), project, "tmp-"+newID())
	f.fake.setState(tenant, previous)
	return u
}

func TestEndUserPagination(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, "Pages")
	want := map[string]bool{}
	for i := 0; i < 11; i++ {
		want[f.user(login(owner), p.Id, "p"+newID()).Id] = true
	}
	token, pages := "", 0
	for {
		r, e := f.client.ListEndUsers(ctx, as(login(owner), &pb.ListEndUsersRequest{ProjectId: p.Id, PageSize: 4, PageToken: token}))
		if e != nil {
			t.Fatal(e)
		}
		pages++
		for _, u := range r.Msg.EndUsers {
			if !want[u.Id] {
				t.Fatal("duplicate or foreign user", u.Id)
			}
			delete(want, u.Id)
		}
		if token = r.Msg.NextPageToken; token == "" {
			break
		}
	}
	if len(want) != 0 || pages != 3 {
		t.Fatalf("pages=%d missing=%d", pages, len(want))
	}
	for _, bad := range []*pb.ListEndUsersRequest{{ProjectId: p.Id, PageSize: 101}, {ProjectId: p.Id, PageToken: "bogus"}} {
		if _, e := f.client.ListEndUsers(ctx, as(login(owner), bad)); code(e) != connect.CodeInvalidArgument {
			t.Fatal("bad list request accepted", bad)
		}
	}
	// A project's list never includes another project's users.
	q := f.project(login(owner), tenant, "Empty")
	r, e := f.client.ListEndUsers(ctx, as(login(owner), &pb.ListEndUsersRequest{ProjectId: q.Id}))
	if e != nil || len(r.Msg.EndUsers) != 0 {
		t.Fatal("cross-project list", e)
	}
}

func TestCreateReplayAndDeletedResourceReplay(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, "Replays")
	secret, _ := f.key(login(owner), p.Id, crud)
	create := &pb.CreateEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, ExternalId: "private-external-id", Name: "Private Person", Email: "private@example.com", Metadata: meta(map[string]any{"secret-field": "secret-value"})}
	first, e := f.client.CreateEndUser(ctx, as(secret, create))
	if e != nil {
		t.Fatal(e)
	}
	u := first.Msg.EndUser
	// A lost response is resolved by repeating the request with the same key.
	again, e := f.client.CreateEndUser(ctx, as(secret, create))
	if e != nil || again.Msg.EndUser.Id != u.Id || again.Msg.EndUser.Email != "private@example.com" {
		t.Fatal("replay", e)
	}
	if f.count("SELECT count(*) FROM end_users") != 1 {
		t.Fatal("replay created a second user")
	}
	update := &pb.UpdateEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, EndUserId: u.Id, UpdateMask: mask("name"), Name: "Renamed Person", Version: 1}
	if _, e = f.client.UpdateEndUser(ctx, as(secret, update)); e != nil {
		t.Fatal(e)
	}
	del := &pb.DeleteEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, EndUserId: u.Id, Version: 2}
	if _, e = f.client.DeleteEndUser(ctx, as(secret, del)); e != nil {
		t.Fatal(e)
	}
	// Personal data is gone from operational and replay storage.
	for _, table := range []string{"end_users", "replays", "audit_events", "projects", "api_keys"} {
		var dump string
		if e = f.db.Pool.QueryRow(ctx, "SELECT coalesce(string_agg(t::text,' '),'') FROM "+table+" t").Scan(&dump); e != nil {
			t.Fatal(e)
		}
		for _, personal := range []string{"private-external-id", "Private Person", "Renamed Person", "private@example.com", "secret-value", "secret-field"} {
			if strings.Contains(dump, personal) {
				t.Fatalf("%q survives in %s", personal, table)
			}
		}
	}
	// Replays of the deleted user's create and update are not found and restore nothing.
	if _, e = f.client.CreateEndUser(ctx, as(secret, create)); code(e) != connect.CodeNotFound {
		t.Fatal("deleted create replayed", e)
	}
	if _, e = f.client.UpdateEndUser(ctx, as(secret, update)); code(e) != connect.CodeNotFound {
		t.Fatal("deleted update replayed", e)
	}
	if f.count("SELECT count(*) FROM end_users") != 0 {
		t.Fatal("deleted user resurrected")
	}
	// Repeating the delete is idempotent.
	if _, e = f.client.DeleteEndUser(ctx, as(secret, del)); e != nil {
		t.Fatal("repeated delete failed", e)
	}
	// A deliberate new creation with a reused external ID and a new key still works.
	reborn, e := f.client.CreateEndUser(ctx, as(secret, &pb.CreateEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, ExternalId: "private-external-id"}))
	if e != nil || reborn.Msg.EndUser.Id == u.Id {
		t.Fatal("new creation blocked or reused identity", e)
	}
}

func TestReplayExpiryForUsers(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, "Expiry")
	req := &pb.CreateEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, ExternalId: "once"}
	if _, e := f.client.CreateEndUser(ctx, as(login(owner), req)); e != nil {
		t.Fatal(e)
	}
	f.clock.advance(25 * time.Hour)
	// After 24 hours the key is a new request, which conflicts with the existing external ID.
	if _, e := f.client.CreateEndUser(ctx, as(login(owner), req)); code(e) != connect.CodeAlreadyExists {
		t.Fatal("expired key replayed", e)
	}
}

func TestDifferingInputWithSameKeyForUsers(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, "Differing")
	req := &pb.CreateEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, ExternalId: "one"}
	if _, e := f.client.CreateEndUser(ctx, as(login(owner), req)); e != nil {
		t.Fatal(e)
	}
	changed := &pb.CreateEndUserRequest{IdempotencyKey: req.IdempotencyKey, ProjectId: p.Id, ExternalId: "two"}
	if _, e := f.client.CreateEndUser(ctx, as(login(owner), changed)); code(e) != connect.CodeAlreadyExists {
		t.Fatal("differing input accepted", e)
	}
	if f.count("SELECT count(*) FROM end_users") != 1 {
		t.Fatal("differing retry executed")
	}
}

func TestUserQuotaAndDuplicateRaces(t *testing.T) {
	f := setup(t, func(l *config.Limits) { l.MaxUsersPerProject = 5; l.UserRate = 1000; l.UserBurst = 2000 })
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, "Quota")
	secret, _ := f.key(login(owner), p.Id, crud)
	var wg sync.WaitGroup
	results := make(chan error, 30)
	for i := 0; i < 30; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_, e := f.client.CreateEndUser(ctx, as(secret, &pb.CreateEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, ExternalId: "u" + newID()}))
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
	if ok != 5 || exhausted != 25 || f.count("SELECT count(*) FROM end_users") != 5 || f.count("SELECT user_count FROM projects WHERE id=$1", p.Id) != 5 {
		t.Fatalf("quota not atomic: ok=%d exhausted=%d", ok, exhausted)
	}
	// Deleting frees capacity.
	var victim string
	f.db.Pool.QueryRow(ctx, "SELECT id::text FROM end_users LIMIT 1").Scan(&victim)
	if _, e := f.client.DeleteEndUser(ctx, as(secret, &pb.DeleteEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, EndUserId: victim, Version: 1})); e != nil {
		t.Fatal(e)
	}
	f.user(login(owner), p.Id, "after-delete")

	// Duplicate external IDs race with distinct keys: exactly one wins.
	wins := make(chan error, 12)
	other := f.project(login(owner), tenant, "Dupes")
	token, _ := f.key(login(owner), other.Id, crud)
	for i := 0; i < 12; i++ {
		go func() {
			_, e := f.client.CreateEndUser(ctx, as(token, &pb.CreateEndUserRequest{IdempotencyKey: idem(), ProjectId: other.Id, ExternalId: "same"}))
			wins <- e
		}()
	}
	won := 0
	for i := 0; i < 12; i++ {
		if e := <-wins; e == nil {
			won++
		} else if code(e) != connect.CodeAlreadyExists {
			t.Fatal(e)
		}
	}
	if won != 1 || f.count("SELECT user_count FROM projects WHERE id=$1", other.Id) != 1 {
		t.Fatalf("duplicate race winners: %d", won)
	}
}

func TestConcurrentUpdatesAndDeletesSerialize(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, "Serialize")
	u := f.user(login(owner), p.Id, "contended")
	var wg sync.WaitGroup
	results := make(chan error, 10)
	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_, e := f.client.UpdateEndUser(ctx, as(login(owner), &pb.UpdateEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, EndUserId: u.Id, UpdateMask: mask("name"), Name: "n" + newID(), Version: 1}))
			results <- e
		}()
	}
	wg.Wait()
	close(results)
	ok := 0
	for e := range results {
		if e == nil {
			ok++
		} else if code(e) != connect.CodeAborted {
			t.Fatal(e)
		}
	}
	if ok != 1 {
		t.Fatalf("expected one update at version 1, got %d", ok)
	}
	// An update and a delete at the same version: one wins, and a deleted user never returns.
	errs := make(chan error, 2)
	go func() {
		_, e := f.client.UpdateEndUser(ctx, as(login(owner), &pb.UpdateEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, EndUserId: u.Id, UpdateMask: mask("name"), Name: "late", Version: 2}))
		errs <- e
	}()
	go func() {
		_, e := f.client.DeleteEndUser(ctx, as(login(owner), &pb.DeleteEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, EndUserId: u.Id, Version: 2}))
		errs <- e
	}()
	<-errs
	<-errs
	if _, e := f.client.UpdateEndUser(ctx, as(login(owner), &pb.UpdateEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, EndUserId: u.Id, UpdateMask: mask("name"), Name: "after", Version: 3})); code(e) == 0 && f.count("SELECT count(*) FROM end_users WHERE name='after'") == 0 {
		t.Fatal("update succeeded without a row")
	}
}

func TestProjectDeletionRacesWithUserAndKeyRequests(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, "Doomed")
	secret, _ := f.key(login(owner), p.Id, crud)
	f.user(login(owner), p.Id, "existing")
	var wg sync.WaitGroup
	results := make(chan error, 60)
	for i := 0; i < 30; i++ {
		wg.Add(2)
		go func() {
			defer wg.Done()
			_, e := f.client.CreateEndUser(ctx, as(login(owner), &pb.CreateEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, ExternalId: "n" + newID()}))
			results <- e
		}()
		go func() {
			defer wg.Done()
			_, e := f.client.IssueApiKey(ctx, as(login(owner), &pb.IssueApiKeyRequest{IdempotencyKey: idem(), ProjectId: p.Id, Permission: crud}))
			results <- e
		}()
	}
	if _, e := f.client.DeleteProject(ctx, as(login(owner), &pb.DeleteProjectRequest{IdempotencyKey: idem(), ProjectId: p.Id, Version: 1})); e != nil {
		t.Fatal(e)
	}
	wg.Wait()
	close(results)
	for e := range results {
		if c := code(e); c != 0 && c != connect.CodeNotFound && c != connect.CodeResourceExhausted {
			t.Fatal("unexpected outcome during deletion", e)
		}
	}
	// Nothing survives or is recreated after the deletion completes.
	for _, table := range []string{"projects", "end_users", "api_keys"} {
		if n := f.count("SELECT count(*) FROM " + table); n != 0 {
			t.Fatalf("%s retains %d rows after project deletion", table, n)
		}
	}
	if _, e := f.client.CreateEndUser(ctx, as(login(owner), &pb.CreateEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, ExternalId: "late"})); code(e) != connect.CodeNotFound {
		t.Fatal("user created in a deleted project", e)
	}
	if _, e := f.client.GetEndUser(ctx, as(secret, &pb.GetEndUserRequest{ProjectId: p.Id, EndUserId: newID()})); code(e) != connect.CodeUnauthenticated {
		t.Fatal("deleted project's key still works", e)
	}
	if n := f.count("SELECT count(*) FROM replays WHERE operation LIKE '%EndUser' AND (response IS NOT NULL OR length(fingerprint)>0)"); n != 0 {
		t.Fatal("project deletion left user replay payloads", n)
	}
}

func TestUserRateLimitAggregatesKeysAndLogin(t *testing.T) {
	f := setup(t, func(l *config.Limits) {
		l.UserRate = 2
		l.UserBurst = 6
		l.ManagementRate = 50
		l.ManagementBurst = 100
	})
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, "Rates")
	q := f.project(login(owner), tenant, "Other rates")
	var tokens []string
	for i := 0; i < 3; i++ {
		s, _ := f.key(login(owner), p.Id, readOnly)
		tokens = append(tokens, s)
	}
	tokens = append(tokens, login(owner))
	list := func(token, project string) error {
		_, e := f.client.ListEndUsers(ctx, as(token, &pb.ListEndUsersRequest{ProjectId: project}))
		return e
	}
	// Six requests spread across three keys and a login consume one shared bucket.
	for i := 0; i < 6; i++ {
		if e := list(tokens[i%len(tokens)], p.Id); e != nil {
			t.Fatal(i, e)
		}
	}
	for _, token := range tokens {
		e := list(token, p.Id)
		if code(e) != connect.CodeResourceExhausted {
			t.Fatal("extra credential bypassed the limit", e)
		}
		if e.(*connect.Error).Meta().Get("Retry-After") == "" {
			t.Fatal("no retry timing")
		}
	}
	// Management traffic uses its own bucket, and another project is independent.
	if _, e := f.client.GetProject(ctx, as(login(owner), &pb.GetProjectRequest{ProjectId: p.Id})); e != nil {
		t.Fatal("management blocked by user traffic", e)
	}
	if e := list(login(owner), q.Id); e != nil {
		t.Fatal("another project shares the bucket", e)
	}
	if f.metricsText(t, "devbase_limit_rejections_total") == "" {
		t.Fatal("limit rejections are not counted")
	}
}

func TestRequestTimeoutDeniesWithoutCommitting(t *testing.T) {
	f := setup(t, func(l *config.Limits) {
		l.RequestTimeout = 300 * time.Millisecond
		l.DependencyTimeout = 200 * time.Millisecond
	})
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, "Slow")
	f.fake.setDelay(2 * time.Second)
	start := time.Now()
	_, e := f.client.CreateEndUser(ctx, as(login(owner), &pb.CreateEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, ExternalId: "never"}))
	if code(e) != connect.CodeUnavailable || time.Since(start) > time.Second {
		t.Fatal("slow dependency was not cut off", e)
	}
	f.fake.setDelay(0)
	if f.count("SELECT count(*) FROM end_users") != 0 {
		t.Fatal("timed-out request committed work")
	}
}

func TestTelemetryAndAuditCarryNoSecretsOrPersonalData(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, "Telemetry")
	secret, key := f.key(login(owner), p.Id, crud)
	u, e := f.client.CreateEndUser(ctx, as(secret, &pb.CreateEndUserRequest{IdempotencyKey: idem(), ProjectId: p.Id, ExternalId: "pii-external", Name: "PII Name", Email: "pii@example.com", Metadata: meta(map[string]any{"pii": "pii-metadata"})}))
	if e != nil {
		t.Fatal(e)
	}
	f.client.GetEndUser(ctx, as(secret, &pb.GetEndUserRequest{ProjectId: p.Id, EndUserId: u.Msg.EndUser.Id}))
	f.client.GetEndUser(ctx, as("dbk_"+strings.Repeat("0", 32)+"_"+strings.Repeat("A", 43), &pb.GetEndUserRequest{ProjectId: p.Id, EndUserId: u.Msg.EndUser.Id}))
	logs := f.logs.String()
	metricsText := f.metricsText(t, "")
	for _, sensitive := range []string{"pii-external", "PII Name", "pii@example.com", "pii-metadata", secret, secret[len(secret)-43:], owner} {
		if strings.Contains(logs, sensitive) || strings.Contains(metricsText, sensitive) {
			t.Fatalf("telemetry contains %q", sensitive)
		}
	}
	for _, identifier := range []string{tenant, p.Id, key.Id, u.Msg.EndUser.Id} {
		if strings.Contains(metricsText, identifier) {
			t.Fatal("metrics label carries an identifier", identifier)
		}
	}
	if !strings.Contains(logs, "trace_id=") || !strings.Contains(logs, "procedure=CreateEndUser") {
		t.Fatal("request logs lack trace IDs or procedure names")
	}
	// The audit trail records actor, action, target and trace, never personal fields.
	rows, e := f.db.Pool.Query(ctx, "SELECT action,actor,target_id::text,trace_id FROM audit_events WHERE action LIKE 'end_user.%'")
	if e != nil {
		t.Fatal(e)
	}
	defer rows.Close()
	seen := 0
	for rows.Next() {
		var action, actor, target, trace string
		if e = rows.Scan(&action, &actor, &target, &trace); e != nil {
			t.Fatal(e)
		}
		if action != "end_user.create" || actor != "key:"+key.Id || target != u.Msg.EndUser.Id || trace == "" {
			t.Fatal("unexpected audit record", action, actor, target, trace)
		}
		seen++
	}
	if seen != 1 {
		t.Fatal("expected one end-user audit record", seen)
	}
}

func (f *fixture) metricsText(t *testing.T, name string) string {
	t.Helper()
	families, e := f.gather.Gather()
	if e != nil {
		t.Fatal(e)
	}
	var b strings.Builder
	for _, family := range families {
		if name != "" && family.GetName() != name {
			continue
		}
		b.WriteString(family.String())
	}
	return b.String()
}

func canonical(id string) bool { return len(id) == 36 && id[14] == '7' }
