// SPDX-License-Identifier: Apache-2.0
package runtime

import (
	"connectrpc.com/connect"
	"context"
	"database/sql"
	"encoding/json"
	"github.com/delinoio/apps/cmds/lane/internal/cli"
	"github.com/delinoio/apps/cmds/lane/internal/store"
	"github.com/delinoio/apps/cmds/lane/internal/syncclient"
	v1 "github.com/delinoio/apps/protos/lane/v1"
	"github.com/delinoio/apps/protos/lane/v1/lanev1connect"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

type vault struct {
	mu     sync.Mutex
	values map[string]string
}

func (v *vault) Get(k string) (string, error) {
	v.mu.Lock()
	defer v.mu.Unlock()
	s, ok := v.values[k]
	if !ok {
		return "", syncclient.ErrCredentials
	}
	return s, nil
}
func (v *vault) Set(k, s string) error { v.mu.Lock(); defer v.mu.Unlock(); v.values[k] = s; return nil }
func (v *vault) Delete(k string) error {
	v.mu.Lock()
	defer v.mu.Unlock()
	delete(v.values, k)
	return nil
}

type accountHandler struct {
	lanev1connect.UnimplementedAccountServiceHandler
	t         *testing.T
	deleted   bool
	challenge string
}

func (h *accountHandler) BeginDataDeletion(ctx context.Context, q *connect.Request[v1.BeginDataDeletionRequest]) (*connect.Response[v1.BeginDataDeletionResponse], error) {
	if q.Header().Get("Authorization") != "Bearer synthetic-token" || q.Msg.ConfirmedAccountId != "account" {
		h.t.Fatal(q.Msg)
	}
	return connect.NewResponse(&v1.BeginDataDeletionResponse{ChallengeId: h.challenge, AuthorizationUrl: "https://approval.example/reauth"}), nil
}
func (h *accountHandler) DeleteAccountData(ctx context.Context, q *connect.Request[v1.DeleteAccountDataRequest]) (*connect.Response[v1.DeleteAccountDataResponse], error) {
	if q.Msg.ReauthProof != "proof" || q.Msg.Mode != v1.DataDeletionMode_DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL {
		h.t.Fatal(q.Msg)
	}
	h.deleted = true
	return connect.NewResponse(&v1.DeleteAccountDataResponse{NewEpoch: store.NewID(), Disconnected: true}), nil
}
func TestBrowserDeletionRequiresSeparateProofAndKeepsLocal(t *testing.T) {
	ctx := context.Background()
	root := t.TempDir()
	local, e := store.Open(root, "")
	if e != nil {
		t.Fatal(e)
	}
	defer local.Close()
	account, e := local.SelectAccount(ctx, "https://issuer.example", "account", "tenant")
	if e != nil {
		t.Fatal(e)
	}
	defer account.Close()
	account.SetSyncEnabled(ctx, true)
	h := &accountHandler{t: t, challenge: store.NewID()}
	p, handler := lanev1connect.NewAccountServiceHandler(h)
	mux := http.NewServeMux()
	mux.Handle(p, handler)
	mux.HandleFunc("/auth/reauth/proof", func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer synthetic-token" {
			t.Error("missing credential")
		}
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte(`{"reauth_proof":"proof"}`))
	})
	server := httptest.NewServer(mux)
	defer server.Close()
	auth := syncclient.Auth{Credentials: &vault{values: map[string]string{}}}
	if e = auth.Save(ctx, account.SpaceID(), account.DeviceID(), syncclient.Credential{RedirectURI: "lane://auth/callback", SessionID: store.NewID(), AccessToken: "synthetic-token", TokenType: "Bearer", Issuer: "https://issuer.example", AccountID: "account", TenantID: "tenant", DeviceID: account.DeviceID(), Endpoint: server.URL, ExpiresAt: time.Now().Add(time.Hour)}); e != nil {
		t.Fatal(e)
	}
	r := &Runtime{Root: root, Auth: auth}
	defer r.Close()
	opt := cli.Options{Remote: true, ExpectedSpace: account.SpaceID()}
	result, e := r.Invoke(ctx, opt, "ClearServer", json.RawMessage(`{"confirmedAccountId":"account"}`))
	if e != nil || h.deleted {
		t.Fatal(result, e, h.deleted)
	}
	callbackResult, e := r.Invoke(ctx, cli.Options{Space: account.SpaceID()}, "SyncCallback", json.RawMessage(`{"callbackUri":"lane://auth/callback?reauth_challenge=`+h.challenge+`"}`))
	encoded, _ := json.Marshal(callbackResult)
	if e != nil || h.deleted || strings.Contains(string(encoded), "proof") || !strings.Contains(string(encoded), h.challenge) {
		t.Fatalf("callback deleted or exposed proof: %s %v", encoded, e)
	}
	wrong := opt
	wrong.ReauthChallenge = store.NewID()
	raw := json.RawMessage(`{"confirmedAccountId":"account","mode":"DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL"}`)
	if _, e = r.Invoke(ctx, wrong, "DeleteAccountData", raw); e == nil || h.deleted {
		t.Fatal("wrong challenge accepted", e)
	}
	opt.ReauthChallenge = h.challenge
	_, e = r.Invoke(ctx, opt, "DeleteAccountData", raw)
	if e != nil || !h.deleted {
		t.Fatal(e, h.deleted)
	}
	id, e := account.Identity(ctx)
	if e != nil || id.SyncEnabled {
		t.Fatal(id, e)
	}
	retained, e := store.Open(root, account.SpaceID())
	if e != nil {
		t.Fatal("retained space missing", e)
	}
	retained.Close()
}

func TestReconnectRequiresNewExplicitConsent(t *testing.T) {
	ctx := context.Background()
	root := t.TempDir()
	local, e := store.Open(root, "")
	if e != nil {
		t.Fatal(e)
	}
	defer local.Close()
	account, e := local.SelectAccount(ctx, "https://issuer.example", "account", "tenant")
	if e != nil {
		t.Fatal(e)
	}
	defer account.Close()
	if e = account.WithWrite(ctx, func(c *sql.Conn) error {
		_, e := c.ExecContext(ctx, "INSERT OR REPLACE INTO store_meta(key,value) VALUES('account_epoch',?),('reconnect_required','1'),('sync_error_reason','ERROR_REASON_STALE_EPOCH')", store.NewID())
		return e
	}); e != nil {
		t.Fatal(e)
	}
	keys := &vault{values: map[string]string{}}
	r := &Runtime{Root: root, Auth: syncclient.Auth{Credentials: keys}}
	defer r.Close()
	_, e = r.Invoke(ctx, cli.Options{RedirectURI: "lane://auth/callback"}, "ConnectSync", json.RawMessage(`{}`))
	if e == nil || len(keys.values) != 0 {
		t.Fatal("ordinary connect started reconnect", e)
	}
	_, e = r.invoke(ctx, cli.Options{RedirectURI: "lane://auth/callback"}, "ConnectSync", json.RawMessage(`{}`), v1.OriginKind_ORIGIN_KIND_MCP)
	if e == nil || len(keys.values) != 0 {
		t.Fatal("untargeted MCP confirmed reconnect", e)
	}
	_, e = r.invoke(ctx, cli.Options{RedirectURI: "lane://auth/callback"}, "ConnectSync", json.RawMessage(`{"accountId":"other"}`), v1.OriginKind_ORIGIN_KIND_MCP)
	if e == nil || len(keys.values) != 0 {
		t.Fatal("wrong account confirmed reconnect", e)
	}
	result, e := r.Invoke(ctx, cli.Options{ConfirmEpochChange: true, RedirectURI: "lane://auth/callback"}, "ConnectSync", json.RawMessage(`{"accountId":"account"}`))
	if e != nil || len(keys.values) != 1 {
		t.Fatal(result, e, len(keys.values))
	}
	identity, e := account.Identity(ctx)
	if e != nil || identity.SyncEnabled {
		t.Fatal("approval start enabled sync before browser completion", identity, e)
	}
	// Abandoning the browser leaves durable data and the reconnect requirement intact.
	result, e = r.Invoke(ctx, cli.Options{}, "GetLocalStatus", json.RawMessage(`{}`))
	if e != nil {
		t.Fatal(e)
	}
	status := result.(syncclient.LocalStatus)
	if !status.ReconnectRequired || status.AccountID != "account" {
		t.Fatal(status)
	}
}

func TestOldTokenDeletionDirectiveRuntime(t *testing.T) {
	for _, scenario := range []string{"delete", "keep", "no-record", "wrong-epoch", "wrong-account", "wrong-device"} {
		t.Run(scenario, func(t *testing.T) {
			ctx := context.Background()
			root := t.TempDir()
			local, e := store.Open(root, "")
			if e != nil {
				t.Fatal(e)
			}
			account, e := local.SelectAccount(ctx, "https://issuer.example", "account", "tenant")
			if e != nil {
				t.Fatal(e)
			}
			local.Close()
			space, device, oldEpoch := account.SpaceID(), account.DeviceID(), store.NewID()
			if e = account.WithWrite(ctx, func(c *sql.Conn) error {
				_, e := c.ExecContext(ctx, "INSERT OR REPLACE INTO store_meta(key,value) VALUES('account_epoch',?)", oldEpoch)
				return e
			}); e != nil {
				t.Fatal(e)
			}
			account.SetSyncEnabled(ctx, true)
			account.Close()
			mux := http.NewServeMux()
			mux.HandleFunc("/auth/deletion-status", func(w http.ResponseWriter, q *http.Request) {
				if q.Header.Get("Authorization") != "Bearer synthetic-token" {
					t.Error("wrong saved bearer")
				}
				if scenario == "no-record" {
					w.WriteHeader(401)
					return
				}
				epoch, acc, dev := oldEpoch, "account", device
				if scenario == "wrong-epoch" {
					epoch = store.NewID()
				}
				if scenario == "wrong-account" {
					acc = "other"
				}
				if scenario == "wrong-device" {
					dev = store.NewID()
				}
				mode := "DATA_DELETION_MODE_DELETE_EVERYWHERE"
				if scenario == "keep" {
					mode = "DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL"
				}
				json.NewEncoder(w).Encode(map[string]any{"mode": mode, "old_epoch": epoch, "new_epoch": store.NewID(), "account_id": acc, "device_id": dev, "disconnected": true})
			})
			mux.HandleFunc("/", func(w http.ResponseWriter, q *http.Request) {
				w.Header().Set("Content-Type", "application/json")
				w.WriteHeader(401)
				w.Write([]byte(`{"code":"unauthenticated","message":"revoked"}`))
			})
			server := httptest.NewServer(mux)
			defer server.Close()
			auth := syncclient.Auth{Credentials: &vault{values: map[string]string{}}}
			if e = auth.Save(ctx, space, device, syncclient.Credential{AccessToken: "synthetic-token", TokenType: "Bearer", Issuer: "https://issuer.example", AccountID: "account", TenantID: "tenant", DeviceID: device, Endpoint: server.URL, ExpiresAt: time.Now().Add(time.Hour)}); e != nil {
				t.Fatal(e)
			}
			r := &Runtime{Root: root, Auth: auth}
			defer r.Close()
			_, e = r.Invoke(ctx, cli.Options{Remote: true, ExpectedSpace: space}, "ListDevices", json.RawMessage(`{}`))
			if e == nil {
				t.Fatal("unauthenticated call reported success")
			}
			if scenario == "delete" {
				if _, e = os.Stat(filepath.Join(root, "spaces", space)); !os.IsNotExist(e) {
					t.Fatal("deletion did not remove files", e)
				}
			} else {
				s, e := store.Open(root, space)
				if e != nil {
					t.Fatal("unmatched directive erased space", e)
				}
				defer s.Close()
				status, e := syncclient.Status(ctx, s, true, true)
				if e != nil {
					t.Fatal(e)
				}
				if status.ReconnectRequired != (scenario == "keep") {
					t.Fatal(status)
				}
			}
		})
	}
}
