// SPDX-License-Identifier: FSL-1.1-ALv2

package service

import (
	"context"
	"strconv"
	"strings"

	"connectrpc.com/connect"
	"github.com/coreos/go-oidc/v3/oidc"
	dc "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/protos/delcore/v1/delcorev1connect"
	pb "github.com/delinoio/apps/protos/gittracker/v1"
)

type DelcoreAuthority struct {
	Client       delcorev1connect.DelcoreServiceClient
	ServiceToken func(context.Context) (string, error)
	Actions      *oidc.IDTokenVerifier
	GitHub       *GitHubClient
}

func (a *DelcoreAuthority) Allow(ctx context.Context, authorization, space string, role Role) error {
	q := connect.NewRequest(&dc.GetCurrentUserRequest{})
	q.Header().Set("Authorization", authorization)
	user, e := a.Client.GetCurrentUser(ctx, q)
	if e != nil {
		return fault(connect.CodeUnauthenticated, "Delino authentication failed")
	}
	token, e := a.ServiceToken(ctx)
	if e != nil {
		return fault(connect.CodeUnavailable, "authorization unavailable")
	}
	roles := []Role{role}
	if role == View {
		roles = append(roles, Edit, Manage)
	} else if role == Edit {
		roles = append(roles, Manage)
	}
	for _, r := range roles {
		check := connect.NewRequest(&dc.AuthorizeRequest{TenantId: space, UserId: user.Msg.UserId, Capability: string(r), Paid: false})
		check.Header().Set("Authorization", "Bearer "+token)
		result, e := a.Client.Authorize(ctx, check)
		if e != nil {
			return fault(connect.CodeUnavailable, "authorization unavailable")
		}
		if result.Msg.Allowed {
			return nil
		}
	}
	return fault(connect.CodePermissionDenied, "space permission is required")
}
func (a *DelcoreAuthority) VerifyActions(ctx context.Context, raw string, r *pb.Repository, c *pb.Context) error {
	denied := func() error {
		return fault(connect.CodePermissionDenied, "Actions execution identity is not authorized")
	}
	if !r.Connected || a.Actions == nil {
		return denied()
	}
	token, e := a.Actions.Verify(ctx, raw)
	if e != nil {
		return denied()
	}
	var claims struct {
		RepositoryID string `json:"repository_id"`
		RunID        string `json:"run_id"`
		RunAttempt   string `json:"run_attempt"`
		SHA          string `json:"sha"`
		Ref          string `json:"ref"`
		Event        string `json:"event_name"`
		Subject      string `json:"sub"`
	}
	if e = token.Claims(&claims); e != nil || claims.RepositoryID == "" || claims.RunID == "" || claims.RunAttempt == "" {
		return denied()
	}
	if c.ExecutionId != claims.RepositoryID+"/"+claims.RunID+"/"+claims.RunAttempt {
		return denied()
	}
	if c.PullRequest == 0 {
		if claims.RepositoryID != r.GithubRepositoryId || claims.SHA != c.Commit || claims.Ref != "refs/heads/"+c.Branch || claims.Event != "push" {
			return denied()
		}
		return nil
	}
	if claims.Event != "pull_request" {
		return denied()
	}
	p, e := a.GitHub.Pull(ctx, r, c.PullRequest)
	if e != nil {
		return fault(connect.CodeUnavailable, "GitHub PR verification unavailable")
	}
	// A fork identity can only write the PR head, never upstream push history or
	// an override baseline. The GitHub API, not caller input, supplies PR scope.
	if c.Branch != p.Base.Ref || c.BaselineCommit != "" {
		return denied()
	}
	if claims.RepositoryID != r.GithubRepositoryId && claims.RepositoryID != strconv.FormatInt(p.Head.Repo.ID, 10) {
		return denied()
	}
	if e := a.GitHub.VerifyMergeHead(ctx, r, claims.SHA, c.Commit); e != nil {
		if connect.CodeOf(e) == connect.CodePermissionDenied {
			return denied()
		}
		return fault(connect.CodeUnavailable, "GitHub merge verification unavailable")
	}
	if claims.Ref != "refs/pull/"+strconv.FormatInt(c.PullRequest, 10)+"/merge" || !strings.HasSuffix(claims.Subject, ":pull_request") {
		return denied()
	}
	return nil
}

func (a *DelcoreAuthority) RequireSpaceManager(ctx context.Context, authorization, space string) error {
	q := connect.NewRequest(&dc.ListMembersRequest{TenantId: space, Page: &dc.Page{Size: 1}})
	q.Header().Set("Authorization", authorization)
	if _, e := a.Client.ListMembers(ctx, q); e != nil {
		return fault(connect.CodePermissionDenied, "space owner or administrator authority is required")
	}
	return nil
}
func (a *DelcoreAuthority) Finalized(ctx context.Context, spaces []string) ([]string, error) {
	token, e := a.ServiceToken(ctx)
	if e != nil {
		return nil, e
	}
	q := connect.NewRequest(&dc.CheckTenantLifecycleRequest{TenantIds: spaces})
	q.Header().Set("Authorization", "Bearer "+token)
	result, e := a.Client.CheckTenantLifecycle(ctx, q)
	if e != nil {
		return nil, e
	}
	return result.Msg.FinalizedTenantIds, nil
}

func (a *DelcoreAuthority) Identity(ctx context.Context, authorization string) (string, error) {
	q := connect.NewRequest(&dc.GetCurrentUserRequest{})
	q.Header().Set("Authorization", authorization)
	result, e := a.Client.GetCurrentUser(ctx, q)
	if e != nil {
		return "", fault(connect.CodeUnauthenticated, "Delino authentication failed")
	}
	return result.Msg.UserId, nil
}
