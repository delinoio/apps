// SPDX-License-Identifier: Apache-2.0
package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"strings"
	"testing"
)

func run(t *testing.T, args []string, fn Invoke) (int, string, string) {
	t.Helper()
	var out, diag bytes.Buffer
	a := App{Version: "0.1.0", In: strings.NewReader("prompt\n"), Out: &out, Err: &diag, Invoke: fn}
	code := a.Run(context.Background(), args)
	return code, out.String(), diag.String()
}
func TestVersionAndHelpDoNotOpenStore(t *testing.T) {
	for _, arg := range []string{"--version", "--help"} {
		c, o, e := run(t, []string{arg}, nil)
		if c != 0 || e != "" || !strings.Contains(o, "lane") {
			t.Fatalf("%d %s %s", c, o, e)
		}
	}
}
func TestInvalidInvocation(t *testing.T) {
	for _, args := range [][]string{{"--json", "--remote", "--space", "x", "task", "list"}, {"--json", "task", "list", "--bogus", "x"}, {"--json", "rpc", "Bogus"}, {"--json", "task", "create", "--progress", "done,canceled"}, {"--json", "task", "edit", "id"}} {
		c, o, _ := run(t, args, nil)
		if c != 2 || !strings.Contains(o, "ERROR_REASON_INVALID_INPUT") {
			t.Fatalf("%v %d %s", args, c, o)
		}
	}
}
func TestCreateAndStdin(t *testing.T) {
	c, o, _ := run(t, []string{"--json", "task", "create", "--repository-url", "https://example.com/r", "--title", "Task", "--prompt-stdin"}, func(ctx context.Context, opt Options, m string, r json.RawMessage) (any, error) {
		if opt.Remote || m != "CreateTask" {
			t.Fatal(m)
		}
		var b map[string]any
		json.Unmarshal(r, &b)
		if b["prompt"] != "prompt\n" || b["repository"].(map[string]any)["url"] != "https://example.com/r" {
			t.Fatal(b)
		}
		return map[string]any{"id": "created"}, nil
	})
	if c != 0 || !strings.Contains(o, `"ok":true`) {
		t.Fatalf("%d %s", c, o)
	}
}
func TestEditPreservesFullTaskAndExpectedVersion(t *testing.T) {
	calls := 0
	c, _, _ := run(t, []string{"task", "edit", "id", "--expected-version", "old", "--title", "new"}, func(ctx context.Context, opt Options, m string, r json.RawMessage) (any, error) {
		calls++
		if m == "GetTask" {
			return map[string]any{"result": map[string]any{"version": map[string]any{"task": map[string]any{"title": "before", "prompt": "preserved", "repositoryId": "repo", "progress": "PROGRESS_BLOCKED", "proposal": "PROPOSAL_PENDING"}}}}, nil
		}
		var b map[string]any
		json.Unmarshal(r, &b)
		task := b["task"].(map[string]any)
		if m != "SaveTask" || b["expectedVersionId"] != "old" || task["title"] != "new" || task["prompt"] != "preserved" || task["progress"] != "PROGRESS_BLOCKED" {
			t.Fatal(b)
		}
		return map[string]any{}, nil
	})
	if c != 0 || calls != 2 {
		t.Fatal(c, calls)
	}
}
func TestFailureDoesNotLeakDetails(t *testing.T) {
	c, o, e := run(t, []string{"--json", "task", "list"}, func(context.Context, Options, string, json.RawMessage) (any, error) {
		return nil, errors.New("secret prompt SQL path /private/data")
	})
	if c != 1 || strings.Contains(o, "secret") || e != "" {
		t.Fatal(c, o, e)
	}
}
func TestRemoteRPCPin(t *testing.T) {
	c, _, _ := run(t, []string{"--remote", "--expected-space", "pinned", "--json", "rpc", "ListDevices", "--request-json", "{}"}, func(ctx context.Context, opt Options, m string, r json.RawMessage) (any, error) {
		if !opt.Remote || opt.ExpectedSpace != "pinned" || m != "ListDevices" {
			t.Fatal(opt, m)
		}
		return map[string]any{}, nil
	})
	if c != 0 {
		t.Fatal(c)
	}
}

func TestPrivateChallengeFlagsStayOutsideWire(t *testing.T) {
	code, _, err := run(t, []string{"--remote", "--json", "--expected-space", "space", "--reauth-challenge", "challenge", "rpc", "DeleteAccountData", "--request-json", `{"confirmedAccountId":"account","mode":"DATA_DELETION_MODE_DELETE_EVERYWHERE"}`}, func(ctx context.Context, opt Options, method string, raw json.RawMessage) (any, error) {
		if opt.ReauthChallenge != "challenge" || method != "DeleteAccountData" || strings.Contains(string(raw), "challenge") || strings.Contains(string(raw), "proof") {
			t.Fatal(opt, method, string(raw))
		}
		return map[string]any{"completed": true}, nil
	})
	if code != 0 {
		t.Fatal(code, err)
	}
	code, _, err = run(t, []string{"--json", "--space", "space", "sync", "cancel-reauth", "--challenge-id", "challenge"}, func(ctx context.Context, opt Options, method string, raw json.RawMessage) (any, error) {
		if method != "CancelReauthentication" || !strings.Contains(string(raw), `"challengeId":"challenge"`) {
			t.Fatal(method, string(raw))
		}
		return map[string]any{"completed": true}, nil
	})
	if code != 0 {
		t.Fatal(code, err)
	}
}
