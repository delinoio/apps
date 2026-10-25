// SPDX-License-Identifier: Apache-2.0
package runtime

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"sync"
	"time"

	"github.com/delinoio/apps/cmds/lane/internal/api"
	"github.com/delinoio/apps/cmds/lane/internal/cli"
	"github.com/delinoio/apps/cmds/lane/internal/locallog"
	"github.com/delinoio/apps/cmds/lane/internal/mcp"
	"github.com/delinoio/apps/cmds/lane/internal/store"
	"github.com/delinoio/apps/cmds/lane/internal/syncclient"
	v1 "github.com/delinoio/apps/protos/lane/v1"
	"github.com/delinoio/apps/protos/lane/v1/lanev1connect"
	"google.golang.org/protobuf/proto"
)

const DefaultEndpoint = "https://lane.api.delino.io"

// Root is injectable only by embedding tests; the executable has no data-root override.
type Runtime struct {
	Root        string
	Diagnostics io.Writer
	Auth        syncclient.Auth
	once        sync.Once
	db          *store.Store
	openErr     error
	closeOnce   sync.Once
}

func (r *Runtime) Close() {
	r.closeOnce.Do(func() {
		if r.db != nil {
			_ = r.db.Close()
		}
	})
}
func (r *Runtime) open(opt cli.Options) (*store.Store, error) {
	r.once.Do(func() {
		root := r.Root
		if root == "" {
			root, r.openErr = store.DefaultRoot()
			if r.openErr != nil {
				return
			}
		}
		r.db, r.openErr = store.Open(root, opt.Space)
	})
	return r.db, r.openErr
}
func fail(reason, message string) error {
	return &api.Failure{Reason: "ERROR_REASON_" + reason, Message: message, CorrelationID: store.NewID()}
}
func (r *Runtime) Invoke(ctx context.Context, opt cli.Options, method string, raw json.RawMessage) (any, error) {
	return r.invoke(ctx, opt, method, raw, v1.OriginKind_ORIGIN_KIND_CLI)
}
func (r *Runtime) ServeMCP(ctx context.Context, opt cli.Options, in io.Reader, out io.Writer) error {
	return mcp.Serve(ctx, in, out, func(ctx context.Context, method string, raw json.RawMessage) (any, error) {
		result, e := r.invoke(ctx, opt, method, raw, v1.OriginKind_ORIGIN_KIND_MCP)
		if e != nil {
			return nil, api.Error(e)
		}
		return result, nil
	})
}
func (r *Runtime) invoke(ctx context.Context, opt cli.Options, method string, raw json.RawMessage, kind v1.OriginKind) (result any, err error) {
	s, e := r.open(opt)
	if e != nil {
		return nil, e
	}
	defer func() {
		err = classify(err)
		reason := "APPLIED"
		correlation := store.NewID()
		if err != nil {
			f := api.Error(err)
			reason = f.Reason
			correlation = f.CorrelationID
		}
		if e := locallog.Record(ctx, s.Root(), method, reason, correlation); e != nil && r.Diagnostics != nil {
			fmt.Fprintln(r.Diagnostics, "Local diagnostic log could not be retained.")
		}
	}()
	if opt.ExpectedSpace != "" {
		active, e := store.Open(s.Root(), "")
		if e != nil {
			return nil, e
		}
		id := active.SpaceID()
		active.Close()
		if id != opt.ExpectedSpace || s.SpaceID() != opt.ExpectedSpace {
			return nil, fail("FORBIDDEN", "The active space changed. Reopen the selected account and retry.")
		}
	}
	identity, e := s.Identity(ctx)
	if e != nil {
		return nil, e
	}
	if opt.Remote {
		switch method {
		case "GetLocalStatus", "GetNotificationPreferences", "SetNotificationPreferences", "ConnectSync", "DisconnectSync", "RemoveDeviceData", "SyncRun", "SyncCallback":
			return nil, fail("INVALID_INPUT", "This command requires the local target.")
		}
	}
	var local map[string]any
	if json.Unmarshal(raw, &local) != nil {
		return nil, fail("INVALID_INPUT", "Expected a JSON object.")
	}
	switch method {
	case "GetLocalStatus":
		connected := false
		if identity.SyncEnabled && identity.AccountID != "" {
			_, credentialError := r.Auth.Load(ctx, s.SpaceID(), s.DeviceID(), identity.Issuer, identity.AccountID, identity.TenantID)
			connected = credentialError == nil
		}
		status, e := syncclient.Status(ctx, s, identity.SyncEnabled, connected)
		if e != nil {
			return nil, e
		}
		b, e := s.InvokeJSON(ctx, "GetNotificationPreferences", []byte("{}"))
		if e != nil {
			return nil, e
		}
		if e = json.Unmarshal(b, &status.NotificationPreferences); e != nil {
			return nil, e
		}
		return status, nil
	case "GetNotificationPreferences", "SetNotificationPreferences":
		b, e := s.InvokeJSON(ctx, method, raw)
		return json.RawMessage(b), e
	case "ConnectSync":
		endpoint := opt.Endpoint
		if endpoint == "" {
			endpoint = DefaultEndpoint
		}
		redirect := opt.RedirectURI
		if redirect == "" {
			redirect = "lane://auth/callback"
		}
		return r.Auth.Begin(ctx, s.SpaceID(), s.DeviceID(), endpoint, redirect)
	case "SyncCallback":
		callback, _ := local["callbackUri"].(string)
		cred, e := r.Auth.Complete(ctx, s.SpaceID(), callback)
		if e != nil {
			return nil, e
		}
		if cred.DeviceID != s.DeviceID() {
			return nil, fail("FORBIDDEN", "Approved device does not match this installation.")
		}
		target, e := s.SelectAccount(ctx, cred.Issuer, cred.AccountID, cred.TenantID)
		if e != nil {
			return nil, e
		}
		defer target.Close()
		if e = target.SetSyncEnabled(ctx, false); e != nil {
			return nil, e
		}
		if e = r.Auth.Save(ctx, target.SpaceID(), target.DeviceID(), cred); e != nil {
			return nil, e
		}
		if identity.AccountID == "" && s.SpaceID() != target.SpaceID() {
			if e = syncclient.ImportStandalone(ctx, s, target); e != nil {
				return nil, e
			}
		}
		if e = target.SetSyncEnabled(ctx, true); e != nil {
			return nil, e
		}
		return map[string]any{"spaceId": target.SpaceID(), "deviceId": target.DeviceID(), "connected": true}, nil
	case "DisconnectSync":
		retention, _ := local["retention"].(string)
		if retention != "LOCAL_RETENTION_KEEP_LOCAL" && retention != "LOCAL_RETENTION_REMOVE_LOCAL" {
			return nil, fail("INVALID_INPUT", "Choose --retention keep-local or remove-local.")
		}
		// Stop local workers first; failed revocation is reported, never silently successful.
		if e = s.SetSyncEnabled(ctx, false); e != nil {
			return nil, e
		}
		if identity.AccountID != "" {
			cred, e := r.Auth.Load(ctx, s.SpaceID(), s.DeviceID(), identity.Issuer, identity.AccountID, identity.TenantID)
			if e != nil {
				return nil, e
			}
			if e = r.Auth.Revoke(ctx, cred); e != nil {
				return nil, e
			}
			if e = r.Auth.Delete(ctx, s.SpaceID(), s.DeviceID(), identity.Issuer, identity.AccountID, identity.TenantID); e != nil {
				return nil, e
			}
		}
		if retention == "LOCAL_RETENTION_REMOVE_LOCAL" {
			if e = s.RemoveSpace(ctx); e != nil {
				return nil, e
			}
		}
		return map[string]any{"completed": true}, nil
	case "RemoveDeviceData":
		if e = s.SetSyncEnabled(ctx, false); e != nil {
			return nil, e
		}
		if identity.AccountID != "" {
			if e = r.Auth.Delete(ctx, s.SpaceID(), s.DeviceID(), identity.Issuer, identity.AccountID, identity.TenantID); e != nil {
				return nil, e
			}
		}
		if e = s.RemoveSpace(ctx); e != nil {
			return nil, e
		}
		return map[string]any{"completed": true}, nil
	case "SyncRun":
		if !identity.SyncEnabled {
			return nil, fail("ONLINE_REQUIRED", "Run lane sync connect and approve the connection first.")
		}
		client, endpoint, e := r.client(ctx, s, identity, opt.Endpoint)
		if e != nil {
			return nil, e
		}
		w, e := syncclient.NewWorker(s, lanev1connect.NewSyncServiceClient(client, endpoint), syncclient.Options{AccountID: identity.AccountID, TenantID: identity.TenantID, CheckEnabled: func(ctx context.Context) error {
			current, e := s.Identity(ctx)
			if e != nil {
				return e
			}
			if current != identity || !current.SyncEnabled {
				return fail("FORBIDDEN", "Synchronization was disabled or the account changed.")
			}
			return nil
		}})
		if e != nil {
			return nil, e
		}
		if e = w.Run(ctx, opt.Once); e != nil {
			return nil, e
		}
		return map[string]any{"completed": true}, nil
	case "ConflictList", "ConflictGet":
		return r.conflicts(ctx, opt, method, local, kind)
	case "DeleteEverywhere", "ClearServer":
		return r.deleteAccount(ctx, opt, method, local, kind)
	}
	request, e := api.Decode(method, raw)
	if e != nil {
		return nil, fail("INVALID_INPUT", "Invalid typed request.")
	}
	ids, e := api.Attribute(request, &v1.Origin{Kind: kind, DeviceId: s.DeviceID(), DeviceName: "Lane", AccountId: identity.AccountID})
	if e != nil {
		return nil, fail("INVALID_INPUT", e.Error())
	}
	if len(ids) > 0 && r.Diagnostics != nil && kind == v1.OriginKind_ORIGIN_KIND_CLI {
		for _, id := range ids {
			fmt.Fprintln(r.Diagnostics, "Request ID:", id)
		}
	}
	var response proto.Message
	if opt.Remote {
		client, endpoint, e := r.client(ctx, s, identity, opt.Endpoint)
		if e != nil {
			return nil, e
		}
		response, e = api.Remote(client, endpoint)(ctx, method, request)
		if e != nil {
			return nil, e
		}
	} else {
		d := api.Descriptor(method)
		if d.Parent().Name() != "LaneService" && method != "GetEvent" && method != "ListEvents" {
			return nil, fail("ONLINE_REQUIRED", "Use --remote after browser-approved login for this command.")
		}
		response, e = s.Invoke(ctx, method, request)
		if e != nil {
			return nil, e
		}
	}
	return response, nil
}
func (r *Runtime) client(ctx context.Context, s *store.Store, id store.AccountIdentity, override string) (*http.Client, string, error) {
	if id.AccountID == "" {
		return nil, "", fail("UNAUTHENTICATED", "Run lane login and approve the account in your browser.")
	}
	cred, e := r.Auth.Load(ctx, s.SpaceID(), s.DeviceID(), id.Issuer, id.AccountID, id.TenantID)
	if e != nil {
		return nil, "", e
	}
	if override != "" && override != cred.Endpoint {
		return nil, "", fail("FORBIDDEN", "Endpoint differs from the approved connection. Reconnect explicitly.")
	}
	// Token refresh is checked for every HTTP request in a long-lived worker.
	transport := credentialTransport{r: r, s: s, id: id}
	return &http.Client{Transport: transport, Timeout: 45 * time.Second, CheckRedirect: func(*http.Request, []*http.Request) error { return errors.New("redirect refused") }}, cred.Endpoint, nil
}

type credentialTransport struct {
	r  *Runtime
	s  *store.Store
	id store.AccountIdentity
}

func (t credentialTransport) RoundTrip(q *http.Request) (*http.Response, error) {
	token, e := t.r.Auth.Token(q.Context(), t.s.SpaceID(), t.s.DeviceID(), t.id.Issuer, t.id.AccountID, t.id.TenantID)
	if e != nil {
		return nil, e
	}
	return (api.AuthTransport{Token: token}).RoundTrip(q)
}
func (r *Runtime) conflicts(ctx context.Context, opt cli.Options, method string, body map[string]any, origin v1.OriginKind) (any, error) {
	entityKind, _ := body["kind"].(string)
	if entityKind == "" {
		entityKind = "task"
	}
	if entityKind != "task" && entityKind != "repository" {
		return nil, fail("INVALID_INPUT", "--kind must be task or repository.")
	}
	if method == "ConflictGet" {
		m := "GetTask"
		if entityKind == "repository" {
			m = "GetRepository"
		}
		b, _ := json.Marshal(map[string]any{"id": body["id"]})
		return r.invoke(ctx, opt, m, b, origin)
	}
	methods := []string{"ListTasks", "ListRepositories"}
	if _, specified := body["kind"]; specified {
		methods = methods[:1]
		if entityKind == "repository" {
			methods[0] = "ListRepositories"
		}
	}
	var items []*v1.EntityResult
	for _, m := range methods {
		cursor := ""
		for {
			b, _ := json.Marshal(map[string]any{"page": map[string]any{"size": 100, "cursor": cursor}})
			v, e := r.invoke(ctx, opt, m, b, origin)
			if e != nil {
				return nil, e
			}
			var page []*v1.EntityResult
			switch q := v.(type) {
			case *v1.ListTasksResponse:
				page = q.Items
				cursor = q.NextCursor
			case *v1.ListRepositoriesResponse:
				page = q.Items
				cursor = q.NextCursor
			}
			for _, item := range page {
				if len(item.Alternatives) > 1 || item.Result == v1.ResultKind_RESULT_KIND_CONFLICT {
					items = append(items, item)
				}
			}
			if cursor == "" {
				break
			}
		}
	}
	return &v1.ListTasksResponse{Items: items}, nil
}
func (r *Runtime) deleteAccount(ctx context.Context, opt cli.Options, method string, b map[string]any, origin v1.OriginKind) (any, error) {
	if !opt.Remote {
		return nil, fail("ONLINE_REQUIRED", "Account deletion requires --remote and browser reauthentication.")
	}
	account, _ := b["confirmedAccountId"].(string)
	if account == "" {
		return nil, fail("INVALID_INPUT", "--confirmed-account-id is required.")
	}
	mode := v1.DataDeletionMode_DATA_DELETION_MODE_DELETE_EVERYWHERE
	if method == "ClearServer" {
		mode = v1.DataDeletionMode_DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL
	}
	callback, _ := b["reauthCallback"].(string)
	challenge, _ := b["challengeId"].(string)
	if callback == "" {
		raw, _ := json.Marshal(map[string]any{"mode": mode.String(), "confirmedAccountId": account, "meta": b["meta"]})
		return r.invoke(ctx, opt, "BeginDataDeletion", raw, origin)
	}
	s, e := r.open(opt)
	if e != nil {
		return nil, e
	}
	id, e := s.Identity(ctx)
	if e != nil {
		return nil, e
	}
	cred, e := r.Auth.Load(ctx, s.SpaceID(), s.DeviceID(), id.Issuer, id.AccountID, id.TenantID)
	if e != nil {
		return nil, e
	}
	proof, e := r.Auth.ReauthenticationProof(ctx, cred, challenge, callback)
	if e != nil {
		return nil, e
	}
	raw, _ := json.Marshal(map[string]any{"mode": mode.String(), "confirmedAccountId": account, "reauthProof": proof, "meta": b["meta"]})
	result, e := r.invoke(ctx, opt, "DeleteAccountData", raw, origin)
	if e != nil {
		return nil, e
	}
	if e = s.SetSyncEnabled(ctx, false); e != nil {
		return nil, e
	}
	if e = r.Auth.Delete(ctx, s.SpaceID(), s.DeviceID(), id.Issuer, id.AccountID, id.TenantID); e != nil {
		return nil, e
	}
	if mode == v1.DataDeletionMode_DATA_DELETION_MODE_DELETE_EVERYWHERE {
		if e = s.RemoveSpace(ctx); e != nil {
			return nil, e
		}
	}
	return result, nil
}

func classify(err error) error {
	if err == nil {
		return nil
	}
	switch {
	case errors.Is(err, syncclient.ErrCredentials):
		return fail("UNAUTHENTICATED", "OS credentials are unavailable. Run lane login and approve the connection again.")
	case errors.Is(err, syncclient.ErrApproval):
		return fail("UNAUTHENTICATED", "Browser approval is invalid or expired. Begin a new connection.")
	case errors.Is(err, syncclient.ErrAccountMismatch):
		return fail("FORBIDDEN", "The credential does not belong to this account space.")
	case errors.Is(err, syncclient.ErrStaleEpoch):
		return fail("STALE_EPOCH", "Server data changed. Reconnect explicitly; local data remains retained.")
	case errors.Is(err, syncclient.ErrUnsupportedFormat):
		return fail("UNSUPPORTED_FORMAT", "Update Lane before synchronizing this format.")
	case errors.Is(err, syncclient.ErrLeaseHeld), errors.Is(err, syncclient.ErrLeaseLost):
		return fail("STORAGE_BUSY", "Another process owns synchronization. Retry after it exits.")
	}
	return err
}
