// SPDX-License-Identifier: Apache-2.0
package syncclient

import (
	"context"
	"crypto/sha256"
	"database/sql"
	"errors"
	"fmt"
	"github.com/google/uuid"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"connectrpc.com/connect"
	lanev1 "github.com/delinoio/apps/protos/lane/v1"
	"github.com/delinoio/apps/protos/lane/v1/lanev1connect"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/types/known/timestamppb"
	_ "modernc.org/sqlite"
)

const schema = `
CREATE TABLE store_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
CREATE TABLE entities(id TEXT PRIMARY KEY,kind INTEGER NOT NULL,repository_id TEXT,normalized_url TEXT,deleted INTEGER NOT NULL DEFAULT 0);
CREATE UNIQUE INDEX repository_url ON entities(normalized_url) WHERE kind=1 AND deleted=0 AND normalized_url IS NOT NULL;
CREATE TABLE versions(version_id TEXT PRIMARY KEY,entity_id TEXT NOT NULL REFERENCES entities(id),envelope_json TEXT NOT NULL,created_at_us INTEGER NOT NULL,prompt_version INTEGER NOT NULL DEFAULT 0);
CREATE TABLE heads(entity_id TEXT NOT NULL REFERENCES entities(id),version_id TEXT NOT NULL REFERENCES versions(version_id),PRIMARY KEY(entity_id,version_id));
CREATE TABLE receipts(method TEXT,request_id TEXT,fingerprint TEXT,entity_id TEXT,result_json TEXT,PRIMARY KEY(method,request_id));
CREATE TABLE outbox(request_id TEXT PRIMARY KEY,entity_id TEXT,request_json TEXT,state INTEGER DEFAULT 1,error_reason TEXT,created_at_us INTEGER);
CREATE TABLE changes(sequence INTEGER PRIMARY KEY AUTOINCREMENT,entity_id TEXT,change_kind INTEGER);
CREATE TABLE remote_objects(kind INTEGER,id TEXT,body_json TEXT,deleted INTEGER DEFAULT 0,PRIMARY KEY(kind,id));
CREATE TABLE repository_mappings(local_id TEXT PRIMARY KEY,canonical_id TEXT);
CREATE TABLE notification_receipts(proposal_id TEXT,device_id TEXT,state INTEGER,PRIMARY KEY(proposal_id,device_id));`

type testStore struct {
	db   *sql.DB
	root string
}

func (s *testStore) SpaceID() string  { return "space" }
func (s *testStore) DeviceID() string { return "device" }
func (s *testStore) Root() string     { return s.root }
func (s *testStore) transaction(ctx context.Context, begin string, f func(*sql.Conn) error) error {
	c, e := s.db.Conn(ctx)
	if e != nil {
		return e
	}
	defer c.Close()
	if _, e = c.ExecContext(ctx, begin); e != nil {
		return e
	}
	defer c.ExecContext(context.Background(), "ROLLBACK")
	if e = f(c); e != nil {
		return e
	}
	_, e = c.ExecContext(ctx, "COMMIT")
	return e
}
func (s *testStore) WithRead(ctx context.Context, f func(*sql.Conn) error) error {
	return s.transaction(ctx, "BEGIN", f)
}
func (s *testStore) WithWrite(ctx context.Context, f func(*sql.Conn) error) error {
	return s.transaction(ctx, "BEGIN IMMEDIATE", f)
}
func openTestStore(t *testing.T) *testStore {
	t.Helper()
	root := t.TempDir()
	db, e := sql.Open("sqlite", filepath.Join(root, "lane.sqlite")+"?_pragma=journal_mode(WAL)&_pragma=foreign_keys(1)&_pragma=busy_timeout(5000)&_pragma=synchronous(FULL)")
	if e != nil {
		t.Fatal(e)
	}
	if _, e = db.Exec(schema); e != nil {
		t.Fatal(e)
	}
	t.Cleanup(func() { db.Close() })
	return &testStore{db, root}
}
func tid(label string) string {
	sum := sha256.Sum256([]byte(label))
	var id uuid.UUID
	copy(id[:], sum[:16])
	id[6] = (id[6] & 15) | 0x70
	id[8] = (id[8] & 63) | 0x80
	return id.String()
}
func version(id, vid string, parents ...string) *lanev1.EntityVersion {
	for i, p := range parents {
		parents[i] = tid(p)
	}
	return &lanev1.EntityVersion{FormatVersion: 1, EntityId: tid(id), VersionId: tid(vid), Kind: lanev1.EntityKind_ENTITY_KIND_TASK, ParentVersionIds: parents, CreatedAt: timestamppb.New(time.Now().Truncate(time.Microsecond)), Origin: &lanev1.Origin{Kind: lanev1.OriginKind_ORIGIN_KIND_CLI, DeviceId: tid("device")}, Body: &lanev1.EntityVersion_Task{Task: &lanev1.Task{RepositoryId: tid("repo"), Title: "private title", Progress: lanev1.Progress_PROGRESS_TODO, Proposal: lanev1.Proposal_PROPOSAL_REGULAR}}}
}
func enqueue(t *testing.T, s *testStore, id string, v *lanev1.EntityVersion) {
	t.Helper()
	m := &lanev1.SyncMutation{Meta: &lanev1.MutationMeta{RequestId: id}, Version: v, Kind: lanev1.MutationKind_MUTATION_KIND_CREATE}
	raw, e := protojson.Marshal(m)
	if e != nil {
		t.Fatal(e)
	}
	e = s.WithWrite(context.Background(), func(c *sql.Conn) error {
		if e := ApplySyncResult(context.Background(), c, &lanev1.EntityResult{Result: lanev1.ResultKind_RESULT_KIND_APPLIED, Version: v}); e != nil {
			return e
		}
		_, e := c.ExecContext(context.Background(), "INSERT INTO outbox(request_id,entity_id,request_json,created_at_us) VALUES(?,?,?,?)", id, v.EntityId, string(raw), time.Now().UnixMicro())
		return e
	})
	if e != nil {
		t.Fatal(e)
	}
}

type service struct {
	lanev1connect.UnimplementedSyncServiceHandler
	push           func(context.Context, *lanev1.PushChangesRequest) (*lanev1.PushChangesResponse, error)
	pull           func(context.Context, *lanev1.PullChangesRequest) (*lanev1.PullChangesResponse, error)
	epoch, account string
}

func (s *service) GetSyncInfo(context.Context, *connect.Request[lanev1.GetSyncInfoRequest]) (*connect.Response[lanev1.GetSyncInfoResponse], error) {
	epoch := s.epoch
	if epoch == "" {
		epoch = "epoch"
	}
	account := s.account
	if account == "" {
		account = "account"
	}
	return connect.NewResponse(&lanev1.GetSyncInfoResponse{FormatVersion: 1, AccountId: account, TenantId: "tenant", Epoch: epoch}), nil
}
func (s *service) PushChanges(ctx context.Context, r *connect.Request[lanev1.PushChangesRequest]) (*connect.Response[lanev1.PushChangesResponse], error) {
	m, e := s.push(ctx, r.Msg)
	if e != nil {
		return nil, e
	}
	return connect.NewResponse(m), nil
}
func (s *service) PullChanges(ctx context.Context, r *connect.Request[lanev1.PullChangesRequest]) (*connect.Response[lanev1.PullChangesResponse], error) {
	if s.pull != nil {
		m, e := s.pull(ctx, r.Msg)
		if e != nil {
			return nil, e
		}
		return connect.NewResponse(m), nil
	}
	return connect.NewResponse(&lanev1.PullChangesResponse{NextCursor: "cursor"}), nil
}
func worker(t *testing.T, s *testStore, handler *service) *Worker {
	t.Helper()
	mux := http.NewServeMux()
	path, h := lanev1connect.NewSyncServiceHandler(handler)
	mux.Handle(path, h)
	server := httptest.NewServer(mux)
	t.Cleanup(server.Close)
	w, e := NewWorker(s, lanev1connect.NewSyncServiceClient(server.Client(), server.URL), Options{AccountID: "account", TenantID: "tenant"})
	if e != nil {
		t.Fatal(e)
	}
	return w
}
func count(t *testing.T, s *testStore, query string) int {
	t.Helper()
	var n int
	if e := s.db.QueryRow(query).Scan(&n); e != nil {
		t.Fatal(e)
	}
	return n
}
func TestResponseLossAndAuthFailureKeepOutbox(t *testing.T) {
	for _, code := range []connect.Code{connect.CodeUnavailable, connect.CodeUnauthenticated} {
		t.Run(code.String(), func(t *testing.T) {
			s := openTestStore(t)
			v := version("task", "version")
			enqueue(t, s, "request", v)
			attempt := 0
			w := worker(t, s, &service{push: func(ctx context.Context, r *lanev1.PushChangesRequest) (*lanev1.PushChangesResponse, error) {
				attempt++
				if r.Mutations[0].Meta.RequestId != "request" {
					t.Error("request identity changed")
				}
				if attempt == 1 {
					return nil, connect.NewError(code, errors.New("failure"))
				}
				return &lanev1.PushChangesResponse{Results: []*lanev1.SyncResult{{RequestId: "request", Result: &lanev1.EntityResult{Result: lanev1.ResultKind_RESULT_KIND_REPLAYED, Version: v}}}}, nil
			}})
			if e := w.Run(context.Background(), true); e == nil {
				t.Fatal("expected failure")
			}
			if count(t, s, "SELECT count(*) FROM outbox") != 1 {
				t.Fatal("lost durable work")
			}
			if e := w.Run(context.Background(), true); e != nil {
				t.Fatal(e)
			}
			if count(t, s, "SELECT count(*) FROM outbox") != 0 {
				t.Fatal("acknowledgment not persisted")
			}
		})
	}
}
func TestPerItemFailureAndNetworkHasNoWriteLock(t *testing.T) {
	s := openTestStore(t)
	a, b := version("a", "va"), version("b", "vb")
	enqueue(t, s, "a", a)
	enqueue(t, s, "b", b)
	w := worker(t, s, &service{push: func(ctx context.Context, r *lanev1.PushChangesRequest) (*lanev1.PushChangesResponse, error) {
		if e := s.WithWrite(ctx, func(c *sql.Conn) error { return putMeta(ctx, c, "concurrent", "yes") }); e != nil {
			t.Error(e)
		}
		return &lanev1.PushChangesResponse{Results: []*lanev1.SyncResult{{RequestId: "a", Result: &lanev1.EntityResult{Result: lanev1.ResultKind_RESULT_KIND_APPLIED, Version: a}}, {RequestId: "b", Error: &lanev1.ErrorDetail{Reason: lanev1.ErrorReason_ERROR_REASON_ITEM_TOO_LARGE}}}}, nil
	}})
	if e := w.Run(context.Background(), true); e != nil {
		t.Fatal(e)
	}
	if count(t, s, "SELECT count(*) FROM outbox WHERE request_id='b' AND state=2 AND error_reason='ERROR_REASON_ITEM_TOO_LARGE'") != 1 {
		t.Fatal("missing item failure")
	}
}
func TestLeaseAndStaleResponse(t *testing.T) {
	s := openTestStore(t)
	v := version("task", "v")
	enqueue(t, s, "req", v)
	entered, release := make(chan struct{}), make(chan struct{})
	handler := &service{push: func(context.Context, *lanev1.PushChangesRequest) (*lanev1.PushChangesResponse, error) {
		close(entered)
		<-release
		return &lanev1.PushChangesResponse{Results: []*lanev1.SyncResult{{RequestId: "req", Result: &lanev1.EntityResult{Result: lanev1.ResultKind_RESULT_KIND_APPLIED, Version: v}}}}, nil
	}}
	first, second := worker(t, s, handler), worker(t, s, handler)
	var wg sync.WaitGroup
	wg.Add(1)
	var firstErr error
	go func() { defer wg.Done(); firstErr = first.Run(context.Background(), true) }()
	<-entered
	if e := second.Run(context.Background(), true); !errors.Is(e, ErrLeaseHeld) {
		t.Fatalf("second owner: %v", e)
	}
	if _, e := s.db.Exec("UPDATE store_meta SET value='0' WHERE key='sync_owner_expires_at_us'"); e != nil {
		t.Fatal(e)
	}
	if e := second.lease(context.Background()); e != nil {
		t.Fatal(e)
	}
	close(release)
	wg.Wait()
	if !errors.Is(firstErr, ErrLeaseLost) {
		t.Fatalf("stale response: %v", firstErr)
	}
	if count(t, s, "SELECT count(*) FROM outbox") != 1 {
		t.Fatal("stale owner acknowledged")
	}
}
func TestEpochAndAccountIsolation(t *testing.T) {
	for _, h := range []*service{{epoch: "new"}, {account: "different"}} {
		s := openTestStore(t)
		if _, e := s.db.Exec("INSERT INTO store_meta VALUES('account_epoch','old')"); e != nil {
			t.Fatal(e)
		}
		w := worker(t, s, h)
		if e := w.Run(context.Background(), true); !errors.Is(e, ErrStaleEpoch) && !errors.Is(e, ErrAccountMismatch) {
			t.Fatalf("unexpected %v", e)
		}
	}
}
func TestDeletionAndDelayedEdit(t *testing.T) {
	s := openTestStore(t)
	v := version("task", "v")
	enqueue(t, s, "req", v)
	if _, e := s.db.Exec("INSERT INTO receipts VALUES('CreateTask','req','digest',?,'private body')", tid("task")); e != nil {
		t.Fatal(e)
	}
	tomb := &lanev1.EntityVersion{FormatVersion: 1, EntityId: tid("task"), VersionId: tid("deleted"), Kind: lanev1.EntityKind_ENTITY_KIND_TASK, Deleted: true}
	for _, snapshot := range []*lanev1.EntityVersion{tomb, v} {
		if e := s.WithWrite(context.Background(), func(c *sql.Conn) error {
			return ApplySyncResult(context.Background(), c, &lanev1.EntityResult{Version: snapshot})
		}); e != nil {
			t.Fatal(e)
		}
	}
	if count(t, s, "SELECT count(*) FROM outbox") != 0 || count(t, s, "SELECT count(*) FROM versions WHERE envelope_json LIKE '%private%'") != 0 || count(t, s, "SELECT count(*) FROM receipts WHERE result_json LIKE '%private%'") != 0 {
		t.Fatal("deleted content retained")
	}
	if count(t, s, "SELECT count(*) FROM entities WHERE deleted=1") != 1 {
		t.Fatal("resurrected")
	}
}
func TestConcurrentLocalDescendantSurvivesAck(t *testing.T) {
	s := openTestStore(t)
	base := version("task", "base")
	enqueue(t, s, "base", base)
	next := version("task", "next", "base")
	enqueue(t, s, "next", next)
	if e := s.WithWrite(context.Background(), func(c *sql.Conn) error {
		return ApplySyncResult(context.Background(), c, &lanev1.EntityResult{Version: base})
	}); e != nil {
		t.Fatal(e)
	}
	if count(t, s, fmt.Sprintf("SELECT count(*) FROM heads WHERE version_id='%s'", tid("next"))) != 1 || count(t, s, "SELECT count(*) FROM heads") != 1 {
		t.Fatal("ack overwrote local descendant")
	}
	other := version("task", "other", "base")
	if e := s.WithWrite(context.Background(), func(c *sql.Conn) error {
		return ApplySyncResult(context.Background(), c, &lanev1.EntityResult{Version: other})
	}); e != nil {
		t.Fatal(e)
	}
	if count(t, s, "SELECT count(*) FROM heads") != 2 {
		t.Fatal("lost whole-task conflict")
	}
}
func TestPullFailureRollsBackCursorAndContent(t *testing.T) {
	s := openTestStore(t)
	w := worker(t, s, &service{pull: func(context.Context, *lanev1.PullChangesRequest) (*lanev1.PullChangesResponse, error) {
		return &lanev1.PullChangesResponse{NextCursor: "bad", Changes: []*lanev1.Change{{Change: &lanev1.Change_Entity{Entity: &lanev1.EntityResult{Version: version("a", "v")}}}, {}}}, nil
	}})
	if e := w.Run(context.Background(), true); e == nil {
		t.Fatal("accepted malformed page")
	}
	if count(t, s, "SELECT count(*) FROM entities") != 0 || count(t, s, "SELECT count(*) FROM store_meta WHERE key='pull_cursor'") != 0 {
		t.Fatal("partial page committed")
	}
}
