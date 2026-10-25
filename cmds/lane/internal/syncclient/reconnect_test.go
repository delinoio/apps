// SPDX-License-Identifier: Apache-2.0
package syncclient

import (
	"context"
	"database/sql"
	"errors"
	"sort"
	"testing"
	"time"

	"connectrpc.com/connect"
	lanev1 "github.com/delinoio/apps/protos/lane/v1"
)

func staleEpochError() error {
	e := connect.NewError(connect.CodeFailedPrecondition, errors.New("stale epoch"))
	d, _ := connect.NewErrorDetail(&lanev1.ErrorDetail{Reason: lanev1.ErrorReason_ERROR_REASON_STALE_EPOCH})
	e.AddDetail(d)
	return e
}
func TestReconnectRequiresPriorMarkerAndNewBoundApproval(t *testing.T) {
	s := openTestStore(t)
	enqueue(t, s, "req", version("task", "v"))
	if _, e := s.db.Exec("INSERT INTO store_meta VALUES('account_epoch',?)", tid("old")); e != nil {
		t.Fatal(e)
	}
	w := worker(t, s, &service{epoch: tid("new"), pull: func(context.Context, *lanev1.PullChangesRequest) (*lanev1.PullChangesResponse, error) {
		return nil, staleEpochError()
	}})
	if e := ApprovedReconnect(context.Background(), s, w.client, "account", "tenant"); !errors.Is(e, ErrReconnectRequired) {
		t.Fatalf("first mismatch: %v", e)
	}
	if count(t, s, "SELECT count(*) FROM store_meta WHERE key='reconnect_required' AND value='1'") != 1 || count(t, s, "SELECT count(*) FROM outbox WHERE request_id='req'") != 1 {
		t.Fatal("first probe changed data or lost marker")
	}
	a := Auth{Credentials: &memoryCredentials{}, CredentialLockRoot: t.TempDir()}
	if _, e := a.BeginStore(context.Background(), s, "https://lane.example", "lane://auth/callback", "account", false); !errors.Is(e, ErrReconnectRequired) {
		t.Fatal("flagless approval accepted")
	}
	consent := Credential{ApprovalSequence: "1", ApprovalSpaceID: s.SpaceID(), ApprovalEpoch: tid("old"), AccountID: "account", TenantID: "tenant", ReconnectConfirmed: true, ApprovalExpiresAt: time.Now().Add(time.Minute)}
	wrong := consent
	wrong.ApprovalEpoch = tid("different")
	if e := ApprovedReconnect(context.Background(), s, w.client, "account", "tenant", wrong); !errors.Is(e, ErrReconnectRequired) {
		t.Fatal("mismatched approval accepted")
	}
	if e := ApprovedReconnect(context.Background(), s, w.client, "account", "tenant", consent); e != nil {
		t.Fatal(e)
	}
	if count(t, s, "SELECT count(*) FROM outbox") != 1 || count(t, s, "SELECT count(*) FROM outbox WHERE request_id='req'") != 0 || count(t, s, "SELECT count(*) FROM store_meta WHERE key='reconnect_required' AND value='1'") != 0 {
		t.Fatal("confirmed reset did not rebuild queue")
	}
}
func TestReconnectDeletionWinsOverConsent(t *testing.T) {
	s := openTestStore(t)
	enqueue(t, s, "req", version("task", "v"))
	if _, e := s.db.Exec("INSERT INTO store_meta VALUES('account_epoch',?)", tid("old")); e != nil {
		t.Fatal(e)
	}
	w := worker(t, s, &service{epoch: tid("new"), pull: func(context.Context, *lanev1.PullChangesRequest) (*lanev1.PullChangesResponse, error) {
		return &lanev1.PullChangesResponse{Changes: []*lanev1.Change{{Change: &lanev1.Change_Deletion{Deletion: &lanev1.DeletionRecord{Kind: lanev1.EntityKindOrRemoteKind_ENTITY_KIND_OR_REMOTE_KIND_ACCOUNT, Id: "account", DeletionVersionId: tid("new")}}}}}, nil
	}})
	if e := ApprovedReconnect(context.Background(), s, w.client, "account", "tenant"); !errors.Is(e, ErrAccountDeleted) {
		t.Fatalf("deletion: %v", e)
	}
	if count(t, s, "SELECT count(*) FROM entities") != 0 || count(t, s, "SELECT count(*) FROM outbox") != 0 {
		t.Fatal("deleted account retained content")
	}
}
func TestRejectedProvisionalResolutionIsHistoryOnly(t *testing.T) {
	s := openTestStore(t)
	base, b, c := version("task", "base"), version("task", "b", "base"), version("task", "c", "base")
	for _, v := range []*lanev1.EntityVersion{base, b, c} {
		if e := s.WithWrite(context.Background(), func(conn *sql.Conn) error {
			return ApplySyncResult(context.Background(), conn, &lanev1.EntityResult{Version: v})
		}); e != nil {
			t.Fatal(e)
		}
	}
	rejected, accepted := version("task", "e", "b", "c"), version("task", "d", "b", "c")
	sort.Strings(rejected.ParentVersionIds)
	sort.Strings(accepted.ParentVersionIds)
	enqueue(t, s, "resolution", rejected)
	w := worker(t, s, &service{push: func(context.Context, *lanev1.PushChangesRequest) (*lanev1.PushChangesResponse, error) {
		return &lanev1.PushChangesResponse{Results: []*lanev1.SyncResult{{RequestId: "resolution", Error: &lanev1.ErrorDetail{Reason: lanev1.ErrorReason_ERROR_REASON_STALE_RESOLUTION}}}}, nil
	}, pull: func(context.Context, *lanev1.PullChangesRequest) (*lanev1.PullChangesResponse, error) {
		return &lanev1.PullChangesResponse{Changes: []*lanev1.Change{{Change: &lanev1.Change_Entity{Entity: &lanev1.EntityResult{Version: accepted}}}}}, nil
	}})
	if e := w.Run(context.Background(), true); e != nil {
		t.Fatal(e)
	}
	var head string
	if e := s.db.QueryRow("SELECT version_id FROM heads").Scan(&head); e != nil || head != accepted.VersionId {
		t.Fatalf("head=%s %v", head, e)
	}
	var exists int
	if e := s.db.QueryRow("SELECT count(*) FROM versions WHERE version_id=?", rejected.VersionId).Scan(&exists); e != nil || exists != 1 {
		t.Fatal("rejected history lost")
	}
}

func TestAccountDeletionCleanupResumesAfterCrash(t *testing.T) {
	s := openTestStore(t)
	if _, e := s.db.Exec("INSERT INTO store_meta VALUES('account_deleted','1')"); e != nil {
		t.Fatal(e)
	}
	w := worker(t, s, &service{})
	called := false
	w.options.OnAccountDeleted = func(ctx context.Context) error {
		called = true
		return s.WithWrite(ctx, func(c *sql.Conn) error { return putMeta(ctx, c, "cleanup", "done") })
	}
	if e := w.Run(context.Background(), true); !errors.Is(e, ErrAccountDeleted) {
		t.Fatalf("deletion error: %v", e)
	}
	if !called {
		t.Fatal("durable cleanup marker ignored")
	}
}
