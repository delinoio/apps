// SPDX-License-Identifier: FSL-1.1-ALv2

package service

import (
	"connectrpc.com/connect"
	"context"
	"crypto/rand"
	"crypto/rsa"
	"encoding/base64"
	"encoding/json"
	"math/big"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/coreos/go-oidc/v3/oidc"
	pb "github.com/delinoio/apps/protos/gittracker/v1"
	"github.com/golang-jwt/jwt/v4"
)

func TestOIDCClaimsAndForkRestrictions(t *testing.T) {
	key, e := rsa.GenerateKey(rand.Reader, 2048)
	if e != nil {
		t.Fatal(e)
	}
	head, merge := strings.Repeat("a", 40), strings.Repeat("b", 40)
	var failurePath atomic.Value
	failurePath.Store("")
	provider := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		if r.URL.Path == failurePath.Load().(string) {
			w.WriteHeader(503)
			return
		}
		switch r.URL.Path {
		case "/jwks":
			json.NewEncoder(w).Encode(map[string]any{"keys": []any{map[string]string{"kty": "RSA", "kid": "test", "use": "sig", "alg": "RS256", "n": base64.RawURLEncoding.EncodeToString(key.N.Bytes()), "e": base64.RawURLEncoding.EncodeToString(big.NewInt(int64(key.E)).Bytes())}}})
		case "/app/installations/1/access_tokens":
			w.Write([]byte(`{"token":"fixture"}`))
		case "/repos/owner/repo/pulls/7":
			w.Write([]byte(`{"head":{"sha":"` + head + `","repo":{"id":99}},"base":{"ref":"main","repo":{"id":42}}}`))
		case "/repos/owner/repo/commits/" + merge:
			w.Write([]byte(`{"sha":"` + merge + `","parents":[{"sha":"` + strings.Repeat("c", 40) + `"},{"sha":"` + head + `"}]}`))
		default:
			w.WriteHeader(404)
		}
	}))
	defer provider.Close()
	verifier := oidc.NewVerifier(provider.URL, oidc.NewRemoteKeySet(context.Background(), provider.URL+"/jwks"), &oidc.Config{ClientID: "tracker"})
	a := &DelcoreAuthority{Actions: verifier, GitHub: &GitHubClient{HTTP: provider.Client(), BaseURL: provider.URL, AppID: "1", Key: key}}
	repo := &pb.Repository{Id: "connection", FullName: "owner/repo", GithubRepositoryId: "42", Connected: true, InstallationId: "1"}
	claims := func() jwt.MapClaims {
		return jwt.MapClaims{"iss": provider.URL, "aud": "tracker", "sub": "repo:owner/repo:pull_request", "exp": time.Now().Add(time.Hour).Unix(), "repository_id": "42", "run_id": "123", "run_attempt": "1", "sha": merge, "ref": "refs/pull/7/merge", "event_name": "pull_request"}
	}
	sign := func(c jwt.MapClaims) string {
		token := jwt.NewWithClaims(jwt.SigningMethodRS256, c)
		token.Header["kid"] = "test"
		raw, e := token.SignedString(key)
		if e != nil {
			t.Fatal(e)
		}
		return raw
	}
	contextFor := func() *pb.Context {
		return &pb.Context{RepositoryId: "42", ExecutionId: "42/123/1", Commit: head, Branch: "main", PullRequest: 7}
	}
	if e = a.VerifyActions(context.Background(), sign(claims()), repo, contextFor()); e != nil {
		t.Fatal(e)
	}
	for _, path := range []string{"/repos/owner/repo/pulls/7", "/repos/owner/repo/commits/" + merge} {
		failurePath.Store(path)
		if err := a.VerifyActions(context.Background(), sign(claims()), repo, contextFor()); connect.CodeOf(err) != connect.CodeUnavailable {
			t.Fatal("provider failure lost", err)
		}
	}
	failurePath.Store("")
	for name, change := range map[string]func(jwt.MapClaims){"audience": func(c jwt.MapClaims) { c["aud"] = "other" }, "issuer": func(c jwt.MapClaims) { c["iss"] = "https://other.test" }, "expired": func(c jwt.MapClaims) { c["exp"] = time.Now().Add(-time.Hour).Unix() }, "repository": func(c jwt.MapClaims) { c["repository_id"] = "666" }, "run": func(c jwt.MapClaims) { c["run_id"] = "124" }, "event": func(c jwt.MapClaims) { c["event_name"] = "pull_request_target" }, "ref": func(c jwt.MapClaims) { c["ref"] = "refs/pull/8/merge" }} {
		t.Run(name, func(t *testing.T) {
			c := claims()
			change(c)
			if e := a.VerifyActions(context.Background(), sign(c), repo, contextFor()); e == nil {
				t.Fatal("accepted invalid identity")
			}
		})
	}
	c := contextFor()
	c.BaselineCommit = strings.Repeat("d", 40)
	if e = a.VerifyActions(context.Background(), sign(claims()), repo, c); e == nil {
		t.Fatal("fork baseline override")
	}
	c = contextFor()
	c.PullRequest = 0
	if e = a.VerifyActions(context.Background(), sign(claims()), repo, c); e == nil {
		t.Fatal("fork push write")
	}
}
