// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package integration

import (
	"bytes"
	"context"
	"crypto/sha256"
	"strings"
	"testing"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/devbase/v1"
	"github.com/delinoio/apps/servers/devbase-api/internal/api"
	"github.com/delinoio/apps/servers/devbase-api/internal/config"
	"google.golang.org/protobuf/types/known/timestamppb"
)

func TestKeyIssuanceStoresOnlyAHash(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, "Keys")
	raw, k := f.key(login(owner), p.Id, pb.ApiKeyPermission_API_KEY_PERMISSION_READ_ONLY)
	id, secret, ok := api.ParseKey(raw)
	if !ok || id != k.Id {
		t.Fatal("issued key is malformed")
	}
	var hash []byte
	if e := f.db.Pool.QueryRow(ctx, "SELECT secret_hash FROM api_keys WHERE id=$1", k.Id).Scan(&hash); e != nil {
		t.Fatal(e)
	}
	sum := sha256.Sum256([]byte(secret))
	if !bytes.Equal(hash, sum[:]) {
		t.Fatal("stored value is not the secret hash")
	}
	// The raw key and secret appear in no table.
	for _, table := range []string{"api_keys", "replays", "audit_events", "projects"} {
		var dump string
		if e := f.db.Pool.QueryRow(ctx, "SELECT coalesce(string_agg(t::text,' '),'') FROM "+table+" t").Scan(&dump); e != nil {
			t.Fatal(e)
		}
		if strings.Contains(dump, secret) || strings.Contains(dump, raw) {
			t.Fatalf("secret leaked into %s", table)
		}
	}
	got, e := f.client.GetApiKey(ctx, as(login(owner), &pb.GetApiKeyRequest{ProjectId: p.Id, ApiKeyId: k.Id}))
	if e != nil || got.Msg.ApiKey.Permission != pb.ApiKeyPermission_API_KEY_PERMISSION_READ_ONLY || got.Msg.ApiKey.ExpireTime != nil {
		t.Fatal("get key", e)
	}
	if _, e = f.client.GetApiKey(ctx, as(login(owner), &pb.GetApiKeyRequest{ProjectId: p.Id, ApiKeyId: newID()})); code(e) != connect.CodeNotFound {
		t.Fatal("unknown key", e)
	}
	// A key ID from another project is not found through this project.
	q := f.project(login(owner), tenant, "Other keys")
	if _, e = f.client.GetApiKey(ctx, as(login(owner), &pb.GetApiKeyRequest{ProjectId: q.Id, ApiKeyId: k.Id})); code(e) != connect.CodeNotFound {
		t.Fatal("key readable through another project", e)
	}
}

func TestKeyIssuanceReplayNeverReturnsTheSecret(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, "Replay")
	req := &pb.IssueApiKeyRequest{IdempotencyKey: idem(), ProjectId: p.Id, Permission: pb.ApiKeyPermission_API_KEY_PERMISSION_CRUD}
	first, e := f.client.IssueApiKey(ctx, as(login(owner), req))
	if e != nil || first.Msg.Secret == "" {
		t.Fatal(e)
	}
	again, e := f.client.IssueApiKey(ctx, as(login(owner), req))
	if e != nil || again.Msg.Secret != "" || again.Msg.ApiKey.Id != first.Msg.ApiKey.Id || again.Msg.ApiKey.Permission != first.Msg.ApiKey.Permission {
		t.Fatal("replay must return only the ID and permission", e)
	}
	if n := f.count("SELECT count(*) FROM api_keys"); n != 1 {
		t.Fatal("replay issued another key", n)
	}
}

func TestKeyExpiryRevocationAndListing(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, "Lifecycle")
	if _, e := f.client.IssueApiKey(ctx, as(login(owner), &pb.IssueApiKeyRequest{IdempotencyKey: idem(), ProjectId: p.Id, Permission: pb.ApiKeyPermission_API_KEY_PERMISSION_CRUD, ExpireTime: timestamppb.New(time.Now().Add(-time.Hour))})); code(e) != connect.CodeInvalidArgument {
		t.Fatal("past expiry accepted", e)
	}
	if _, e := f.client.IssueApiKey(ctx, as(login(owner), &pb.IssueApiKeyRequest{IdempotencyKey: idem(), ProjectId: p.Id})); code(e) != connect.CodeInvalidArgument {
		t.Fatal("unspecified permission accepted", e)
	}
	expiry := time.Now().Add(time.Hour)
	r, e := f.client.IssueApiKey(ctx, as(login(owner), &pb.IssueApiKeyRequest{IdempotencyKey: idem(), ProjectId: p.Id, Permission: pb.ApiKeyPermission_API_KEY_PERMISSION_CRUD, ExpireTime: timestamppb.New(expiry)}))
	if e != nil || r.Msg.ApiKey.ExpireTime == nil {
		t.Fatal("expiry not stored", e)
	}
	_, plain := f.key(login(owner), p.Id, pb.ApiKeyPermission_API_KEY_PERMISSION_READ_ONLY)
	rev := &pb.RevokeApiKeyRequest{IdempotencyKey: idem(), ProjectId: p.Id, ApiKeyId: plain.Id}
	first, e := f.client.RevokeApiKey(ctx, as(login(owner), rev))
	if e != nil || first.Msg.ApiKey.RevokeTime == nil {
		t.Fatal("revoke", e)
	}
	// A second revocation with a new idempotency key leaves the first revoke time and writes no audit event.
	second, e := f.client.RevokeApiKey(ctx, as(login(owner), &pb.RevokeApiKeyRequest{IdempotencyKey: idem(), ProjectId: p.Id, ApiKeyId: plain.Id}))
	if e != nil || !second.Msg.ApiKey.RevokeTime.AsTime().Equal(first.Msg.ApiKey.RevokeTime.AsTime()) || f.count("SELECT count(*) FROM audit_events WHERE action='api_key.revoke'") != 1 {
		t.Fatal("revocation is not idempotent in effect", e)
	}
	if _, e = f.client.RevokeApiKey(ctx, as(login(owner), &pb.RevokeApiKeyRequest{IdempotencyKey: idem(), ProjectId: p.Id, ApiKeyId: newID()})); code(e) != connect.CodeNotFound {
		t.Fatal("unknown key revoked", e)
	}
	list, e := f.client.ListApiKeys(ctx, as(login(owner), &pb.ListApiKeysRequest{ProjectId: p.Id, PageSize: 1}))
	if e != nil || len(list.Msg.ApiKeys) != 1 || list.Msg.NextPageToken == "" {
		t.Fatal("key list", e)
	}
	next, e := f.client.ListApiKeys(ctx, as(login(owner), &pb.ListApiKeysRequest{ProjectId: p.Id, PageSize: 5, PageToken: list.Msg.NextPageToken}))
	if e != nil || len(next.Msg.ApiKeys) != 1 || next.Msg.NextPageToken != "" {
		t.Fatal("second key page", e)
	}
}

func TestActiveKeyLimit(t *testing.T) {
	f := setup(t, func(l *config.Limits) { l.MaxActiveKeysPerProject = 3 })
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, "Limit")
	_, first := f.key(login(owner), p.Id, pb.ApiKeyPermission_API_KEY_PERMISSION_CRUD)
	f.key(login(owner), p.Id, pb.ApiKeyPermission_API_KEY_PERMISSION_CRUD)
	f.key(login(owner), p.Id, pb.ApiKeyPermission_API_KEY_PERMISSION_CRUD)
	_, e := f.client.IssueApiKey(ctx, as(login(owner), &pb.IssueApiKeyRequest{IdempotencyKey: idem(), ProjectId: p.Id, Permission: pb.ApiKeyPermission_API_KEY_PERMISSION_CRUD}))
	if code(e) != connect.CodeResourceExhausted {
		t.Fatal("limit not enforced", e)
	}
	// Rotation: revoking an old key frees capacity.
	if _, e = f.client.RevokeApiKey(ctx, as(login(owner), &pb.RevokeApiKeyRequest{IdempotencyKey: idem(), ProjectId: p.Id, ApiKeyId: first.Id})); e != nil {
		t.Fatal(e)
	}
	f.key(login(owner), p.Id, pb.ApiKeyPermission_API_KEY_PERMISSION_CRUD)
}

func TestKeyIssuanceRaceRespectsLimit(t *testing.T) {
	f := setup(t, func(l *config.Limits) { l.MaxActiveKeysPerProject = 4 })
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, "Race")
	results := make(chan error, 16)
	for i := 0; i < 16; i++ {
		go func() {
			_, e := f.client.IssueApiKey(ctx, as(login(owner), &pb.IssueApiKeyRequest{IdempotencyKey: idem(), ProjectId: p.Id, Permission: pb.ApiKeyPermission_API_KEY_PERMISSION_CRUD}))
			results <- e
		}()
	}
	ok := 0
	for i := 0; i < 16; i++ {
		if e := <-results; e == nil {
			ok++
		} else if code(e) != connect.CodeResourceExhausted {
			t.Fatal(e)
		}
	}
	if ok != 4 || f.count("SELECT count(*) FROM api_keys") != 4 {
		t.Fatalf("concurrent issuance exceeded the limit: %d", ok)
	}
}

func TestKeyIssuanceReplayAfterRequestedExpiry(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, "Late replay")
	req := &pb.IssueApiKeyRequest{IdempotencyKey: idem(), ProjectId: p.Id, Permission: pb.ApiKeyPermission_API_KEY_PERMISSION_CRUD, ExpireTime: timestamppb.New(f.clock.now().Add(time.Hour))}
	first, e := f.client.IssueApiKey(ctx, as(login(owner), req))
	if e != nil {
		t.Fatal(e)
	}
	// The retry arrives after the requested expiry but inside the replay window.
	f.clock.advance(2 * time.Hour)
	again, e := f.client.IssueApiKey(ctx, as(login(owner), req))
	if e != nil || again.Msg.ApiKey.Id != first.Msg.ApiKey.Id || again.Msg.Secret != "" {
		t.Fatal("a late retry must replay the prior key ID and permission", e)
	}
	// A genuinely new request with a past expiry is still rejected.
	past := &pb.IssueApiKeyRequest{IdempotencyKey: idem(), ProjectId: p.Id, Permission: pb.ApiKeyPermission_API_KEY_PERMISSION_CRUD, ExpireTime: timestamppb.New(f.clock.now().Add(-time.Minute))}
	if _, e = f.client.IssueApiKey(ctx, as(login(owner), past)); code(e) != connect.CodeInvalidArgument {
		t.Fatal("past expiry accepted", e)
	}
	if f.count("SELECT count(*) FROM api_keys") != 1 {
		t.Fatal("unexpected key count")
	}
}

// A project deleted after authorization but before the query leaves no keys; the empty page
// must not look like a live project without keys.
func TestListApiKeysReportsAProjectDeletedAfterAuthorization(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	tenant, owner, _ := f.tenant()
	p := f.project(login(owner), tenant, "Racing")
	f.key(login(owner), p.Id, pb.ApiKeyPermission_API_KEY_PERMISSION_CRUD)
	if list, e := f.client.ListApiKeys(ctx, as(login(owner), &pb.ListApiKeysRequest{ProjectId: p.Id})); e != nil || len(list.Msg.ApiKeys) != 1 {
		t.Fatal("the live project did not list its key", e)
	}
	empty := f.project(login(owner), tenant, "Empty")
	if list, e := f.client.ListApiKeys(ctx, as(login(owner), &pb.ListApiKeysRequest{ProjectId: empty.Id})); e != nil || len(list.Msg.ApiKeys) != 0 {
		t.Fatal("a live project without keys must list successfully", e)
	}
	f.fake.mu.Lock()
	f.fake.beforeStates = func() {
		if _, e := f.db.Pool.Exec(ctx, "DELETE FROM projects WHERE id=$1", p.Id); e != nil {
			t.Error(e)
		}
	}
	f.fake.mu.Unlock()
	if _, e := f.client.ListApiKeys(ctx, as(login(owner), &pb.ListApiKeysRequest{ProjectId: p.Id})); code(e) != connect.CodeNotFound {
		t.Fatal("a project deleted after authorization listed as empty", e)
	}
}
