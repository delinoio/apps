// SPDX-License-Identifier: Apache-2.0
package runtime

import (
	"bytes"
	"context"
	"encoding/json"
	"github.com/delinoio/apps/cmds/lane/internal/cli"
	"github.com/delinoio/apps/cmds/lane/internal/store"
	v1 "github.com/delinoio/apps/protos/lane/v1"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestLocalCLITransactionReplayDeletion(t *testing.T) {
	root := t.TempDir()
	ctx := context.Background()
	request := store.NewID()
	var diag bytes.Buffer
	call := func(args ...string) (int, string) {
		t.Helper()
		r := &Runtime{Root: root, Diagnostics: &diag}
		defer r.Close()
		var out bytes.Buffer
		a := cli.App{Version: "0.1.0", Out: &out, Err: &diag, In: strings.NewReader(""), Invoke: r.Invoke}
		c := a.Run(ctx, append([]string{"--json"}, args...))
		return c, out.String()
	}
	c, out := call("task", "create", "--repository-url", "https://example.com/team/repo", "--title", "synthetic", "--request-id", request)
	if c != 0 {
		t.Fatal(c, out)
	}
	var response struct {
		Result struct {
			Result struct {
				Version struct {
					ID string `json:"entityId"`
				} `json:"version"`
			} `json:"result"`
		} `json:"result"`
	}
	if e := json.Unmarshal([]byte(out), &response); e != nil {
		t.Fatal(e)
	}
	id := response.Result.Result.Version.ID
	if id == "" {
		t.Fatal(out)
	}
	if c, out = call("task", "create", "--repository-url", "git@example.com:team/repo.git", "--title", "synthetic", "--request-id", request); c != 0 || !strings.Contains(out, "RESULT_KIND_REPLAYED") {
		t.Fatal(c, out)
	}
	if c, out = call("task", "delete", id); c != 0 {
		t.Fatal(c, out)
	}
	if c, out = call("task", "create", "--repository-url", "https://example.com/team/repo", "--title", "synthetic", "--request-id", request); c != 0 || !strings.Contains(out, "RESULT_KIND_DELETED") || strings.Contains(out, "synthetic") {
		t.Fatal(c, out)
	}
}
func TestMCPOriginAndNoStdoutDiagnostics(t *testing.T) {
	r := &Runtime{Root: t.TempDir(), Diagnostics: &bytes.Buffer{}}
	defer r.Close()
	var out bytes.Buffer
	in := strings.NewReader("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"1\"}}}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"lane_create_task\",\"arguments\":{\"repository\":{\"url\":\"https://example.com/r\"},\"title\":\"test\",\"proposal\":\"PROPOSAL_PENDING\"}}}\n")
	if e := r.ServeMCP(context.Background(), cli.Options{}, in, &out); e != nil {
		t.Fatal(e)
	}
	for _, line := range strings.Split(strings.TrimSpace(out.String()), "\n") {
		if !json.Valid([]byte(line)) {
			t.Fatal(line)
		}
	}
	if !strings.Contains(out.String(), "ORIGIN_KIND_MCP") {
		t.Fatal(out.String())
	}
}
func TestRemotePinAndLocalOnlyRejection(t *testing.T) {
	root := t.TempDir()
	s, e := store.Open(root, "")
	if e != nil {
		t.Fatal(e)
	}
	id := s.SpaceID()
	s.Close()
	r := &Runtime{Root: root}
	defer r.Close()
	_, e = r.Invoke(context.Background(), cli.Options{Remote: true, ExpectedSpace: store.NewID()}, "ListDevices", json.RawMessage(`{}`))
	if e == nil || !strings.Contains(e.Error(), "active space changed") {
		t.Fatal(e, id)
	}
	r2 := &Runtime{Root: root}
	defer r2.Close()
	_, e = r2.Invoke(context.Background(), cli.Options{}, "ListDevices", json.RawMessage(`{}`))
	if e == nil || !strings.Contains(e.Error(), "--remote") {
		t.Fatal(e)
	}
}
func TestPinnedLocalSpaceDoesNotFollowAccountSwitch(t *testing.T) {
	root := t.TempDir()
	r := &Runtime{Root: root}
	defer r.Close()
	s, e := r.open(cli.Options{})
	if e != nil {
		t.Fatal(e)
	}
	next, e := s.SelectAccount(context.Background(), "https://issuer.example", "a", "t")
	if e != nil {
		t.Fatal(e)
	}
	defer next.Close()
	v, e := r.Invoke(context.Background(), cli.Options{}, "CreateTask", json.RawMessage(`{"repository":{"url":"https://example.com/r"},"title":"local"}`))
	if e != nil {
		t.Fatal(e)
	}
	if v.(*v1.CreateTaskResponse).Result.Version.Origin.AccountId != "" {
		t.Fatal(v)
	}
}

func TestRemovalRetriesUnavailableSpace(t *testing.T) {
	root := t.TempDir()
	ctx := context.Background()
	held, err := store.Open(root, "")
	if err != nil {
		t.Fatal(err)
	}
	defer held.Close()
	id := held.SpaceID()
	first := &Runtime{Root: root}
	_, err = first.Invoke(ctx, cli.Options{Space: id}, "RemoveDeviceData", json.RawMessage(`{}`))
	first.Close()
	if err == nil {
		t.Fatal("held lifetime lease must block physical removal")
	}
	if _, err = held.Sequence(ctx); err == nil {
		t.Fatal("marked space must reject and drain held handle")
	}
	retry := &Runtime{Root: root}
	_, err = retry.Invoke(ctx, cli.Options{Space: id}, "RemoveDeviceData", json.RawMessage(`{}`))
	retry.Close()
	if err != nil {
		t.Fatal(err)
	}
	if _, err = os.Stat(filepath.Join(root, "spaces", id)); !os.IsNotExist(err) {
		t.Fatalf("space remains: %v", err)
	}
	again := &Runtime{Root: root}
	defer again.Close()
	if _, err = again.Invoke(ctx, cli.Options{Space: id}, "RemoveDeviceData", json.RawMessage(`{}`)); err != nil {
		t.Fatalf("restart after unregister: %v", err)
	}
}
