// SPDX-License-Identifier: FSL-1.1-ALv2

package service

import (
	"bytes"
	"connectrpc.com/connect"
	"context"
	"crypto/rsa"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"

	pb "github.com/delinoio/apps/protos/gittracker/v1"
	"github.com/delinoio/apps/servers/git-tracker-api/internal/measurement"
	"github.com/golang-jwt/jwt/v4"
)

type GitHubClient struct {
	HTTP    *http.Client
	BaseURL string
	AppID   string
	Key     *rsa.PrivateKey
}
type githubRepository struct {
	ID          int64  `json:"id"`
	FullName    string `json:"full_name"`
	Private     bool   `json:"private"`
	Permissions struct {
		Admin bool `json:"admin"`
	} `json:"permissions"`
}
type pull struct {
	Head struct {
		SHA  string           `json:"sha"`
		Repo githubRepository `json:"repo"`
	} `json:"head"`
	Base struct {
		SHA  string           `json:"sha"`
		Ref  string           `json:"ref"`
		Repo githubRepository `json:"repo"`
	} `json:"base"`
}

func (g *GitHubClient) call(ctx context.Context, method, path, token string, input, out any) error {
	var body io.Reader
	if input != nil {
		b, e := json.Marshal(input)
		if e != nil {
			return e
		}
		body = bytes.NewReader(b)
	}
	req, e := http.NewRequestWithContext(ctx, method, g.BaseURL+path, body)
	if e != nil {
		return e
	}
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("Accept", "application/vnd.github+json")
	req.Header.Set("X-GitHub-Api-Version", "2022-11-28")
	req.Header.Set("Content-Type", "application/json")
	res, e := g.HTTP.Do(req)
	if e != nil {
		return errors.New("GitHub request failed")
	}
	defer res.Body.Close()
	if res.StatusCode < 200 || res.StatusCode >= 300 {
		return fmt.Errorf("GitHub status %d", res.StatusCode)
	}
	if out == nil {
		return nil
	}
	return json.NewDecoder(io.LimitReader(res.Body, 8<<20)).Decode(out)
}
func (g *GitHubClient) appToken() (string, error) {
	now := time.Now()
	return jwt.NewWithClaims(jwt.SigningMethodRS256, jwt.RegisteredClaims{Issuer: g.AppID, IssuedAt: jwt.NewNumericDate(now.Add(-time.Minute)), ExpiresAt: jwt.NewNumericDate(now.Add(8 * time.Minute))}).SignedString(g.Key)
}
func (g *GitHubClient) installation(ctx context.Context, installation string) (string, error) {
	token, e := g.appToken()
	if e != nil {
		return "", e
	}
	var out struct {
		Token string `json:"token"`
	}
	e = g.call(ctx, "POST", "/app/installations/"+url.PathEscape(installation)+"/access_tokens", token, struct{}{}, &out)
	return out.Token, e
}
func repositoryIDPath(r *pb.Repository) string {
	return "/repositories/" + url.PathEscape(r.GithubRepositoryId)
}
func repoPath(r *pb.Repository) string {
	owner, name, _ := strings.Cut(r.FullName, "/")
	return "/repos/" + url.PathEscape(owner) + "/" + url.PathEscape(name)
}
func (g *GitHubClient) Connect(ctx context.Context, name, userToken string) (*pb.Repository, error) {
	parts := strings.Split(name, "/")
	if len(parts) != 2 || parts[0] == "" || parts[1] == "" {
		return nil, errors.New("owner/repository required")
	}
	path := "/repos/" + url.PathEscape(parts[0]) + "/" + url.PathEscape(parts[1])
	var repo githubRepository
	if e := g.call(ctx, "GET", path, userToken, nil, &repo); e != nil {
		return nil, e
	}
	if !repo.Permissions.Admin {
		return nil, errors.New("GitHub repository administrator required")
	}
	token, e := g.appToken()
	if e != nil {
		return nil, e
	}
	var install struct {
		ID int64 `json:"id"`
	}
	if e = g.call(ctx, "GET", path+"/installation", token, nil, &install); e != nil {
		return nil, e
	}
	r := &pb.Repository{GithubRepositoryId: strconv.FormatInt(repo.ID, 10), FullName: repo.FullName, Public: !repo.Private, Connected: true, InstallationId: strconv.FormatInt(install.ID, 10)}
	access, e := g.installation(ctx, r.InstallationId)
	if e != nil {
		return nil, e
	}
	if e = g.call(ctx, "GET", repositoryIDPath(r), access, nil, &repo); e != nil {
		return nil, e
	}
	r.Public = !repo.Private
	r.FullName = repo.FullName
	return r, nil
}
func (g *GitHubClient) Pull(ctx context.Context, r *pb.Repository, n int64) (*pull, error) {
	token, e := g.installation(ctx, r.InstallationId)
	if e != nil {
		return nil, e
	}
	p := &pull{}
	e = g.call(ctx, "GET", repoPath(r)+"/pulls/"+strconv.FormatInt(n, 10), token, nil, p)
	return p, e
}
func (g *GitHubClient) Resolve(ctx context.Context, r *pb.Repository, c *pb.Context) ([]string, string, error) {
	return g.resolveRows(ctx, r, c, nil)
}
func (g *GitHubClient) resolveRows(ctx context.Context, r *pb.Repository, c *pb.Context, candidates map[string]map[string]bool) ([]string, string, error) {
	token, e := g.installation(ctx, r.InstallationId)
	if e != nil {
		return nil, "", e
	}
	var commit struct {
		SHA     string `json:"sha"`
		Parents []struct {
			SHA string `json:"sha"`
		} `json:"parents"`
	}
	if e = g.call(ctx, "GET", repoPath(r)+"/commits/"+url.PathEscape(c.Commit), token, nil, &commit); e != nil || commit.SHA != c.Commit {
		return nil, "", errors.New("commit is unavailable")
	}
	if c.PullRequest > 0 {
		p, e := g.Pull(ctx, r, c.PullRequest)
		if e != nil {
			return nil, "", e
		}
		if p.Base.Ref != c.Branch {
			return nil, "", errors.New("PR base branch mismatch")
		}
		found := p.Head.SHA == c.Commit
		for page := 1; !found; page++ {
			var commits []struct {
				SHA string `json:"sha"`
			}
			if e = g.call(ctx, "GET", fmt.Sprintf("%s/pulls/%d/commits?per_page=100&page=%d", repoPath(r), c.PullRequest, page), token, nil, &commits); e != nil {
				return nil, "", e
			}
			for _, v := range commits {
				found = found || v.SHA == c.Commit
			}
			if len(commits) < 100 {
				break
			}
		}
		if !found {
			return nil, "", errors.New("commit is outside PR")
		}
		base := c.BaselineCommit
		if base == "" {
			var comparison struct {
				MergeBase struct {
					SHA string `json:"sha"`
				} `json:"merge_base_commit"`
			}
			if e = g.call(ctx, "GET", repoPath(r)+"/compare/"+p.Base.SHA+"..."+c.Commit, token, nil, &comparison); e != nil {
				return nil, "", e
			}
			base = comparison.MergeBase.SHA
		}
		return nil, base, nil
	}
	branch, e := g.branchUntil(ctx, r, c.Branch, map[string]bool{c.Commit: true})
	if e != nil {
		return nil, "", e
	}
	if len(branch) == 0 || branch[len(branch)-1] != c.Commit {
		return nil, "", errors.New("commit is outside branch first-parent history")
	}
	// Graph history must survive disconnection independently of baseline candidates.
	// Keep the complete first-parent path, including explicit-baseline uploads.
	parents := []string{}
	for len(commit.Parents) > 0 {
		sha := commit.Parents[0].SHA
		parents = append(parents, sha)
		commit.Parents = nil
		if e = g.call(ctx, "GET", repoPath(r)+"/commits/"+sha, token, nil, &commit); e != nil {
			return nil, "", e
		}
	}
	return parents, c.BaselineCommit, nil
}
func (g *GitHubClient) Comment(ctx context.Context, r *pb.Repository, state *measurement.State, record *measurement.Record) error {
	c := record.Execution.Context
	if c.PullRequest == 0 {
		return nil
	}
	p, e := g.Pull(ctx, r, c.PullRequest)
	if e != nil {
		return e
	}
	if p.Head.SHA != c.Commit {
		return nil
	}
	for _, v := range state.Records {
		if v.Execution.Context.PullRequest == c.PullRequest && v.Execution.Context.Commit == c.Commit && v.Execution.RegistrationOrder > record.Execution.RegistrationOrder {
			return nil
		}
	}
	token, e := g.installation(ctx, r.InstallationId)
	if e != nil {
		return e
	}
	regressions := 0
	for _, v := range state.Comparisons(record, nil, false) {
		if v.Status == pb.ComparisonStatus_COMPARISON_STATUS_REGRESSION {
			regressions++
		}
	}
	marker := "<!-- git-tracker -->"
	body := fmt.Sprintf("%s\n### Git Tracker\n\nState: %s. Regressions: %d.\n\n[Execution details](https://git-tracker.delino.io/?repository=%s&execution=%s)", marker, record.Execution.State.String(), regressions, url.QueryEscape(r.Id), url.QueryEscape(c.ExecutionId))
	body += "\n\n| Dataset / item / column | Value | Change | Result |\n| --- | ---: | ---: | --- |\n"
	clean := func(value string) string {
		return strings.NewReplacer("|", "\\|", "\n", " ", "\r", " ", "<", "&lt;", ">", "&gt;").Replace(value)
	}
	for _, v := range state.Comparisons(record, nil, false) {
		line := fmt.Sprintf("| %s / %s / %s | %s %s | %s | %s |\n", clean(v.DatasetId), clean(v.RowId), clean(v.ColumnId), clean(v.Value), clean(v.Unit), clean(v.Delta), strings.TrimPrefix(v.Status.String(), "COMPARISON_STATUS_"))
		if len(body)+len(line) > 60000 {
			body += "\nMore rows are available in the execution details.\n"
			break
		}
		body += line
	}
	var commentID int64
	for page := 1; ; page++ {
		var comments []struct {
			ID   int64  `json:"id"`
			Body string `json:"body"`
			App  *struct {
				ID int64 `json:"id"`
			} `json:"performed_via_github_app"`
		}
		if e = g.call(ctx, "GET", fmt.Sprintf("%s/issues/%d/comments?per_page=100&page=%d", repoPath(r), c.PullRequest, page), token, nil, &comments); e != nil {
			return e
		}
		for _, v := range comments {
			if v.App != nil && strconv.FormatInt(v.App.ID, 10) == g.AppID && strings.HasPrefix(v.Body, marker) {
				commentID = v.ID
			}
		}
		if len(comments) < 100 {
			break
		}
	}
	method, path := "POST", fmt.Sprintf("%s/issues/%d/comments", repoPath(r), c.PullRequest)
	if commentID != 0 {
		method = "PATCH"
		path = fmt.Sprintf("%s/issues/comments/%d", repoPath(r), commentID)
	}
	return g.call(ctx, method, path, token, map[string]string{"body": body}, nil)
}

func (g *GitHubClient) Branch(ctx context.Context, r *pb.Repository, branch string) ([]string, error) {
	return g.branchUntil(ctx, r, branch, nil)
}
func (g *GitHubClient) branchUntil(ctx context.Context, r *pb.Repository, branch string, needed map[string]bool) ([]string, error) {
	token, e := g.installation(ctx, r.InstallationId)
	if e != nil {
		return nil, e
	}
	var commit struct {
		SHA     string `json:"sha"`
		Parents []struct {
			SHA string `json:"sha"`
		} `json:"parents"`
	}
	if e = g.call(ctx, "GET", repoPath(r)+"/commits/"+url.PathEscape(branch), token, nil, &commit); e != nil {
		return nil, e
	}
	result := []string{commit.SHA}
	delete(needed, commit.SHA)
	for len(commit.Parents) > 0 && len(needed) > 0 {
		sha := commit.Parents[0].SHA
		result = append(result, sha)
		delete(needed, sha)
		if len(needed) == 0 {
			break
		}
		if e = g.call(ctx, "GET", repoPath(r)+"/commits/"+sha, token, nil, &commit); e != nil {
			return nil, e
		}
	}
	return result, nil
}
func (g *GitHubClient) User(ctx context.Context, token string) (string, error) {
	var user struct {
		ID int64 `json:"id"`
	}
	if e := g.call(ctx, "GET", "/user", token, nil, &user); e != nil {
		return "", e
	}
	if user.ID == 0 {
		return "", errors.New("GitHub identity missing")
	}
	return strconv.FormatInt(user.ID, 10), nil
}

func (g *GitHubClient) VerifyMergeHead(ctx context.Context, r *pb.Repository, merge, head string) error {
	if !shaPattern.MatchString(merge) || !shaPattern.MatchString(head) {
		return fault(connect.CodePermissionDenied, "invalid commit")
	}
	token, e := g.installation(ctx, r.InstallationId)
	if e != nil {
		return e
	}
	var commit struct {
		SHA     string `json:"sha"`
		Parents []struct {
			SHA string `json:"sha"`
		} `json:"parents"`
	}
	if e = g.call(ctx, "GET", repoPath(r)+"/commits/"+merge, token, nil, &commit); e != nil {
		return e
	}
	if commit.SHA != merge || len(commit.Parents) != 2 || commit.Parents[1].SHA != head {
		return fault(connect.CodePermissionDenied, "PR merge identity does not match head")
	}
	return nil
}
