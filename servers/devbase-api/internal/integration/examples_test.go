// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package integration

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// exampleServer serves the API on a loopback port with the mocked Delcore.
func (f *fixture) exampleServer(t *testing.T) string {
	t.Helper()
	path, handler := f.server.Handler()
	mux := http.NewServeMux()
	mux.Handle(path, handler)
	ts := httptest.NewServer(mux)
	t.Cleanup(ts.Close)
	return ts.URL
}

func repositoryRoot(t *testing.T) string {
	t.Helper()
	root, e := filepath.Abs("../../../..")
	if e != nil {
		t.Fatal(e)
	}
	return root
}

// TestConsumerExamples executes the Go and TypeScript examples against a local
// server with a mocked Delcore. They must leave no project, key or user behind.
func TestConsumerExamples(t *testing.T) {
	root := repositoryRoot(t)
	for _, example := range []struct {
		name    string
		command []string
		dir     string
		need    string
	}{
		{"go", []string{"go", "run", "./examples/devbase-client"}, root, "go"},
		{"typescript", []string{"node", "--experimental-transform-types", "main.ts"}, filepath.Join(root, "examples/devbase-client-ts"), "node"},
	} {
		t.Run(example.name, func(t *testing.T) {
			if _, e := exec.LookPath(example.need); e != nil {
				t.Skip(example.need + " is not installed")
			}
			if example.name == "typescript" {
				if _, e := os.Stat(filepath.Join(example.dir, "node_modules")); e != nil {
					t.Skip("run pnpm install to prepare the TypeScript example")
				}
			}
			f := setup(t)
			tenant, owner, _ := f.tenant()
			url := f.exampleServer(t)
			ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
			defer cancel()
			cmd := exec.CommandContext(ctx, example.command[0], example.command[1:]...)
			cmd.Dir = example.dir
			cmd.Env = append(os.Environ(), "DEVBASE_URL="+url, "DEVBASE_TOKEN="+login(owner), "DEVBASE_TENANT_ID="+tenant, "GOWORK=off")
			output, e := cmd.CombinedOutput()
			if e != nil {
				t.Fatalf("%v\n%s", e, output)
			}
			text := string(output)
			for _, step := range []string{"Created project", "Issued CRUD key", "Registered end user", "Updated end user to version 2", "Listed 1 end user(s)", "Deleted the end user", "rejected as unauthenticated"} {
				if !strings.Contains(text, step) {
					t.Errorf("missing %q in output:\n%s", step, text)
				}
			}
			if strings.Contains(text, "dbk_") {
				t.Error("the example printed a key secret")
			}
			for _, table := range []string{"projects", "api_keys", "end_users"} {
				if n := f.count("SELECT count(*) FROM " + table); n != 0 {
					t.Errorf("%s retains %d rows after the example", table, n)
				}
			}
		})
	}
}

// TestPublishedJSONRequests sends the plain-JSON requests shown in the public guide
// and checks that the documented field names and enum spelling work.
func TestPublishedJSONRequests(t *testing.T) {
	f := setup(t)
	tenant, owner, _ := f.tenant()
	url := f.exampleServer(t)
	post := func(procedure, token, body string) (int, string) {
		t.Helper()
		req, e := http.NewRequest(http.MethodPost, url+"/devbase.v1.DevBaseService/"+procedure, strings.NewReader(body))
		if e != nil {
			t.Fatal(e)
		}
		req.Header.Set("Content-Type", "application/json")
		req.Header.Set("Authorization", "Bearer "+token)
		resp, e := http.DefaultClient.Do(req)
		if e != nil {
			t.Fatal(e)
		}
		defer resp.Body.Close()
		raw, _ := io.ReadAll(resp.Body)
		return resp.StatusCode, string(raw)
	}
	field := func(body, name string) string {
		var parsed map[string]any
		if e := json.Unmarshal([]byte(body), &parsed); e != nil {
			// The body can hold a one-time key secret, so never print it.
			t.Fatal("the response is not a JSON object:", e)
		}
		return fmt.Sprint(find(parsed, name))
	}
	status, body := post("CreateProject", login(owner), `{"idempotencyKey":"3f1f6d1e-5a0a-4d7b-9a53-0c1d9f7a7d01","tenantId":"`+tenant+`","name":"My app"}`)
	if status != 200 || field(body, "version") != "1" {
		t.Fatal(status, body)
	}
	project := field(body, "id")
	status, body = post("IssueApiKey", login(owner), `{"idempotencyKey":"8c2b7a60-91a0-4a53-a9c3-6f7f0b2f1e02","projectId":"`+project+`","permission":"API_KEY_PERMISSION_CRUD"}`)
	// The response carries the one-time key secret, so a failure reports only the status and
	// whether the secret has the documented prefix, never the body.
	if status != 200 || !strings.HasPrefix(field(body, "secret"), "dbk_") {
		t.Fatalf("issuing a key returned status %d or a secret without the dbk_ prefix", status)
	}
	secret := field(body, "secret")
	status, body = post("CreateEndUser", secret, `{"idempotencyKey":"d5f0c3a2-2b1e-4f59-8f0a-5b7c9e4d3a03","projectId":"`+project+`","externalId":"user-1001","email":"ada@example.com","metadata":{"plan":"pro"}}`)
	if status != 200 || field(body, "externalId") != "user-1001" {
		t.Fatal(status, body)
	}
	// Errors carry the documented Connect code.
	status, body = post("CreateEndUser", secret, `{"idempotencyKey":"d5f0c3a2-2b1e-4f59-8f0a-5b7c9e4d3a04","projectId":"`+project+`","externalId":"user-1001"}`)
	if status != http.StatusConflict || field(body, "code") != "already_exists" {
		t.Fatal(status, body)
	}
}

// find returns the first value stored under name in nested JSON.
func find(value any, name string) any {
	switch v := value.(type) {
	case map[string]any:
		if found, ok := v[name]; ok {
			return found
		}
		for _, child := range v {
			if found := find(child, name); found != nil {
				return found
			}
		}
	}
	return nil
}
