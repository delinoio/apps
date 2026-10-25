// SPDX-License-Identifier: Apache-2.0
package store

import (
	"connectrpc.com/connect"
	"context"
	lane "github.com/delinoio/apps/protos/lane/v1"
	"github.com/delinoio/apps/protos/lane/v1/lanev1connect"
	"google.golang.org/protobuf/proto"
)

var _ lanev1connect.LaneServiceHandler = (*Store)(nil)

func (s *Store) CreateRepository(ctx context.Context, req *connect.Request[lane.CreateRepositoryRequest]) (*connect.Response[lane.CreateRepositoryResponse], error) {
	r, repo, e := s.mutate(ctx, "CreateRepository", req.Msg)
	_ = repo
	return connect.NewResponse(&lane.CreateRepositoryResponse{Result: r}), e
}
func (s *Store) SaveRepository(ctx context.Context, req *connect.Request[lane.SaveRepositoryRequest]) (*connect.Response[lane.SaveRepositoryResponse], error) {
	r, repo, e := s.mutate(ctx, "SaveRepository", req.Msg)
	_ = repo
	return connect.NewResponse(&lane.SaveRepositoryResponse{Result: r}), e
}
func (s *Store) DeleteRepository(ctx context.Context, req *connect.Request[lane.DeleteRepositoryRequest]) (*connect.Response[lane.DeleteRepositoryResponse], error) {
	r, repo, e := s.mutate(ctx, "DeleteRepository", req.Msg)
	_ = repo
	return connect.NewResponse(&lane.DeleteRepositoryResponse{Result: r}), e
}
func (s *Store) CreateTask(ctx context.Context, req *connect.Request[lane.CreateTaskRequest]) (*connect.Response[lane.CreateTaskResponse], error) {
	r, repo, e := s.mutate(ctx, "CreateTask", req.Msg)
	_ = repo
	return connect.NewResponse(&lane.CreateTaskResponse{Result: r, Repository: repo}), e
}
func (s *Store) SaveTask(ctx context.Context, req *connect.Request[lane.SaveTaskRequest]) (*connect.Response[lane.SaveTaskResponse], error) {
	r, repo, e := s.mutate(ctx, "SaveTask", req.Msg)
	_ = repo
	return connect.NewResponse(&lane.SaveTaskResponse{Result: r}), e
}
func (s *Store) SetProgress(ctx context.Context, req *connect.Request[lane.SetProgressRequest]) (*connect.Response[lane.SetProgressResponse], error) {
	r, repo, e := s.mutate(ctx, "SetProgress", req.Msg)
	_ = repo
	return connect.NewResponse(&lane.SetProgressResponse{Result: r}), e
}
func (s *Store) SetProposal(ctx context.Context, req *connect.Request[lane.SetProposalRequest]) (*connect.Response[lane.SetProposalResponse], error) {
	r, repo, e := s.mutate(ctx, "SetProposal", req.Msg)
	_ = repo
	return connect.NewResponse(&lane.SetProposalResponse{Result: r}), e
}
func (s *Store) RestorePrompt(ctx context.Context, req *connect.Request[lane.RestorePromptRequest]) (*connect.Response[lane.RestorePromptResponse], error) {
	r, repo, e := s.mutate(ctx, "RestorePrompt", req.Msg)
	_ = repo
	return connect.NewResponse(&lane.RestorePromptResponse{Result: r}), e
}
func (s *Store) DeleteTask(ctx context.Context, req *connect.Request[lane.DeleteTaskRequest]) (*connect.Response[lane.DeleteTaskResponse], error) {
	r, repo, e := s.mutate(ctx, "DeleteTask", req.Msg)
	_ = repo
	return connect.NewResponse(&lane.DeleteTaskResponse{Result: r}), e
}
func (s *Store) ResolveConflict(ctx context.Context, req *connect.Request[lane.ResolveConflictRequest]) (*connect.Response[lane.ResolveConflictResponse], error) {
	r, repo, e := s.mutate(ctx, "ResolveConflict", req.Msg)
	_ = repo
	return connect.NewResponse(&lane.ResolveConflictResponse{Result: r}), e
}
func (s *Store) GetRepository(ctx context.Context, req *connect.Request[lane.GetRepositoryRequest]) (*connect.Response[lane.GetRepositoryResponse], error) {
	r, e := s.query(ctx, "GetRepository", req.Msg)
	if e != nil {
		return nil, e
	}
	return connect.NewResponse(r.(*lane.GetRepositoryResponse)), nil
}
func (s *Store) ListRepositories(ctx context.Context, req *connect.Request[lane.ListRepositoriesRequest]) (*connect.Response[lane.ListRepositoriesResponse], error) {
	r, e := s.query(ctx, "ListRepositories", req.Msg)
	if e != nil {
		return nil, e
	}
	return connect.NewResponse(r.(*lane.ListRepositoriesResponse)), nil
}
func (s *Store) GetTask(ctx context.Context, req *connect.Request[lane.GetTaskRequest]) (*connect.Response[lane.GetTaskResponse], error) {
	r, e := s.query(ctx, "GetTask", req.Msg)
	if e != nil {
		return nil, e
	}
	return connect.NewResponse(r.(*lane.GetTaskResponse)), nil
}
func (s *Store) ListTasks(ctx context.Context, req *connect.Request[lane.ListTasksRequest]) (*connect.Response[lane.ListTasksResponse], error) {
	r, e := s.query(ctx, "ListTasks", req.Msg)
	if e != nil {
		return nil, e
	}
	return connect.NewResponse(r.(*lane.ListTasksResponse)), nil
}
func (s *Store) ListHistory(ctx context.Context, req *connect.Request[lane.ListHistoryRequest]) (*connect.Response[lane.ListHistoryResponse], error) {
	r, e := s.query(ctx, "ListHistory", req.Msg)
	if e != nil {
		return nil, e
	}
	return connect.NewResponse(r.(*lane.ListHistoryResponse)), nil
}
func (s *Store) Invoke(ctx context.Context, method string, m proto.Message) (proto.Message, error) {
	switch method {
	case "CreateRepository":
		q, ok := m.(*lane.CreateRepositoryRequest)
		if !ok {
			return nil, domainError(1, "Wrong request type", "")
		}
		r, e := s.CreateRepository(ctx, connect.NewRequest(q))
		if r == nil {
			return nil, e
		}
		return r.Msg, e
	case "SaveRepository":
		q, ok := m.(*lane.SaveRepositoryRequest)
		if !ok {
			return nil, domainError(1, "Wrong request type", "")
		}
		r, e := s.SaveRepository(ctx, connect.NewRequest(q))
		if r == nil {
			return nil, e
		}
		return r.Msg, e
	case "DeleteRepository":
		q, ok := m.(*lane.DeleteRepositoryRequest)
		if !ok {
			return nil, domainError(1, "Wrong request type", "")
		}
		r, e := s.DeleteRepository(ctx, connect.NewRequest(q))
		if r == nil {
			return nil, e
		}
		return r.Msg, e
	case "CreateTask":
		q, ok := m.(*lane.CreateTaskRequest)
		if !ok {
			return nil, domainError(1, "Wrong request type", "")
		}
		r, e := s.CreateTask(ctx, connect.NewRequest(q))
		if r == nil {
			return nil, e
		}
		return r.Msg, e
	case "SaveTask":
		q, ok := m.(*lane.SaveTaskRequest)
		if !ok {
			return nil, domainError(1, "Wrong request type", "")
		}
		r, e := s.SaveTask(ctx, connect.NewRequest(q))
		if r == nil {
			return nil, e
		}
		return r.Msg, e
	case "SetProgress":
		q, ok := m.(*lane.SetProgressRequest)
		if !ok {
			return nil, domainError(1, "Wrong request type", "")
		}
		r, e := s.SetProgress(ctx, connect.NewRequest(q))
		if r == nil {
			return nil, e
		}
		return r.Msg, e
	case "SetProposal":
		q, ok := m.(*lane.SetProposalRequest)
		if !ok {
			return nil, domainError(1, "Wrong request type", "")
		}
		r, e := s.SetProposal(ctx, connect.NewRequest(q))
		if r == nil {
			return nil, e
		}
		return r.Msg, e
	case "RestorePrompt":
		q, ok := m.(*lane.RestorePromptRequest)
		if !ok {
			return nil, domainError(1, "Wrong request type", "")
		}
		r, e := s.RestorePrompt(ctx, connect.NewRequest(q))
		if r == nil {
			return nil, e
		}
		return r.Msg, e
	case "DeleteTask":
		q, ok := m.(*lane.DeleteTaskRequest)
		if !ok {
			return nil, domainError(1, "Wrong request type", "")
		}
		r, e := s.DeleteTask(ctx, connect.NewRequest(q))
		if r == nil {
			return nil, e
		}
		return r.Msg, e
	case "ResolveConflict":
		q, ok := m.(*lane.ResolveConflictRequest)
		if !ok {
			return nil, domainError(1, "Wrong request type", "")
		}
		r, e := s.ResolveConflict(ctx, connect.NewRequest(q))
		if r == nil {
			return nil, e
		}
		return r.Msg, e
	case "GetRepository":
		q, ok := m.(*lane.GetRepositoryRequest)
		if !ok {
			return nil, domainError(1, "Wrong request type", "")
		}
		r, e := s.GetRepository(ctx, connect.NewRequest(q))
		if r == nil {
			return nil, e
		}
		return r.Msg, e
	case "ListRepositories":
		q, ok := m.(*lane.ListRepositoriesRequest)
		if !ok {
			return nil, domainError(1, "Wrong request type", "")
		}
		r, e := s.ListRepositories(ctx, connect.NewRequest(q))
		if r == nil {
			return nil, e
		}
		return r.Msg, e
	case "GetTask":
		q, ok := m.(*lane.GetTaskRequest)
		if !ok {
			return nil, domainError(1, "Wrong request type", "")
		}
		r, e := s.GetTask(ctx, connect.NewRequest(q))
		if r == nil {
			return nil, e
		}
		return r.Msg, e
	case "ListTasks":
		q, ok := m.(*lane.ListTasksRequest)
		if !ok {
			return nil, domainError(1, "Wrong request type", "")
		}
		r, e := s.ListTasks(ctx, connect.NewRequest(q))
		if r == nil {
			return nil, e
		}
		return r.Msg, e
	case "ListHistory":
		q, ok := m.(*lane.ListHistoryRequest)
		if !ok {
			return nil, domainError(1, "Wrong request type", "")
		}
		r, e := s.ListHistory(ctx, connect.NewRequest(q))
		if r == nil {
			return nil, e
		}
		return r.Msg, e
	case "ListEvents", "GetEvent":
		return s.query(ctx, method, m)
	default:
		return nil, domainError(11, "Operation requires an explicit online target", "")
	}
}
