// SPDX-License-Identifier: FSL-1.1-ALv2

package service

import (
	"context"
	"crypto/rand"
	"crypto/rsa"
	"encoding/json"
	pb "github.com/delinoio/apps/protos/gittracker/v1"
	"github.com/delinoio/apps/servers/git-tracker-api/internal/measurement"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

func TestStalePRCommentsAndMergeBase(t *testing.T) {
	key, _ := rsa.GenerateKey(rand.Reader, 2048)
	head, base := strings.Repeat("a", 40), strings.Repeat("b", 40)
	var writes atomic.Int64
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		switch r.URL.Path {
		case "/app/installations/1/access_tokens":
			w.Write([]byte(`{"token":"fixture"}`))
		case "/repos/owner/repo/pulls/7":
			w.Write([]byte(`{"head":{"sha":"` + head + `"},"base":{"sha":"` + base + `","ref":"main"}}`))
		case "/repos/owner/repo/commits/" + head:
			w.Write([]byte(`{"sha":"` + head + `"}`))
		case "/repos/owner/repo/compare/" + base + "..." + head:
			w.Write([]byte(`{"merge_base_commit":{"sha":"` + base + `"}}`))
		case "/repos/owner/repo/issues/7/comments":
			if r.Method == "GET" {
				w.Write([]byte(`[]`))
			} else {
				writes.Add(1)
				w.Write([]byte(`{}`))
			}
		default:
			w.WriteHeader(404)
		}
	}))
	defer server.Close()
	g := &GitHubClient{HTTP: server.Client(), BaseURL: server.URL, AppID: "1", Key: key}
	repo := &pb.Repository{Id: "connection", FullName: "owner/repo", GithubRepositoryId: "42", InstallationId: "1"}
	old := &pb.Context{ExecutionId: "old", Commit: head, Branch: "main", PullRequest: 7}
	_, resolved, e := g.Resolve(context.Background(), repo, old)
	if e != nil || resolved != base {
		t.Fatal("merge base not resolved", e)
	}
	state := measurement.NewState()
	r := state.Register(old, time.Now(), nil, base)
	newer := state.Register(&pb.Context{ExecutionId: "new", Commit: head, Branch: "main", PullRequest: 7}, time.Now(), nil, base)
	if e = g.Comment(context.Background(), repo, state, r); e != nil || writes.Load() != 0 {
		t.Fatal("old execution published", e)
	}
	if e = g.Comment(context.Background(), repo, state, newer); e != nil || writes.Load() != 1 {
		t.Fatal("latest execution did not publish", e)
	}
	newer.Execution.Context.Commit = base
	if e = g.Comment(context.Background(), repo, state, newer); e != nil || writes.Load() != 1 {
		t.Fatal("old commit published", e)
	}
}
func TestVisibilityFailureRetainsStateThenSynchronizes(t *testing.T) {
	s := database(t)
	ctx := context.Background()
	key, _ := rsa.GenerateKey(rand.Reader, 2048)
	var fail atomic.Bool
	fail.Store(true)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		if strings.HasSuffix(r.URL.Path, "access_tokens") {
			w.Write([]byte(`{"token":"fixture"}`))
			return
		}
		if fail.Load() {
			w.WriteHeader(503)
			return
		}
		json.NewEncoder(w).Encode(map[string]any{"id": 42, "full_name": "owner/repo", "private": true})
	}))
	defer server.Close()
	s.GitHub = &GitHubClient{HTTP: server.Client(), BaseURL: server.URL, AppID: "1", Key: key}
	s.DB.Exec(ctx, "UPDATE git_tracker_repositories SET public=true")
	if e := s.Maintain(ctx); e != nil {
		t.Fatal(e)
	}
	var public bool
	s.DB.QueryRow(ctx, "SELECT public FROM git_tracker_repositories WHERE id=$1", fixtureRepositoryID).Scan(&public)
	if !public {
		t.Fatal("failed visibility lookup changed last confirmed state")
	}
	fail.Store(false)
	if e := s.Maintain(ctx); e != nil {
		t.Fatal(e)
	}
	s.DB.QueryRow(ctx, "SELECT public FROM git_tracker_repositories WHERE id=$1", fixtureRepositoryID).Scan(&public)
	if public {
		t.Fatal("confirmed private visibility not applied")
	}
}

func TestGitHubRepositoryPaths(t *testing.T) {
	r := &pb.Repository{GithubRepositoryId: "42", FullName: "owner/repo"}
	if got := repoPath(r); got != "/repos/owner/repo" {
		t.Fatal(got)
	}
	if got := repositoryIDPath(r); got != "/repositories/42" {
		t.Fatal(got)
	}
}

func TestPushCommitBelongsToBranch(t *testing.T) {
	key, _ := rsa.GenerateKey(rand.Reader, 2048)
	tip, old, side := strings.Repeat("a", 40), strings.Repeat("b", 40), strings.Repeat("c", 40)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/app/installations/1/access_tokens":
			w.Write([]byte(`{"token":"fixture"}`))
		case "/repos/owner/repo/commits/main", "/repos/owner/repo/commits/" + tip:
			json.NewEncoder(w).Encode(map[string]any{"sha": tip, "parents": []map[string]string{{"sha": old}, {"sha": side}}})
		case "/repos/owner/repo/commits/" + old:
			json.NewEncoder(w).Encode(map[string]any{"sha": old, "parents": []any{}})
		case "/repos/owner/repo/commits/" + side:
			json.NewEncoder(w).Encode(map[string]any{"sha": side, "parents": []any{}})
		default:
			w.WriteHeader(404)
		}
	}))
	defer server.Close()
	g := &GitHubClient{HTTP: server.Client(), BaseURL: server.URL, AppID: "1", Key: key}
	repo := &pb.Repository{FullName: "owner/repo", InstallationId: "1"}
	for _, baseline := range []string{"", old} {
		ancestors, _, err := g.resolveRows(context.Background(), repo, &pb.Context{Commit: tip, Branch: "main", BaselineCommit: baseline}, nil)
		if err != nil || len(ancestors) != 1 || ancestors[0] != old {
			t.Fatalf("graph ancestry missing: %v %v", ancestors, err)
		}
	}
	for _, sha := range []string{tip, old, side} {
		_, _, err := g.Resolve(context.Background(), repo, &pb.Context{Commit: sha, Branch: "main", BaselineCommit: old})
		if (err != nil) != (sha == side) {
			t.Fatalf("commit %s: %v", sha, err)
		}
	}
}
