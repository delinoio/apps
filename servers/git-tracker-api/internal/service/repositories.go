// SPDX-License-Identifier: FSL-1.1-ALv2

package service

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"sort"
	"strconv"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/gittracker/v1"
	"github.com/delinoio/apps/servers/git-tracker-api/internal/measurement"
	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
)

func (s *Server) ConnectRepository(ctx context.Context, q *connect.Request[pb.ConnectRepositoryRequest]) (response *connect.Response[pb.ConnectRepositoryResponse], err error) {
	defer func() {
		if err == nil {
			return
		}
		var ce *connect.Error
		if !errors.As(err, &ce) {
			s.event("persistence", "failure")
			s.Log.WarnContext(ctx, "repository connection persistence failed", "correlation_id", correlation(ctx), "error_type", fmt.Sprintf("%T", err))
			err = fault(connect.CodeInternal, "connection could not be saved")
			ce = err.(*connect.Error)
		}
		ce.Meta().Set("X-Correlation-Id", correlation(ctx))
	}()
	if _, e := uuid.Parse(q.Msg.SpaceId); e != nil {
		return nil, fault(connect.CodeInvalidArgument, "invalid space")
	}
	if e := s.Auth.Allow(ctx, q.Header().Get("Authorization"), q.Msg.SpaceId, Manage); e != nil {
		return nil, e
	}
	if e := s.Auth.RequireSpaceManager(ctx, q.Header().Get("Authorization"), q.Msg.SpaceId); e != nil {
		return nil, e
	}
	actor, e := s.Auth.Identity(ctx, q.Header().Get("Authorization"))
	if e != nil {
		return nil, e
	}
	githubUser, e := s.GitHub.User(ctx, q.Msg.GithubAccessToken)
	if e != nil {
		return nil, fault(connect.CodePermissionDenied, "GitHub account verification failed")
	}
	r, e := s.GitHub.Connect(ctx, q.Msg.FullName, q.Msg.GithubAccessToken)
	if e != nil {
		return nil, fault(connect.CodePermissionDenied, "GitHub administrator access and App installation are required")
	}
	r.SpaceId = q.Msg.SpaceId
	tx, e := s.DB.Begin(ctx)
	if e != nil {
		return nil, fault(connect.CodeUnavailable, "database unavailable")
	}
	defer tx.Rollback(ctx)
	// Serialize connection changes by GitHub's stable external repository ID.
	if _, e = tx.Exec(ctx, "SELECT pg_advisory_xact_lock(hashtextextended($1,0))", r.GithubRepositoryId); e != nil {
		return nil, e
	}
	var activeSpace string
	e = tx.QueryRow(ctx, "SELECT space_id::text FROM git_tracker_repositories WHERE github_id=$1 AND connected FOR UPDATE", r.GithubRepositoryId).Scan(&activeSpace)
	if e != nil && !errors.Is(e, pgx.ErrNoRows) {
		return nil, e
	}
	if activeSpace != "" && activeSpace != r.SpaceId {
		return nil, fault(connect.CodeAlreadyExists, "repository is connected to another space")
	}
	r.Id = id()
	e = tx.QueryRow(ctx, `INSERT INTO git_tracker_repositories(id,github_id,space_id,full_name,installation_id,public) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(github_id,space_id) DO UPDATE SET full_name=EXCLUDED.full_name,installation_id=EXCLUDED.installation_id,public=EXCLUDED.public,connected=true RETURNING id::text`, r.Id, r.GithubRepositoryId, r.SpaceId, r.FullName, r.InstallationId, r.Public).Scan(&r.Id)
	if e == nil {
		_, e = tx.Exec(ctx, "INSERT INTO git_tracker_links(space_id,user_id,github_id) VALUES($1,$2,$3) ON CONFLICT(space_id,user_id) DO UPDATE SET github_id=EXCLUDED.github_id", r.SpaceId, actor, githubUser)
	}
	if e == nil {
		e = s.audit(ctx, tx, r.SpaceId, "repository_connected")
	}
	if e == nil {
		e = tx.Commit(ctx)
	}
	if e != nil {
		return nil, fault(connect.CodeInternal, "connection could not be saved")
	}
	return connect.NewResponse(&pb.ConnectRepositoryResponse{Repository: r}), nil
}
func (s *Server) ListRepositories(ctx context.Context, q *connect.Request[pb.ListRepositoriesRequest]) (*connect.Response[pb.ListRepositoriesResponse], error) {
	if q.Msg.SpaceId != "" {
		if e := s.Auth.Allow(ctx, q.Header().Get("Authorization"), q.Msg.SpaceId, View); e != nil {
			return nil, e
		}
	}
	rows, e := s.DB.Query(ctx, `SELECT id::text,github_id::text,space_id::text,full_name,installation_id,public,connected FROM git_tracker_repositories WHERE ($1='' AND public AND connected) OR space_id::text=$1 ORDER BY full_name`, q.Msg.SpaceId)
	if e != nil {
		return nil, fault(connect.CodeUnavailable, "repository list unavailable")
	}
	defer rows.Close()
	out := &pb.ListRepositoriesResponse{}
	for rows.Next() {
		r := &pb.Repository{}
		if e = rows.Scan(&r.Id, &r.GithubRepositoryId, &r.SpaceId, &r.FullName, &r.InstallationId, &r.Public, &r.Connected); e != nil {
			return nil, e
		}
		out.Repositories = append(out.Repositories, r)
	}
	if e = rows.Err(); e != nil {
		return nil, e
	}
	resp := connect.NewResponse(out)
	resp.Header().Set("Cache-Control", "no-store")
	return resp, nil
}
func (s *Server) GetHistory(ctx context.Context, q *connect.Request[pb.GetHistoryRequest]) (*connect.Response[pb.GetHistoryResponse], error) {
	out := &pb.GetHistoryResponse{}
	var since, until time.Time
	for _, v := range []struct {
		s string
		t *time.Time
	}{{q.Msg.Since, &since}, {q.Msg.Until, &until}} {
		if v.s != "" {
			t, e := time.Parse(time.RFC3339, v.s)
			if e != nil {
				return nil, fault(connect.CodeInvalidArgument, "invalid date range")
			}
			*v.t = t
		}
	}
	offset := 0
	if q.Msg.Cursor != "" {
		n, e := strconv.Atoi(q.Msg.Cursor)
		if e != nil || n < 0 {
			return nil, fault(connect.CodeInvalidArgument, "invalid cursor")
		}
		offset = n
	}
	return run(s, ctx, q, out, q.Msg.RepositoryId, View, nil, func(_ pgx.Tx, repo *pb.Repository, state *measurement.State) error {
		records := []*measurement.Record{}
		for _, r := range state.Records {
			c := r.Execution.Context
			if q.Msg.Branch != "" && c.Branch != q.Msg.Branch {
				continue
			}
			at, _ := time.Parse(time.RFC3339Nano, r.Execution.RegisteredAt)
			if !since.IsZero() && at.Before(since) || !until.IsZero() && at.After(until) {
				continue
			}
			records = append(records, r)
		}
		sort.Slice(records, func(i, j int) bool {
			return records[i].Execution.RegistrationOrder > records[j].Execution.RegistrationOrder
		})
		// Each execution retains verified first-parent ancestry; the most recently
		// registered branch execution supplies the graph axis, not upload timestamps.
		for _, record := range records {
			if record.Execution.Context.PullRequest == 0 {
				out.Commits = append([]string{record.Execution.Context.Commit}, record.Ancestors...)
				break
			}
		}
		if graph, ok := s.GitHub.(interface {
			Branch(context.Context, *pb.Repository, string) ([]string, error)
		}); ok && repo.Connected && q.Msg.Branch != "" {
			var commits []string
			var e error
			if github, ok := s.GitHub.(*GitHubClient); ok {
				needed := map[string]bool{}
				for _, record := range records {
					if record.Execution.Context.PullRequest == 0 && record.Execution.State == pb.ExecutionState_EXECUTION_STATE_COMPLETED {
						needed[record.Execution.Context.Commit] = true
					}
				}
				commits, e = github.branchUntil(ctx, repo, q.Msg.Branch, needed)
			} else {
				commits, e = graph.Branch(ctx, repo, q.Msg.Branch)
			}
			if e != nil {
				s.event("github", "failure")
				return fault(connect.CodeUnavailable, "branch history unavailable")
			}
			out.Commits = commits
		}
		if q.Msg.ToCommit != "" {
			for i, c := range out.Commits {
				if c == q.Msg.ToCommit {
					out.Commits = out.Commits[i:]
					break
				}
			}
		}
		if q.Msg.FromCommit != "" {
			for i, c := range out.Commits {
				if c == q.Msg.FromCommit {
					out.Commits = out.Commits[:i+1]
					break
				}
			}
		}
		if q.Msg.FromCommit != "" || q.Msg.ToCommit != "" {
			allowed := map[string]bool{}
			for _, commit := range out.Commits {
				allowed[commit] = true
			}
			filtered := records[:0]
			for _, record := range records {
				if allowed[record.Execution.Context.Commit] {
					filtered = append(filtered, record)
				}
			}
			records = filtered
		}
		if offset > len(records) {
			offset = len(records)
		}
		end := offset + 100
		if end > len(records) {
			end = len(records)
		} else {
			out.NextCursor = strconv.Itoa(end)
		}
		for _, r := range records[offset:end] {
			raw, _ := json.Marshal(r.Execution)
			e := &pb.Execution{}
			json.Unmarshal(raw, e)
			e.Representative = state.Representative(e.Context.Commit, time.Time{}) == r
			if q.Msg.DatasetId != "" {
				items := e.Measurements[:0]
				for _, m := range e.Measurements {
					if m.Dataset.Id == q.Msg.DatasetId {
						items = append(items, m)
					}
				}
				e.Measurements = items
			}
			out.Executions = append(out.Executions, e)
		}
		return nil
	})
}
