// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package integration

import (
	"context"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"sync"
	"testing"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/devbase/v1"
	"github.com/delinoio/apps/protos/devbase/v1/devbasev1connect"
	"github.com/delinoio/apps/servers/devbase-api/internal/api"
	"github.com/delinoio/apps/servers/devbase-api/internal/delcore"
)

// hookedAuthority runs a hook once at the start of the next tenant-state read. Authorization
// reads the state after it resolved the project and before the operation runs, so the hook can
// delete the project in exactly that window.
type hookedAuthority struct {
	delcore.Authority
	mu     sync.Mutex
	before func()
}

func (h *hookedAuthority) arm(before func()) { h.mu.Lock(); h.before = before; h.mu.Unlock() }

func (h *hookedAuthority) TenantStates(ctx context.Context, tenants []string) (map[string]delcore.State, error) {
	h.mu.Lock()
	hook := h.before
	h.before = nil
	h.mu.Unlock()
	if hook != nil {
		hook()
	}
	return h.Authority.TenantStates(ctx, tenants)
}

// A project deleted after authorization but before the query cascades away every user. The
// empty page must not look like a live empty directory, for login tokens and keys alike.
func TestListEndUsersReportsAProjectDeletedAfterAuthorization(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	hook := &hookedAuthority{Authority: f.fake}
	server := api.New(f.db.Pool, hook, f.limits, slog.New(slog.NewTextHandler(logSink(), nil)), f.metrics, f.clock.now)
	path, handler := server.Handler()
	mux := http.NewServeMux()
	mux.Handle(path, handler)
	ts := httptest.NewServer(mux)
	t.Cleanup(ts.Close)
	client := devbasev1connect.NewDevBaseServiceClient(ts.Client(), ts.URL)

	tenant, owner, _ := f.tenant()
	empty := f.project(login(owner), tenant, "Empty")
	if list, e := client.ListEndUsers(ctx, as(login(owner), &pb.ListEndUsersRequest{ProjectId: empty.Id})); e != nil || len(list.Msg.EndUsers) != 0 {
		t.Fatal("a live empty project must list successfully", e)
	}
	deleteProject := func(id string) func() {
		return func() {
			if _, e := f.db.Pool.Exec(ctx, "DELETE FROM projects WHERE id=$1", id); e != nil {
				t.Error(e)
			}
		}
	}

	byLogin := f.project(login(owner), tenant, "ByLogin")
	f.user(login(owner), byLogin.Id, "kept")
	hook.arm(deleteProject(byLogin.Id))
	if _, e := client.ListEndUsers(ctx, as(login(owner), &pb.ListEndUsersRequest{ProjectId: byLogin.Id})); code(e) != connect.CodeNotFound {
		t.Fatal("a login request listed a project deleted after authorization as empty", e)
	}

	byKey := f.project(login(owner), tenant, "ByKey")
	f.user(login(owner), byKey.Id, "kept")
	secret, _ := f.key(login(owner), byKey.Id, readOnly)
	hook.arm(deleteProject(byKey.Id))
	if _, e := client.ListEndUsers(ctx, as(secret, &pb.ListEndUsersRequest{ProjectId: byKey.Id})); code(e) != connect.CodeNotFound {
		t.Fatal("a key request listed a project deleted after authorization as empty", e)
	}
}
