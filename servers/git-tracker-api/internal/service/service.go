// SPDX-License-Identifier: FSL-1.1-ALv2

package service

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"log/slog"
	"net/http"
	"regexp"
	"strings"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/gittracker/v1"
	"github.com/delinoio/apps/protos/gittracker/v1/gittrackerv1connect"
	"github.com/delinoio/apps/servers/git-tracker-api/internal/measurement"
	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

type correlationKey struct{}

func correlation(ctx context.Context) string {
	if value, ok := ctx.Value(correlationKey{}).(string); ok {
		return value
	}
	return id()
}

type Role string

const (
	View   Role = "view"
	Edit   Role = "edit"
	Manage Role = "manage"
)

type Authority interface {
	Allow(context.Context, string, string, Role) error
	RequireSpaceManager(context.Context, string, string) error
	Identity(context.Context, string) (string, error)
	Finalized(context.Context, []string) ([]string, error)
	VerifyActions(context.Context, string, *pb.Repository, *pb.Context) error
}
type GitHub interface {
	Connect(context.Context, string, string) (*pb.Repository, error)
	User(context.Context, string) (string, error)
	Branch(context.Context, *pb.Repository, string) ([]string, error)
	Resolve(context.Context, *pb.Repository, *pb.Context) ([]string, string, error)
	Comment(context.Context, *pb.Repository, *measurement.State, *measurement.Record) error
}
type Server struct {
	gittrackerv1connect.UnimplementedGitTrackerServiceHandler
	DB             *pgxpool.Pool
	Auth           Authority
	GitHub         GitHub
	Log            *slog.Logger
	Now            func() time.Time
	Event          func(string, string)
	AuditRetention time.Duration
}

func (s *Server) now() time.Time {
	if s.Now != nil {
		return s.Now().UTC()
	}
	return time.Now().UTC()
}
func id() string                                { return uuid.Must(uuid.NewV7()).String() }
func fault(code connect.Code, msg string) error { return connect.NewError(code, errors.New(msg)) }
func (s *Server) Handler() (string, http.Handler) {
	path, handler := gittrackerv1connect.NewGitTrackerServiceHandler(s, connect.WithReadMaxBytes(8<<20))
	return path, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		requestID := id()
		r = r.WithContext(context.WithValue(r.Context(), correlationKey{}, requestID))
		w.Header().Set("X-Correlation-Id", requestID)
		w.Header().Set("Cache-Control", "no-store")
		handler.ServeHTTP(w, r)
	})
}

var shaPattern = regexp.MustCompile(`^[a-f0-9]{40}$`)

func validContext(c *pb.Context) error {
	if c == nil || c.RepositoryId == "" || c.ExecutionId == "" || len(c.ExecutionId) > 200 || !shaPattern.MatchString(c.Commit) || c.PullRequest < 0 || c.Branch == "" {
		return fault(connect.CodeInvalidArgument, "repository, execution, commit and branch are required")
	}
	if c.BaselineCommit != "" && !shaPattern.MatchString(c.BaselineCommit) {
		return fault(connect.CodeInvalidArgument, "baseline must be a full commit SHA")
	}
	return nil
}
func (s *Server) authorize(ctx context.Context, tx pgx.Tx, h string, r *pb.Repository, role Role, c *pb.Context) error {
	if role == View && r.Public && r.Connected {
		return nil
	}
	if h == "" {
		return fault(connect.CodeUnauthenticated, "sign in to access this repository")
	}
	token := strings.TrimPrefix(h, "Bearer ")
	if strings.HasPrefix(token, "gt_") {
		if role != Edit || !r.Connected {
			return fault(connect.CodePermissionDenied, "token cannot perform this operation")
		}
		digest := sha256.Sum256([]byte(token))
		var ok bool
		e := tx.QueryRow(ctx, `SELECT EXISTS(SELECT 1 FROM git_tracker_tokens WHERE digest=$1 AND repository_id=$2 AND NOT revoked AND expires_at>$3)`, digest[:], r.Id, s.now()).Scan(&ok)
		if e != nil {
			return e
		}
		if !ok {
			return fault(connect.CodeUnauthenticated, "invalid or expired token")
		}
		return nil
	}
	if role == Edit && c != nil && strings.HasPrefix(h, "Actions ") {
		return s.Auth.VerifyActions(ctx, strings.TrimPrefix(h, "Actions "), r, c)
	}
	return s.Auth.Allow(ctx, h, r.SpaceId, role)
}
func (s *Server) repository(ctx context.Context, tx pgx.Tx, rid string) (*pb.Repository, *measurement.State, error) {
	r := &pb.Repository{}
	var raw []byte
	e := tx.QueryRow(ctx, `SELECT id::text,github_id::text,space_id::text,full_name,installation_id,public,connected,state FROM git_tracker_repositories WHERE id=$1 FOR UPDATE`, rid).Scan(&r.Id, &r.GithubRepositoryId, &r.SpaceId, &r.FullName, &r.InstallationId, &r.Public, &r.Connected, &raw)
	if errors.Is(e, pgx.ErrNoRows) {
		return nil, nil, fault(connect.CodeNotFound, "repository not found")
	}
	if e != nil {
		return nil, nil, e
	}
	state := measurement.NewState()
	if e = json.Unmarshal(raw, state); e != nil {
		return nil, nil, e
	}
	return r, state, nil
}
func run[Q, R any](s *Server, ctx context.Context, q *connect.Request[Q], out *R, rid string, role Role, c *pb.Context, fn func(pgx.Tx, *pb.Repository, *measurement.State) error) (*connect.Response[R], error) {
	correlation := correlation(ctx)
	if _, err := uuid.Parse(rid); err != nil {
		return nil, fault(connect.CodeInvalidArgument, "invalid repository connection ID")
	}
	interrupted := 0
	tx, e := s.DB.Begin(ctx)
	if e == nil {
		defer tx.Rollback(ctx)
		var r *pb.Repository
		var state *measurement.State
		r, state, e = s.repository(ctx, tx, rid)
		if e == nil {
			e = s.authorize(ctx, tx, q.Header().Get("Authorization"), r, role, c)
		}
		if e == nil && role == Edit && !r.Connected {
			e = fault(connect.CodeFailedPrecondition, "repository is disconnected")
		}
		if e == nil {
			interrupted = state.Interrupt(s.now())
			e = fn(tx, r, state)
		}
		if e == nil {
			var raw []byte
			raw, e = json.Marshal(state)
			if e == nil {
				_, e = tx.Exec(ctx, "UPDATE git_tracker_repositories SET state=$2 WHERE id=$1", rid, raw)
			}
		}
		if e == nil {
			e = tx.Commit(ctx)
		}
	}
	if e != nil {
		var ce *connect.Error
		if !errors.As(e, &ce) {
			e = fault(connect.CodeInternal, "operation failed; retry with the correlation ID")
		}
		e.(*connect.Error).Meta().Set("X-Correlation-Id", correlation)
		if role == Edit && connect.CodeOf(e) == connect.CodeInternal {
			s.event("persistence", "failure")
		}
		s.Log.WarnContext(ctx, "request failed", "correlation_id", correlation, "category", connect.CodeOf(e).String())
		return nil, e
	}
	for i := 0; i < interrupted; i++ {
		s.event("execution", "interrupted")
	}
	if c != nil && role == Edit {
		s.Log.InfoContext(ctx, "measurement saved", "correlation_id", correlation, "execution_id", c.ExecutionId)
		if err := s.publish(ctx, rid, c.ExecutionId); err != nil {
			switch v := any(out).(type) {
			case *pb.UploadResponse:
				v.Warnings = append(v.Warnings, "PR comment could not be updated")
			case *pb.CompleteResponse:
				v.Warnings = append(v.Warnings, "PR comment could not be updated")
			}
		}
	}
	resp := connect.NewResponse(out)
	resp.Header().Set("X-Correlation-Id", correlation)
	resp.Header().Set("Cache-Control", "no-store")
	return resp, nil
}
func (s *Server) Upload(ctx context.Context, q *connect.Request[pb.UploadRequest]) (*connect.Response[pb.UploadResponse], error) {
	if e := validContext(q.Msg.Context); e != nil {
		return nil, e
	}
	o := &pb.UploadResponse{}
	return run(s, ctx, q, o, q.Msg.Context.RepositoryId, Edit, q.Msg.Context, func(tx pgx.Tx, r *pb.Repository, state *measurement.State) error {
		now := s.now()
		if state.IsUploadRetry(q.Msg.Context, q.Msg.Dataset, q.Msg.Rows) {
			record, err := state.Upload(q.Msg.Context, q.Msg.Dataset, q.Msg.Rows, now, nil, "")
			if err != nil {
				return fault(connect.CodeFailedPrecondition, err.Error())
			}
			o.Execution = record.Execution
			o.Comparisons = state.Comparisons(record, nil, false)
			return nil
		}
		cutoff := now
		if existing := state.Records[q.Msg.Context.ExecutionId]; existing != nil {
			cutoff = existing.Cutoff
		}
		var parents []string
		var base string
		var e error
		if g, ok := s.GitHub.(*GitHubClient); ok {
			parents, base, e = g.resolveRows(ctx, r, q.Msg.Context, state.BaselineCandidates(q.Msg.Dataset, q.Msg.Rows, cutoff))
		} else {
			parents, base, e = s.GitHub.Resolve(ctx, r, q.Msg.Context)
		}
		if e != nil {
			s.event("github", "failure")
			return fault(connect.CodeUnavailable, "GitHub commit verification failed")
		}
		record, e := state.Upload(q.Msg.Context, q.Msg.Dataset, q.Msg.Rows, now, parents, base)
		if e != nil {
			return fault(connect.CodeFailedPrecondition, e.Error())
		}
		o.Execution = record.Execution
		o.Comparisons = state.Comparisons(record, nil, false)
		return nil
	})
}
func (s *Server) Complete(ctx context.Context, q *connect.Request[pb.CompleteRequest]) (*connect.Response[pb.CompleteResponse], error) {
	if e := validContext(q.Msg.Context); e != nil {
		return nil, e
	}
	o := &pb.CompleteResponse{}
	return run(s, ctx, q, o, q.Msg.Context.RepositoryId, Edit, q.Msg.Context, func(tx pgx.Tx, r *pb.Repository, state *measurement.State) error {
		if existing := state.Records[q.Msg.Context.ExecutionId]; existing != nil && existing.Execution.State == pb.ExecutionState_EXECUTION_STATE_COMPLETED {
			record, e := state.Complete(q.Msg.Context, s.now())
			if e != nil {
				return fault(connect.CodeFailedPrecondition, e.Error())
			}
			o.Execution = record.Execution
			return nil
		}
		parents, _, e := s.GitHub.Resolve(ctx, r, q.Msg.Context)
		if e != nil {
			s.event("github", "failure")
			return fault(connect.CodeUnavailable, "GitHub commit verification failed")
		}
		record, e := state.Complete(q.Msg.Context, s.now())
		if e != nil {
			return fault(connect.CodeFailedPrecondition, e.Error())
		}
		if len(parents) > len(record.Ancestors) {
			record.Ancestors = parents
		}
		o.Execution = record.Execution
		return nil
	})
}
func (s *Server) GetExecution(ctx context.Context, q *connect.Request[pb.GetExecutionRequest]) (*connect.Response[pb.GetExecutionResponse], error) {
	o := &pb.GetExecutionResponse{}
	return run(s, ctx, q, o, q.Msg.RepositoryId, View, nil, func(_ pgx.Tx, _ *pb.Repository, state *measurement.State) error {
		r := state.Records[q.Msg.ExecutionId]
		if r == nil {
			return fault(connect.CodeNotFound, "execution not found")
		}
		o.Execution = r.Execution
		o.Comparisons = state.Comparisons(r, nil, false)
		return nil
	})
}
func (s *Server) Compare(ctx context.Context, q *connect.Request[pb.CompareRequest]) (*connect.Response[pb.CompareResponse], error) {
	o := &pb.CompareResponse{}
	return run(s, ctx, q, o, q.Msg.RepositoryId, View, nil, func(_ pgx.Tx, _ *pb.Repository, state *measurement.State) error {
		r := state.Representative(q.Msg.TargetCommit, time.Time{})
		if r == nil {
			return fault(connect.CodeNotFound, "completed target not found")
		}
		o.Comparisons = state.Comparisons(r, state.Representative(q.Msg.BaselineCommit, time.Time{}), true)
		return nil
	})
}
func (s *Server) audit(ctx context.Context, tx pgx.Tx, space, action string) error {
	_, e := tx.Exec(ctx, "INSERT INTO git_tracker_audit VALUES($1,$2,$3,$4,$5)", id(), space, action, correlation(ctx), s.now())
	return e
}
func (s *Server) IssueToken(ctx context.Context, q *connect.Request[pb.IssueTokenRequest]) (*connect.Response[pb.IssueTokenResponse], error) {
	expiry, e := time.Parse(time.RFC3339, q.Msg.ExpiresAt)
	if e != nil || !expiry.After(s.now()) {
		return nil, fault(connect.CodeInvalidArgument, "future expiration is required")
	}
	o := &pb.IssueTokenResponse{}
	return run(s, ctx, q, o, q.Msg.RepositoryId, Manage, nil, func(tx pgx.Tx, r *pb.Repository, _ *measurement.State) error {
		b := make([]byte, 32)
		if _, e := rand.Read(b); e != nil {
			return e
		}
		o.Token = "gt_" + base64.RawURLEncoding.EncodeToString(b)
		o.TokenId = id()
		sum := sha256.Sum256([]byte(o.Token))
		if _, e := tx.Exec(ctx, "INSERT INTO git_tracker_tokens(id,repository_id,digest,expires_at) VALUES($1,$2,$3,$4)", o.TokenId, r.Id, sum[:], expiry); e != nil {
			return e
		}
		return s.audit(ctx, tx, r.SpaceId, "token_issued")
	})
}
func (s *Server) RevokeToken(ctx context.Context, q *connect.Request[pb.RevokeTokenRequest]) (*connect.Response[pb.RevokeTokenResponse], error) {
	return run(s, ctx, q, &pb.RevokeTokenResponse{}, q.Msg.RepositoryId, Manage, nil, func(tx pgx.Tx, r *pb.Repository, _ *measurement.State) error {
		if _, e := tx.Exec(ctx, "UPDATE git_tracker_tokens SET revoked=true WHERE id::text=$1 AND repository_id=$2", q.Msg.TokenId, r.Id); e != nil {
			return e
		}
		return s.audit(ctx, tx, r.SpaceId, "token_revoked")
	})
}
func (s *Server) Disconnect(ctx context.Context, q *connect.Request[pb.DisconnectRequest]) (*connect.Response[pb.DisconnectResponse], error) {
	return run(s, ctx, q, &pb.DisconnectResponse{}, q.Msg.RepositoryId, Manage, nil, func(tx pgx.Tx, r *pb.Repository, _ *measurement.State) error {
		if _, e := tx.Exec(ctx, "UPDATE git_tracker_repositories SET connected=false,public=false WHERE id=$1", r.Id); e != nil {
			return e
		}
		return s.audit(ctx, tx, r.SpaceId, "repository_disconnected")
	})
}
func (s *Server) Delete(ctx context.Context, q *connect.Request[pb.DeleteRequest]) (*connect.Response[pb.DeleteResponse], error) {
	if !q.Msg.Confirmed {
		return nil, fault(connect.CodeInvalidArgument, "deletion confirmation is required")
	}
	return run(s, ctx, q, &pb.DeleteResponse{}, q.Msg.RepositoryId, Manage, nil, func(tx pgx.Tx, r *pb.Repository, state *measurement.State) error {
		switch q.Msg.Kind {
		case pb.DeleteKind_DELETE_KIND_REPOSITORY:
			if state.DeletedExecutions == nil {
				state.DeletedExecutions = map[string]bool{}
			}
			for id := range state.Records {
				state.DeletedExecutions[id] = true
			}
			state.Records = map[string]*measurement.Record{}
		case pb.DeleteKind_DELETE_KIND_EXECUTION:
			if state.DeletedExecutions == nil {
				state.DeletedExecutions = map[string]bool{}
			}
			state.DeletedExecutions[q.Msg.Id] = true
			delete(state.Records, q.Msg.Id)
		case pb.DeleteKind_DELETE_KIND_DATASET:
			for _, record := range state.Records {
				if record.RemovedDatasets == nil {
					record.RemovedDatasets = map[string]bool{}
				}
				record.RemovedDatasets[q.Msg.Id] = true
				items := record.Execution.Measurements[:0]
				for _, m := range record.Execution.Measurements {
					if m.Dataset.Id != q.Msg.Id {
						items = append(items, m)
					}
				}
				record.Execution.Measurements = items
			}
		default:
			return fault(connect.CodeInvalidArgument, "invalid deletion kind")
		}
		return s.audit(ctx, tx, r.SpaceId, "measurements_deleted")
	})
}

// Publish after persistence, under the same repository lock used by writers.
// Re-read current state so an older request cannot replace a newer comment.
func (s *Server) publish(ctx context.Context, rid, execution string) error {
	tx, e := s.DB.Begin(ctx)
	if e != nil {
		return e
	}
	defer tx.Rollback(ctx)
	r, state, e := s.repository(ctx, tx, rid)
	if e != nil {
		return e
	}
	record := state.Records[execution]
	if record == nil {
		return nil
	}
	e = s.GitHub.Comment(ctx, r, state, record)
	if e != nil {
		s.event("github", "failure")
	}
	return e
}

func (s *Server) event(kind, category string) {
	if s.Event != nil {
		s.Event(kind, category)
	}
}
