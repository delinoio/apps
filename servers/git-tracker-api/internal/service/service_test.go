// SPDX-License-Identifier: FSL-1.1-ALv2

package service

import (
	"bytes"
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"sync"
	"testing"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/gittracker/v1"
	"github.com/delinoio/apps/protos/gittracker/v1/gittrackerv1connect"
	"github.com/delinoio/apps/servers/git-tracker-api/internal/measurement"
	"github.com/delinoio/apps/servers/git-tracker-api/migrations"
	"github.com/jackc/pgx/v5/pgxpool"
)

var fixtureRepositoryID string

type testAuth struct{}

func (testAuth) Allow(_ context.Context, header, space string, role Role) error {
	if header != "Bearer manager" || space != "019b0000-0000-7000-8000-000000000001" {
		return fault(connect.CodePermissionDenied, "denied")
	}
	return nil
}
func (testAuth) VerifyActions(context.Context, string, *pb.Repository, *pb.Context) error {
	return fault(connect.CodePermissionDenied, "denied")
}

type testGitHub struct{}

func (testGitHub) Connect(context.Context, string, string) (*pb.Repository, error) {
	return &pb.Repository{GithubRepositoryId: "42", FullName: "owner/repo", InstallationId: "1", Connected: true}, nil
}
func (testGitHub) Resolve(context.Context, *pb.Repository, *pb.Context) ([]string, string, error) {
	return nil, "", nil
}
func (testGitHub) Comment(context.Context, *pb.Repository, *measurement.State, *measurement.Record) error {
	return nil
}
func database(t *testing.T) *Server {
	t.Helper()
	dsn := os.Getenv("TEST_DATABASE_URL")
	if dsn == "" {
		t.Skip("TEST_DATABASE_URL required for PostgreSQL integration")
	}
	ctx := context.Background()
	db, e := pgxpool.New(ctx, dsn)
	if e != nil {
		t.Fatal(e)
	}
	t.Cleanup(db.Close)
	_, e = db.Exec(ctx, "DROP TABLE IF EXISTS git_tracker_links,git_tracker_tokens,git_tracker_audit,git_tracker_repositories CASCADE")
	if e != nil {
		t.Fatal(e)
	}
	if _, e = db.Exec(ctx, migrations.Initial); e != nil {
		t.Fatal(e)
	}
	s := &Server{DB: db, Auth: testAuth{}, GitHub: testGitHub{}, Log: slog.New(slog.NewTextHandler(io.Discard, nil)), AuditRetention: time.Hour}
	q := connect.NewRequest(&pb.ConnectRepositoryRequest{SpaceId: "019b0000-0000-7000-8000-000000000001", FullName: "owner/repo"})
	q.Header().Set("Authorization", "Bearer manager")
	connected, err := s.ConnectRepository(ctx, q)
	if err != nil {
		t.Fatal(err)
	}
	fixtureRepositoryID = connected.Msg.Repository.Id
	return s
}
func uploadRequest(row string) *connect.Request[pb.UploadRequest] {
	q := connect.NewRequest(&pb.UploadRequest{Context: &pb.Context{RepositoryId: fixtureRepositoryID, ExecutionId: "run", Commit: strings.Repeat("a", 40), Branch: "main"}, Dataset: &pb.Dataset{Id: "sizes", Name: "Sizes", Columns: []*pb.Column{{Id: "bytes", Name: "Bytes", Unit: "B", Direction: pb.Direction_DIRECTION_LOWER}}}, Rows: []*pb.Row{{Id: row, Values: map[string]string{"bytes": "9007199254740993.00000000000001"}}}})
	q.Header().Set("Authorization", "Bearer manager")
	return q
}
func TestPostgresConcurrentAtomicAndAuthorization(t *testing.T) {
	s := database(t)
	ctx := context.Background()
	var wg sync.WaitGroup
	errs := make(chan error, 2)
	for _, row := range []string{"a", "b"} {
		wg.Add(1)
		go func(row string) { defer wg.Done(); _, e := s.Upload(ctx, uploadRequest(row)); errs <- e }(row)
	}
	wg.Wait()
	close(errs)
	for e := range errs {
		if e != nil {
			t.Fatal(e)
		}
	}
	get := connect.NewRequest(&pb.GetExecutionRequest{RepositoryId: fixtureRepositoryID, ExecutionId: "run"})
	if _, e := s.GetExecution(ctx, get); connect.CodeOf(e) != connect.CodeUnauthenticated {
		t.Fatal("anonymous private read", e)
	}
	get.Header().Set("Authorization", "Bearer manager")
	res, e := s.GetExecution(ctx, get)
	if e != nil {
		t.Fatal(e)
	}
	if len(res.Msg.Execution.Measurements[0].Rows) != 2 {
		t.Fatal("lost parallel row")
	}
	bad := uploadRequest("new")
	bad.Msg.Rows = append(bad.Msg.Rows, &pb.Row{Id: "a", Values: map[string]string{"bytes": "1"}})
	if _, e = s.Upload(ctx, bad); e == nil {
		t.Fatal("conflict accepted")
	}
	res, e = s.GetExecution(ctx, get)
	if e != nil || len(res.Msg.Execution.Measurements[0].Rows) != 2 {
		t.Fatal("partial commit")
	}
	complete := connect.NewRequest(&pb.CompleteRequest{Context: uploadRequest("a").Msg.Context})
	complete.Header().Set("Authorization", "Bearer manager")
	if _, e = s.Complete(ctx, complete); e != nil {
		t.Fatal(e)
	}
	if _, e = s.Upload(ctx, uploadRequest("c")); e == nil {
		t.Fatal("upload after completion")
	}
	issue := connect.NewRequest(&pb.IssueTokenRequest{RepositoryId: fixtureRepositoryID, ExpiresAt: time.Now().Add(time.Hour).Format(time.RFC3339)})
	issue.Header().Set("Authorization", "Bearer manager")
	issued, e := s.IssueToken(ctx, issue)
	if e != nil {
		t.Fatal(e)
	}
	next := uploadRequest("a")
	next.Msg.Context.ExecutionId = "next"
	next.Header().Set("Authorization", "Bearer "+issued.Msg.Token)
	if _, e = s.Upload(ctx, next); e != nil {
		t.Fatal(e)
	}
	revoke := connect.NewRequest(&pb.RevokeTokenRequest{RepositoryId: fixtureRepositoryID, TokenId: issued.Msg.TokenId})
	revoke.Header().Set("Authorization", "Bearer manager")
	if _, e = s.RevokeToken(ctx, revoke); e != nil {
		t.Fatal(e)
	}
	if _, e = s.Upload(ctx, next); connect.CodeOf(e) != connect.CodeUnauthenticated {
		t.Fatal("revoked token accepted", e)
	}
	get.Header().Set("Authorization", "Bearer other-space")
	if _, e = s.GetExecution(ctx, get); connect.CodeOf(e) != connect.CodePermissionDenied {
		t.Fatal("cross-space read", e)
	}
}
func TestWebhookSignatureAndRemoval(t *testing.T) {
	s := database(t)
	ctx := context.Background()
	s.DB.Exec(ctx, "UPDATE git_tracker_repositories SET public=true")
	raw := `{"action":"deleted","installation":{"id":1}}`
	handler := s.Webhook("test-secret")
	request := httptest.NewRequest("POST", "/github/events", strings.NewReader(raw))
	request.Header.Set("X-GitHub-Event", "installation")
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, request)
	if response.Code != 401 {
		t.Fatal(response.Code)
	}
	mac := hmac.New(sha256.New, []byte("test-secret"))
	mac.Write([]byte(raw))
	request = httptest.NewRequest("POST", "/github/events", strings.NewReader(raw))
	request.Header.Set("X-GitHub-Event", "installation")
	request.Header.Set("X-Hub-Signature-256", "sha256="+hex.EncodeToString(mac.Sum(nil)))
	response = httptest.NewRecorder()
	handler.ServeHTTP(response, request)
	if response.Code != 204 {
		t.Fatal(response.Code)
	}
	var public, connected bool
	s.DB.QueryRow(ctx, "SELECT public,connected FROM git_tracker_repositories WHERE github_id=42").Scan(&public, &connected)
	if public || connected {
		t.Fatal("removal retained public access")
	}
}

func (testAuth) RequireSpaceManager(context.Context, string, string) error { return nil }
func (testAuth) Identity(context.Context, string) (string, error) {
	return "019b0000-0000-7000-8000-000000000002", nil
}
func (testAuth) Finalized(context.Context, []string) ([]string, error) { return nil, nil }
func (testGitHub) User(context.Context, string) (string, error)        { return "1", nil }
func (testGitHub) Branch(context.Context, *pb.Repository, string) ([]string, error) {
	return []string{strings.Repeat("a", 40)}, nil
}

func TestUploadCompletionRaceAndInterruption(t *testing.T) {
	s := database(t)
	ctx := context.Background()
	q := uploadRequest("first")
	if _, e := s.Upload(ctx, q); e != nil {
		t.Fatal(e)
	}
	ready := make(chan struct{})
	done := make(chan error, 2)
	go func() { <-ready; _, e := s.Upload(ctx, uploadRequest("second")); done <- e }()
	go func() {
		<-ready
		c := connect.NewRequest(&pb.CompleteRequest{Context: q.Msg.Context})
		c.Header().Set("Authorization", "Bearer manager")
		_, e := s.Complete(ctx, c)
		done <- e
	}()
	close(ready)
	for range 2 {
		e := <-done
		if e != nil && connect.CodeOf(e) != connect.CodeFailedPrecondition {
			t.Fatal(e)
		}
	}
	get := connect.NewRequest(&pb.GetExecutionRequest{RepositoryId: fixtureRepositoryID, ExecutionId: "run"})
	get.Header().Set("Authorization", "Bearer manager")
	out, e := s.GetExecution(ctx, get)
	if e != nil || out.Msg.Execution.State != pb.ExecutionState_EXECUTION_STATE_COMPLETED {
		t.Fatal("race did not complete", e)
	}
	now := time.Now().UTC()
	s.Now = func() time.Time { return now }
	next := uploadRequest("first")
	next.Msg.Context.ExecutionId = "interrupted"
	if _, e = s.Upload(ctx, next); e != nil {
		t.Fatal(e)
	}
	s.Now = func() time.Time { return now.Add(5 * time.Hour) }
	if _, e = s.Upload(ctx, next); e != nil {
		t.Fatal(e)
	}
	s.Now = func() time.Time { return now.Add(6 * time.Hour) }
	if e = s.Maintain(ctx); e != nil {
		t.Fatal(e)
	}
	get.Msg.ExecutionId = "interrupted"
	out, e = s.GetExecution(ctx, get)
	if e != nil || out.Msg.Execution.State != pb.ExecutionState_EXECUTION_STATE_INTERRUPTED || len(out.Msg.Execution.Measurements) == 0 {
		t.Fatal("interruption failed", e)
	}
	var audits int
	if e = s.DB.QueryRow(ctx, "SELECT count(*) FROM git_tracker_audit").Scan(&audits); e != nil || audits != 0 {
		t.Fatal("expired audits retained", e)
	}
}

type failingCommentGitHub struct{ testGitHub }

func (failingCommentGitHub) Comment(context.Context, *pb.Repository, *measurement.State, *measurement.Record) error {
	return fault(connect.CodeUnavailable, "fixture comment failure")
}

type finalAuth struct{ testAuth }

func (finalAuth) Finalized(context.Context, []string) ([]string, error) {
	return []string{"019b0000-0000-7000-8000-000000000001"}, nil
}

type multiSpaceAuth struct{ testAuth }

func (multiSpaceAuth) Allow(_ context.Context, h, _ string, _ Role) error {
	if h != "Bearer manager" {
		return fault(connect.CodePermissionDenied, "denied")
	}
	return nil
}
func TestCommentFailureAndFinalSpaceDeletion(t *testing.T) {
	s := database(t)
	ctx := context.Background()
	s.GitHub = failingCommentGitHub{}
	result, e := s.Upload(ctx, uploadRequest("row"))
	if e != nil || len(result.Msg.Warnings) != 1 {
		t.Fatal("comment error changed upload result", e)
	}
	q := connect.NewRequest(&pb.IssueTokenRequest{RepositoryId: fixtureRepositoryID, ExpiresAt: time.Now().Add(time.Hour).Format(time.RFC3339)})
	q.Header().Set("Authorization", "Bearer manager")
	if _, e = s.IssueToken(ctx, q); e != nil {
		t.Fatal(e)
	}
	s.Auth = finalAuth{}
	if e = s.Maintain(ctx); e != nil {
		t.Fatal(e)
	}
	for _, table := range []string{"git_tracker_repositories", "git_tracker_tokens", "git_tracker_links", "git_tracker_audit"} {
		var count int
		if e = s.DB.QueryRow(ctx, "SELECT count(*) FROM "+table).Scan(&count); e != nil || count != 0 {
			t.Fatal("retained finalized space state", table, count, e)
		}
	}
}
func TestConnectionTransferKeepsOldSpaceHistoryPrivate(t *testing.T) {
	s := database(t)
	ctx := context.Background()
	s.Auth = multiSpaceAuth{}
	old := fixtureRepositoryID
	if _, e := s.Upload(ctx, uploadRequest("retained")); e != nil {
		t.Fatal(e)
	}
	q := connect.NewRequest(&pb.ConnectRepositoryRequest{SpaceId: "019b0000-0000-7000-8000-000000000099", FullName: "owner/repo"})
	q.Header().Set("Authorization", "Bearer manager")
	if _, e := s.ConnectRepository(ctx, q); connect.CodeOf(e) != connect.CodeAlreadyExists {
		t.Fatal("duplicate active connection", e)
	}
	disconnect := connect.NewRequest(&pb.DisconnectRequest{RepositoryId: old})
	disconnect.Header().Set("Authorization", "Bearer manager")
	if _, e := s.Disconnect(ctx, disconnect); e != nil {
		t.Fatal(e)
	}
	next, e := s.ConnectRepository(ctx, q)
	if e != nil || next.Msg.Repository.Id == old {
		t.Fatal("connection reused across spaces", e)
	}
	var connected, public bool
	var raw []byte
	if e = s.DB.QueryRow(ctx, "SELECT connected,public,state FROM git_tracker_repositories WHERE id=$1", old).Scan(&connected, &public, &raw); e != nil || connected || public || !strings.Contains(string(raw), "retained") {
		t.Fatal("old space history moved or exposed", e)
	}
	if _, e = s.Upload(ctx, uploadRequest("blocked")); connect.CodeOf(e) != connect.CodeFailedPrecondition {
		t.Fatal("disconnected upload accepted", e)
	}
}

func TestConnectTransportPreservesDecimalsAndErrorMetadata(t *testing.T) {
	s := database(t)
	path, handler := s.Handler()
	mux := http.NewServeMux()
	mux.Handle(path, handler)
	server := httptest.NewServer(mux)
	defer server.Close()
	client := gittrackerv1connect.NewGitTrackerServiceClient(server.Client(), server.URL, connect.WithProtoJSON())
	q := uploadRequest("wire")
	out, e := client.Upload(context.Background(), q)
	if e != nil {
		t.Fatal(e)
	}
	if out.Msg.Execution.Measurements[0].Rows[0].Values["bytes"] != q.Msg.Rows[0].Values["bytes"] {
		t.Fatal("transport rounded decimal")
	}
	if out.Header().Get("Cache-Control") != "no-store" || out.Header().Get("X-Correlation-Id") == "" {
		t.Fatal("response metadata missing")
	}
	q.Header().Set("Authorization", "Bearer denied")
	if _, e = client.Upload(context.Background(), q); connect.CodeOf(e) != connect.CodePermissionDenied {
		t.Fatal("wire access denied incorrectly", e)
	}
	var ce *connect.Error
	if !errors.As(e, &ce) || ce.Meta().Get("X-Correlation-Id") == "" {
		t.Fatal("error correlation missing")
	}
}

func TestTelemetryDoesNotExposeCredentialsOrMeasurements(t *testing.T) {
	s := database(t)
	var logs bytes.Buffer
	s.Log = slog.New(slog.NewJSONHandler(&logs, nil))
	q := uploadRequest("private-file")
	if _, e := s.Upload(context.Background(), q); e != nil {
		t.Fatal(e)
	}
	q.Header().Set("Authorization", "Bearer private-test-secret")
	s.Upload(context.Background(), q)
	for _, secret := range []string{"private-test-secret", "9007199254740993.00000000000001", "private-file"} {
		if strings.Contains(logs.String(), secret) {
			t.Fatal("telemetry contains sensitive payload")
		}
	}
	if !strings.Contains(logs.String(), "correlation_id") {
		t.Fatal("missing request correlation")
	}
	var audit string
	if e := s.DB.QueryRow(context.Background(), "SELECT COALESCE(string_agg(action,' '),'') FROM git_tracker_audit").Scan(&audit); e != nil {
		t.Fatal(e)
	}
	if strings.Contains(audit, "secret") || strings.Contains(audit, "9007199") {
		t.Fatal("audit leaked payload")
	}
}

type unavailableGitHub struct{ testGitHub }

func (unavailableGitHub) Resolve(context.Context, *pb.Repository, *pb.Context) ([]string, string, error) {
	return nil, "", errors.New("provider unavailable")
}
func TestCompletedRetrySurvivesGitHubOutage(t *testing.T) {
	s := database(t)
	ctx := context.Background()
	q := connect.NewRequest(&pb.CompleteRequest{Context: uploadRequest("row").Msg.Context})
	q.Header().Set("Authorization", "Bearer manager")
	first, err := s.Complete(ctx, q)
	if err != nil {
		t.Fatal(err)
	}
	s.GitHub = unavailableGitHub{}
	retry, err := s.Complete(ctx, q)
	if err != nil {
		t.Fatal(err)
	}
	if first.Msg.Execution.CompletedAt != retry.Msg.Execution.CompletedAt || first.Msg.Execution.RegistrationOrder != retry.Msg.Execution.RegistrationOrder {
		t.Fatal("retry changed completion")
	}
	q.Msg.Context.Commit = strings.Repeat("b", 40)
	if _, err = s.Complete(ctx, q); connect.CodeOf(err) != connect.CodeFailedPrecondition {
		t.Fatal("conflicting retry accepted", err)
	}
	q.Msg.Context.ExecutionId = "new"
	if _, err = s.Complete(ctx, q); connect.CodeOf(err) != connect.CodeUnavailable {
		t.Fatal("new completion bypassed verification", err)
	}
}

func TestDisconnectedHistoryUsesBranchAxis(t *testing.T) {
	s := database(t)
	ctx := context.Background()
	branch := uploadRequest("branch")
	if _, err := s.Upload(ctx, branch); err != nil {
		t.Fatal(err)
	}
	pr := uploadRequest("pr")
	pr.Msg.Context.ExecutionId = "pr"
	pr.Msg.Context.PullRequest = 1
	pr.Msg.Context.Commit = strings.Repeat("b", 40)
	if _, err := s.Upload(ctx, pr); err != nil {
		t.Fatal(err)
	}
	if _, err := s.DB.Exec(ctx, "UPDATE git_tracker_repositories SET connected=false WHERE id=$1", fixtureRepositoryID); err != nil {
		t.Fatal(err)
	}
	q := connect.NewRequest(&pb.GetHistoryRequest{RepositoryId: fixtureRepositoryID, Branch: "main"})
	q.Header().Set("Authorization", "Bearer manager")
	history, err := s.GetHistory(ctx, q)
	if err != nil {
		t.Fatal(err)
	}
	if len(history.Msg.Commits) != 1 || history.Msg.Commits[0] != branch.Msg.Context.Commit {
		t.Fatalf("unexpected branch axis: %v", history.Msg.Commits)
	}
	if len(history.Msg.Executions) != 2 {
		t.Fatal("PR execution was removed from history")
	}
}

func TestMaintenanceEventsRequireCommit(t *testing.T) {
	for _, deferred := range []bool{false, true} {
		t.Run(fmt.Sprint(deferred), func(t *testing.T) {
			s := database(t)
			ctx := context.Background()
			now := time.Now().UTC()
			s.Now = func() time.Time { return now }
			if _, err := s.Upload(ctx, uploadRequest("row")); err != nil {
				t.Fatal(err)
			}
			s.Now = func() time.Time { return now.Add(6 * time.Hour) }
			events := 0
			s.Event = func(kind, category string) {
				if kind == "execution" && category == "interrupted" {
					events++
				}
			}
			var logs bytes.Buffer
			s.Log = slog.New(slog.NewTextHandler(&logs, nil))
			trigger := "CREATE TRIGGER fail_maintenance BEFORE UPDATE ON git_tracker_repositories FOR EACH ROW EXECUTE FUNCTION fail_maintenance()"
			if deferred {
				trigger = "CREATE CONSTRAINT TRIGGER fail_maintenance AFTER UPDATE ON git_tracker_repositories DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION fail_maintenance()"
			}
			if _, err := s.DB.Exec(ctx, "CREATE OR REPLACE FUNCTION fail_maintenance() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture failure'; END $$"); err != nil {
				t.Fatal(err)
			}
			if _, err := s.DB.Exec(ctx, trigger); err != nil {
				t.Fatal(err)
			}
			if err := s.Maintain(ctx); err == nil {
				t.Fatal("expected persistence failure")
			}
			if events != 0 || strings.Contains(logs.String(), "executions interrupted") {
				t.Fatal("reported uncommitted interruption")
			}
			if _, err := s.DB.Exec(ctx, "DROP TRIGGER fail_maintenance ON git_tracker_repositories"); err != nil {
				t.Fatal(err)
			}
			if err := s.Maintain(ctx); err != nil {
				t.Fatal(err)
			}
			if events != 1 || !strings.Contains(logs.String(), "executions interrupted") {
				t.Fatal("missing committed interruption")
			}
			if err := s.Maintain(ctx); err != nil {
				t.Fatal(err)
			}
			if events != 1 {
				t.Fatal("duplicate interruption")
			}
		})
	}
}

func TestHistoryRepresentativeIncludesPR(t *testing.T) {
	s := database(t)
	ctx := context.Background()
	for _, pr := range []int64{0, 1} {
		c := uploadRequest("row").Msg.Context
		c.PullRequest = pr
		c.ExecutionId = fmt.Sprint(pr)
		q := connect.NewRequest(&pb.CompleteRequest{Context: c})
		q.Header().Set("Authorization", "Bearer manager")
		if _, e := s.Complete(ctx, q); e != nil {
			t.Fatal(e)
		}
	}
	q := connect.NewRequest(&pb.GetHistoryRequest{RepositoryId: fixtureRepositoryID})
	q.Header().Set("Authorization", "Bearer manager")
	res, e := s.GetHistory(ctx, q)
	if e != nil {
		t.Fatal(e)
	}
	if len(res.Msg.Executions) != 2 {
		t.Fatal(res.Msg)
	}
	for _, execution := range res.Msg.Executions {
		if execution.Representative != (execution.Context.PullRequest == 1) {
			t.Fatal("incorrect representative", execution)
		}
	}
}

func TestUploadRetrySurvivesGitHubOutage(t *testing.T) {
	s := database(t)
	ctx := context.Background()
	q := uploadRequest("row")
	first, e := s.Upload(ctx, q)
	if e != nil {
		t.Fatal(e)
	}
	s.GitHub = unavailableGitHub{}
	retry, e := s.Upload(ctx, q)
	if e != nil {
		t.Fatal(e)
	}
	if first.Msg.Execution.RegistrationOrder != retry.Msg.Execution.RegistrationOrder {
		t.Fatal("retry registered new execution")
	}
	q.Msg.Rows[0].Values["bytes"] = "1"
	if _, e = s.Upload(ctx, q); connect.CodeOf(e) != connect.CodeUnavailable {
		t.Fatal("changed data bypassed verification", e)
	}
	q = uploadRequest("new")
	if _, e = s.Upload(ctx, q); connect.CodeOf(e) != connect.CodeUnavailable {
		t.Fatal("new data bypassed verification", e)
	}
}

func TestConnectionDatabaseFailureIsSanitized(t *testing.T) {
	s := database(t)
	ctx := context.Background()
	if _, e := s.DB.Exec(ctx, "ALTER TABLE git_tracker_repositories RENAME TO private_internal_table"); e != nil {
		t.Fatal(e)
	}
	t.Cleanup(func() { s.DB.Exec(ctx, "ALTER TABLE private_internal_table RENAME TO git_tracker_repositories") })
	q := connect.NewRequest(&pb.ConnectRepositoryRequest{SpaceId: "019b0000-0000-7000-8000-000000000001", FullName: "owner/repo"})
	q.Header().Set("Authorization", "Bearer manager")
	_, err := s.ConnectRepository(ctx, q)
	var ce *connect.Error
	if !errors.As(err, &ce) || ce.Code() != connect.CodeInternal || ce.Meta().Get("X-Correlation-Id") == "" || strings.Contains(ce.Message(), "git_tracker") {
		t.Fatal("unsafe database failure", err)
	}
}

func TestHistoryCommitBoundsBeforePagination(t *testing.T) {
	s := database(t)
	ctx := context.Background()
	state := measurement.NewState()
	ancestors := []string{}
	for i := 1; i <= 102; i++ {
		c := &pb.Context{RepositoryId: fixtureRepositoryID, ExecutionId: fmt.Sprint(i), Commit: fmt.Sprintf("%040x", i), Branch: "main"}
		r := state.Register(c, time.Now(), append([]string{}, ancestors...), "")
		r.Execution.State = pb.ExecutionState_EXECUTION_STATE_COMPLETED
		ancestors = append([]string{c.Commit}, ancestors...)
	}
	raw, _ := json.Marshal(state)
	if _, e := s.DB.Exec(ctx, "UPDATE git_tracker_repositories SET connected=false,state=$2 WHERE id=$1", fixtureRepositoryID, raw); e != nil {
		t.Fatal(e)
	}
	q := connect.NewRequest(&pb.GetHistoryRequest{RepositoryId: fixtureRepositoryID, Branch: "main", FromCommit: fmt.Sprintf("%040x", 1), ToCommit: fmt.Sprintf("%040x", 1)})
	q.Header().Set("Authorization", "Bearer manager")
	res, e := s.GetHistory(ctx, q)
	if e != nil {
		t.Fatal(e)
	}
	if len(res.Msg.Executions) != 1 || res.Msg.Executions[0].Context.ExecutionId != "1" || res.Msg.NextCursor != "" {
		t.Fatal("range paginated before filtering", res.Msg)
	}
}

type unavailableBranch struct{ testGitHub }

func (unavailableBranch) Branch(context.Context, *pb.Repository, string) ([]string, error) {
	return nil, errors.New("provider unavailable")
}
func TestBranchLookupFailureEmitsEvent(t *testing.T) {
	s := database(t)
	s.GitHub = unavailableBranch{}
	count := 0
	s.Event = func(kind, category string) {
		if kind == "github" && category == "failure" {
			count++
		}
	}
	q := connect.NewRequest(&pb.GetHistoryRequest{RepositoryId: fixtureRepositoryID, Branch: "main"})
	q.Header().Set("Authorization", "Bearer manager")
	if _, e := s.GetHistory(context.Background(), q); connect.CodeOf(e) != connect.CodeUnavailable || count != 1 {
		t.Fatal("missing failure event", e, count)
	}
}
