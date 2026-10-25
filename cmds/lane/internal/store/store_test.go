// SPDX-License-Identifier: Apache-2.0
package store

import (
	"connectrpc.com/connect"
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	lane "github.com/delinoio/apps/protos/lane/v1"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
)

var ctx = context.Background()

func testStore(t *testing.T) *Store {
	t.Helper()
	s, e := Open(t.TempDir(), "")
	if e != nil {
		t.Fatal(e)
	}
	t.Cleanup(func() { s.Close() })
	return s
}
func meta(s *Store) *lane.MutationMeta {
	return &lane.MutationMeta{RequestId: NewID(), Origin: &lane.Origin{Kind: 2, DeviceId: s.DeviceID()}}
}
func create(t *testing.T, s *Store) (*lane.CreateTaskRequest, *lane.EntityVersion) {
	t.Helper()
	r := &lane.CreateTaskRequest{Meta: meta(s), Title: "Title", Repository: &lane.RepositoryTarget{Target: &lane.RepositoryTarget_Url{Url: "https://example.com/acme/repo.git"}}}
	v, e := s.CreateTask(ctx, connect.NewRequest(r))
	if e != nil {
		t.Fatal(e)
	}
	return r, v.Msg.Result.Version
}
func count(t *testing.T, s *Store, table string) int {
	t.Helper()
	var n int
	e := s.WithRead(ctx, func(c *sql.Conn) error { return c.QueryRowContext(ctx, "SELECT count(*) FROM "+table).Scan(&n) })
	if e != nil {
		t.Fatal(e)
	}
	return n
}
func reason(e error) lane.ErrorReason {
	var ce *connect.Error
	if errors.As(e, &ce) {
		for _, d := range ce.Details() {
			v, _ := d.Value()
			if x, ok := v.(*lane.ErrorDetail); ok {
				return x.Reason
			}
		}
	}
	return 0
}
func TestSchemaFreshness(t *testing.T) {
	for _, name := range []string{"schema.sql", "registry.sql"} {
		a, e := os.ReadFile(name)
		if e != nil {
			t.Fatal(e)
		}
		b, e := os.ReadFile(filepath.Join("../../../../apps/lane/native/store", name))
		if e != nil {
			t.Fatal(e)
		}
		if string(a) != string(b) {
			t.Fatalf("%s differs from canonical native schema", name)
		}
	}
}
func TestSharedNormalization(t *testing.T) {
	var f struct {
		Cases []struct {
			ID    string `json:"id"`
			Input struct {
				URL string `json:"url"`
			}
			Expected struct {
				NormalizedURL string `json:"normalizedUrl"`
				Reason        string
			}
		}
	}
	b, e := os.ReadFile("../../../../protos/lane/v1/fixtures/normalization.json")
	if e != nil {
		t.Fatal(e)
	}
	if e = json.Unmarshal(b, &f); e != nil {
		t.Fatal(e)
	}
	for _, c := range f.Cases {
		t.Run(c.ID, func(t *testing.T) {
			v, e := NormalizeURL(c.Input.URL)
			if c.Expected.Reason != "" {
				if reason(e) != 1 {
					t.Fatalf("want invalid input: %v", e)
				}
			} else if e != nil || v != c.Expected.NormalizedURL {
				t.Fatalf("got %q %v want %q", v, e, c.Expected.NormalizedURL)
			}
		})
	}
}
func TestSharedFingerprints(t *testing.T) {
	var f struct {
		Cases []struct {
			ID       string
			Input    struct{ NormalizedRequest json.RawMessage }
			Expected struct{ SHA256 string }
		}
	}
	b, e := os.ReadFile("../../../../protos/lane/v1/fixtures/fingerprints.json")
	if e != nil {
		t.Fatal(e)
	}
	if e = json.Unmarshal(b, &f); e != nil {
		t.Fatal(e)
	}
	for _, c := range f.Cases {
		t.Run(c.ID, func(t *testing.T) {
			r := new(lane.CreateTaskRequest)
			if e := protojson.Unmarshal(c.Input.NormalizedRequest, r); e != nil {
				t.Fatal(e)
			}
			fp, e := fingerprint(r)
			if e != nil || fp != c.Expected.SHA256 {
				t.Fatalf("got %s %v want %s", fp, e, c.Expected.SHA256)
			}
		})
	}
}
func TestTransactionsReplayDelete(t *testing.T) {
	s := testStore(t)
	req, v := create(t, s)
	if count(t, s, "entities") != 2 || count(t, s, "versions") != 2 || count(t, s, "outbox") != 2 {
		t.Fatal("creation not atomic")
	}
	seq, _ := s.Sequence(ctx)
	replay, e := s.CreateTask(ctx, connect.NewRequest(req))
	if e != nil || replay.Msg.Result.Version.VersionId != v.VersionId {
		t.Fatal("replay changed identity", e)
	}
	after, _ := s.Sequence(ctx)
	if after != seq {
		t.Fatal("replay emitted change")
	}
	changed := proto.Clone(req).(*lane.CreateTaskRequest)
	changed.Title = "Other"
	if _, e = s.CreateTask(ctx, connect.NewRequest(changed)); reason(e) != 6 {
		t.Fatal("request reuse accepted", e)
	}
	if _, e = s.DeleteTask(ctx, connect.NewRequest(&lane.DeleteTaskRequest{Meta: meta(s), Id: v.EntityId})); e != nil {
		t.Fatal(e)
	}
	replay, e = s.CreateTask(ctx, connect.NewRequest(req))
	if e != nil || replay.Msg.Result.Result != 3 || replay.Msg.Result.Version.Body != nil {
		t.Fatal("deleted replay leaked content", e)
	}
	hist, e := s.ListHistory(ctx, connect.NewRequest(&lane.ListHistoryRequest{Id: v.EntityId}))
	if e != nil || len(hist.Msg.Versions) != 0 {
		t.Fatal("deleted history retained", e)
	}
	var raw string
	e = s.WithRead(ctx, func(c *sql.Conn) error {
		return c.QueryRowContext(ctx, "SELECT group_concat(request_json) FROM outbox WHERE entity_id=?", v.EntityId).Scan(&raw)
	})
	if e != nil || strings.Contains(raw, "Title") {
		t.Fatal("outbox retained deleted body", e)
	}
}
func TestConcurrentWholeTaskConflicts(t *testing.T) {
	s := testStore(t)
	_, v := create(t, s)
	other, e := Open(s.Root(), s.SpaceID())
	if e != nil {
		t.Fatal(e)
	}
	defer other.Close()
	var wg sync.WaitGroup
	errs := make(chan error, 2)
	for i, st := range []*Store{s, other} {
		wg.Add(1)
		go func(i int, st *Store) {
			defer wg.Done()
			task := proto.Clone(v.GetTask()).(*lane.Task)
			if i == 0 {
				task.Title = "Changed title"
			} else {
				task.Prompt = proto.String("Changed prompt")
			}
			_, e := st.SaveTask(ctx, connect.NewRequest(&lane.SaveTaskRequest{Meta: meta(st), Id: v.EntityId, ExpectedVersionId: v.VersionId, Task: task}))
			errs <- e
		}(i, st)
	}
	wg.Wait()
	close(errs)
	applied, stale := 0, 0
	for e := range errs {
		if e == nil {
			applied++
		} else if reason(e) == 3 {
			stale++
		} else {
			t.Fatal(e)
		}
	}
	if applied != 1 || stale != 1 {
		t.Fatalf("applied=%d stale=%d", applied, stale)
	}
	got, e := s.GetTask(ctx, connect.NewRequest(&lane.GetTaskRequest{Id: v.EntityId}))
	if e != nil || len(got.Msg.Result.Alternatives) != 2 {
		t.Fatal("alternatives missing", e)
	}
	if _, e = s.SetProgress(ctx, connect.NewRequest(&lane.SetProgressRequest{Meta: meta(s), Id: v.EntityId, ExpectedVersionId: v.VersionId, Progress: 4})); reason(e) != 4 {
		t.Fatal("conflict edited", e)
	}
	hs := headIDs(got.Msg.Result.Alternatives)
	resolved := proto.Clone(got.Msg.Result.Alternatives[0]).(*lane.EntityVersion)
	r := &lane.ResolveConflictRequest{Meta: meta(s), Id: v.EntityId, ExpectedHeadIds: hs, Resolved: resolved}
	if _, e = s.ResolveConflict(ctx, connect.NewRequest(r)); e != nil {
		t.Fatal(e)
	}
	r.Meta = meta(s)
	if _, e = s.ResolveConflict(ctx, connect.NewRequest(r)); reason(e) != 5 {
		t.Fatal("stale resolution accepted", e)
	}
}
func TestRollbackAndLongLivedCompatibility(t *testing.T) {
	s := testStore(t)
	_, v := create(t, s)
	before, _ := s.Sequence(ctx)
	versions := count(t, s, "versions")
	e := s.WithWrite(ctx, func(c *sql.Conn) error {
		_, e := c.ExecContext(ctx, "CREATE TRIGGER fail_outbox BEFORE INSERT ON outbox BEGIN SELECT RAISE(ABORT,'injected write failure'); END")
		return e
	})
	if e != nil {
		t.Fatal(e)
	}
	_, e = s.SetProgress(ctx, connect.NewRequest(&lane.SetProgressRequest{Meta: meta(s), Id: v.EntityId, ExpectedVersionId: v.VersionId, Progress: 4}))
	if e == nil {
		t.Fatal("failure accepted")
	}
	after, _ := s.Sequence(ctx)
	if before != after || versions != count(t, s, "versions") {
		t.Fatal("partial write survived rollback")
	}
	if _, e = s.db.Exec("PRAGMA user_version=2"); e != nil {
		t.Fatal(e)
	}
	_, e = s.SetProgress(ctx, connect.NewRequest(&lane.SetProgressRequest{Meta: meta(s), Id: v.EntityId, ExpectedVersionId: v.VersionId, Progress: 4}))
	if reason(e) != 8 {
		t.Fatal("old writer accepted upgraded schema", e)
	}
}
func TestPromptProposalNotifications(t *testing.T) {
	s := testStore(t)
	req := &lane.CreateTaskRequest{Meta: meta(s), Title: "Proposal", Repository: &lane.RepositoryTarget{Target: &lane.RepositoryTarget_Url{Url: "https://example.com/r"}}, Proposal: 2, Progress: 3}
	req.Meta.Origin.Kind = 3
	r, e := s.CreateTask(ctx, connect.NewRequest(req))
	if e != nil {
		t.Fatal(e)
	}
	v := r.Msg.Result.Version
	if count(t, s, "notification_receipts") != 1 {
		t.Fatal("notification absent")
	}
	if _, e = s.CreateTask(ctx, connect.NewRequest(req)); e != nil {
		t.Fatal(e)
	}
	if count(t, s, "notification_receipts") != 1 {
		t.Fatal("duplicate notification")
	}
	var source string
	for _, p := range []string{"", "one"} {
		task := proto.Clone(v.GetTask()).(*lane.Task)
		task.Prompt = proto.String(p)
		r, e := s.SaveTask(ctx, connect.NewRequest(&lane.SaveTaskRequest{Meta: meta(s), Id: v.EntityId, ExpectedVersionId: v.VersionId, Task: task}))
		if e != nil {
			t.Fatal(e)
		}
		v = r.Msg.Result.Version
		if p == "" {
			source = v.VersionId
		}
	}
	restored, e := s.RestorePrompt(ctx, connect.NewRequest(&lane.RestorePromptRequest{Meta: meta(s), Id: v.EntityId, ExpectedVersionId: v.VersionId, SourceVersionId: source}))
	if e != nil {
		t.Fatal(e)
	}
	v = restored.Msg.Result.Version
	if v.GetTask().Prompt == nil || *v.GetTask().Prompt != "" {
		t.Fatal("prompt presence lost")
	}
	for _, p := range []lane.Proposal{4, 3} {
		r, e := s.SetProposal(ctx, connect.NewRequest(&lane.SetProposalRequest{Meta: meta(s), Id: v.EntityId, ExpectedVersionId: v.VersionId, Proposal: p}))
		if e != nil {
			t.Fatal(e)
		}
		v = r.Msg.Result.Version
		if v.GetTask().Progress != 3 {
			t.Fatal("decision changed progress")
		}
	}
	h, e := s.ListHistory(ctx, connect.NewRequest(&lane.ListHistoryRequest{Id: v.EntityId, PromptsOnly: true}))
	if e != nil || len(h.Msg.Versions) != 3 {
		t.Fatal("wrong prompt history", e)
	}
	var state int
	e = s.WithRead(ctx, func(c *sql.Conn) error {
		return c.QueryRowContext(ctx, "SELECT state FROM notification_receipts").Scan(&state)
	})
	if e != nil || state != 3 {
		t.Fatal("notification not canceled", e)
	}
}
func TestRegistryIsolationRemoval(t *testing.T) {
	s := testStore(t)
	_, v := create(t, s)
	a, e := s.SelectAccount(ctx, "https://issuer.example", "user-a", "tenant-a")
	if e != nil {
		t.Fatal(e)
	}
	defer a.Close()
	b, e := s.SelectAccount(ctx, "https://issuer.example", "user-b", "tenant-b")
	if e != nil {
		t.Fatal(e)
	}
	defer b.Close()
	if a.SpaceID() == b.SpaceID() || s.SpaceID() == a.SpaceID() {
		t.Fatal("spaces merged")
	}
	if _, e = a.GetTask(ctx, connect.NewRequest(&lane.GetTaskRequest{Id: v.EntityId})); reason(e) != 2 {
		t.Fatal("cross-account data", e)
	}
	other, e := Open(a.Root(), a.SpaceID())
	if e != nil {
		t.Fatal(e)
	}
	defer other.Close()
	if e = a.RemoveSpace(ctx); e != nil {
		t.Fatal(e)
	}
	if e = other.WithWrite(ctx, func(c *sql.Conn) error {
		_, e := c.ExecContext(ctx, "INSERT INTO changes(change_kind) VALUES(1)")
		return e
	}); reason(e) != 7 {
		t.Fatal("removed handle wrote", e)
	}
	if _, e = Open(a.Root(), a.SpaceID()); e == nil {
		t.Fatal("explicit absent space recreated")
	}
}
func TestSQLiteEngine(t *testing.T) {
	s := testStore(t)
	var version, source string
	e := s.WithRead(ctx, func(c *sql.Conn) error {
		return c.QueryRowContext(ctx, "SELECT sqlite_version(),sqlite_source_id()").Scan(&version, &source)
	})
	if e != nil {
		t.Fatal(e)
	}
	t.Logf("SQLite %s source %s", version, source)
	if version < "3.51.3" {
		t.Fatal("unpatched WAL engine")
	}
}
