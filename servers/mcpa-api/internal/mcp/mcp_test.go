// SPDX-License-Identifier: FSL-1.1-ALv2

package mcp_test

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"slices"
	"strings"
	"sync"
	"testing"

	"connectrpc.com/connect"
	gomcp "github.com/modelcontextprotocol/go-sdk/mcp"
	dto "github.com/prometheus/client_model/go"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protoreflect"
	"google.golang.org/protobuf/types/known/timestamppb"

	delcorev1 "github.com/delinoio/apps/protos/delcore/v1"
	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/protos/mcpa/v1/mcpav1connect"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth/authtest"
	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
	"github.com/delinoio/apps/servers/mcpa-api/internal/mcp"
	"github.com/delinoio/apps/servers/mcpa-api/internal/obs"
	"github.com/delinoio/apps/servers/mcpa-api/internal/server"
	"github.com/delinoio/apps/servers/mcpa-api/internal/web"
)

const (
	publicURL      = "https://api.example.test"
	webOrigin      = "https://console.example.test"
	mcpAudience    = "https://api.example.test/mcp"
	apiAudience    = "https://api.example.test"
	reauthURL      = "https://login.example.test/mfa"
	metadataURL    = publicURL + "/.well-known/oauth-protected-resource"
	personalKey    = "dlk_personal_secret"
	tenantKey      = "dlk_tenant_secret"
	keyUserID      = "0190f000-0000-7000-8000-00000000000a"
	keyTenantID    = "0190f000-0000-7000-8000-00000000000b"
	wantChallenge  = `Bearer resource_metadata="` + metadataURL + `"`
	createResource = "create_resource"
)

func discard() *slog.Logger { return slog.New(slog.NewTextHandler(io.Discard, nil)) }

type recorded struct {
	method string
	kind   mcpav1.ActorKind
	userID string
	msg    proto.Message
}

// fakeService records calls and answers a few RPCs; the rest stay
// unimplemented.
type fakeService struct {
	mcpav1connect.UnimplementedMcpaServiceHandler
	mu    sync.Mutex
	calls []recorded
}

func (f *fakeService) record(ctx context.Context, method string, msg proto.Message) *auth.Principal {
	p := auth.FromContext(ctx)
	f.mu.Lock()
	defer f.mu.Unlock()
	r := recorded{method: method, msg: msg}
	if p != nil {
		r.kind, r.userID = p.Kind, p.UserID
	}
	f.calls = append(f.calls, r)
	return p
}

func (f *fakeService) last() recorded {
	f.mu.Lock()
	defer f.mu.Unlock()
	if len(f.calls) == 0 {
		return recorded{}
	}
	return f.calls[len(f.calls)-1]
}

func (f *fakeService) GetViewer(ctx context.Context, req *connect.Request[mcpav1.GetViewerRequest]) (*connect.Response[mcpav1.GetViewerResponse], error) {
	p := f.record(ctx, "GetViewer", req.Msg)
	if p == nil {
		return nil, apperr.Unauthenticated("no principal")
	}
	return connect.NewResponse(&mcpav1.GetViewerResponse{
		Metadata: apperr.Metadata(ctx),
		Viewer:   &mcpav1.Viewer{UserId: p.UserID, ActorKind: p.Kind, Tenants: []*mcpav1.Tenant{{Id: p.KeyTenantID}}},
	}), nil
}

func (f *fakeService) CreateResource(ctx context.Context, req *connect.Request[mcpav1.CreateResourceRequest]) (*connect.Response[mcpav1.CreateResourceResponse], error) {
	f.record(ctx, "CreateResource", req.Msg)
	return connect.NewResponse(&mcpav1.CreateResourceResponse{
		Metadata: apperr.Metadata(ctx),
		Resource: &mcpav1.Resource{
			Id: "0190f000-0000-7000-8000-0000000000aa", TenantId: req.Msg.GetTenantId(), Name: req.Msg.GetName(),
			CallerRole: mcpav1.ResourceRole_RESOURCE_ROLE_MANAGE, CreateTime: timestamppb.New(timestamppb.Now().AsTime()),
		},
	}), nil
}

func (f *fakeService) DeleteResource(ctx context.Context, req *connect.Request[mcpav1.DeleteResourceRequest]) (*connect.Response[mcpav1.DeleteResourceResponse], error) {
	f.record(ctx, "DeleteResource", req.Msg)
	return nil, apperr.PermissionDenied("this action requires the manage role on the resource")
}

func (f *fakeService) GetTenantBudget(ctx context.Context, req *connect.Request[mcpav1.GetTenantBudgetRequest]) (*connect.Response[mcpav1.GetTenantBudgetResponse], error) {
	f.record(ctx, "GetTenantBudget", req.Msg)
	return nil, apperr.MFARequired(reauthURL)
}

type fakeKeys struct{}

func (fakeKeys) AuthorizeAPIKey(_ context.Context, secret string, _ delcore.Capability, _ bool) (delcore.KeyDecision, error) {
	switch secret {
	case personalKey:
		return delcore.KeyDecision{Allowed: true, OwnerKind: delcorev1.ApiKeyOwnerKind_API_KEY_OWNER_KIND_PERSONAL, UserID: keyUserID, TenantID: keyTenantID}, nil
	case tenantKey:
		return delcore.KeyDecision{Allowed: true, OwnerKind: delcorev1.ApiKeyOwnerKind_API_KEY_OWNER_KIND_TENANT, TenantID: keyTenantID}, nil
	}
	return delcore.KeyDecision{}, &delcore.Error{Kind: delcore.KindUnauthenticated, RPC: "AuthorizeApiKey", Code: connect.CodeUnauthenticated, Cause: errors.New("invalid")}
}

// fakeResolver binds MCP tokens of subject "mcp-user" to a grant.
type fakeResolver struct{}

func (fakeResolver) ResolveMCPToken(_ context.Context, tok *auth.VerifiedToken) (*auth.MCPGrant, error) {
	if tok.Subject != "mcp-user" {
		return nil, apperr.Unauthenticated("the MCP grant is not valid; sign in again")
	}
	return &auth.MCPGrant{UserID: "delcore-user-1", DelcoreToken: func(context.Context) (string, error) { return "dc", nil }}, nil
}

type env struct {
	t       *testing.T
	logto   *authtest.Provider
	svc     *fakeService
	h       http.Handler
	metrics *obs.Metrics
	srv     *httptest.Server
}

func newEnv(t *testing.T) *env {
	t.Helper()
	e := &env{t: t, logto: authtest.New(t), svc: &fakeService{}, metrics: obs.NewMetrics()}
	authn := auth.NewAuthenticator(auth.AuthenticatorConfig{
		Logto: auth.NewLogto(e.logto.Issuer(), "console-client", "secret", e.logto.Server.Client()),
		Keys:  fakeKeys{}, WebOrigin: webOrigin, MCPResource: mcpAudience, MFAClaim: "mcpa_mfa", ReauthURL: reauthURL,
	})
	authn.SetGrantResolver(fakeResolver{})
	m, err := mcp.Mount(mcp.Config{
		Service: e.svc, Authenticator: authn, ResourceMetadataURL: metadataURL, WebOrigin: webOrigin, Log: discard(),
	})
	if err != nil {
		t.Fatal(err)
	}
	// Serve through the assembled root handler so the request ID, tracing
	// and surface metrics apply as in production.
	e.h = server.New(server.Config{
		Service:       mcpav1connect.UnimplementedMcpaServiceHandler{},
		Authenticator: authn,
		Web:           web.New(web.Config{Authenticator: authn, Log: discard()}),
		Metrics:       e.metrics,
		Log:           discard(),
		WebOrigin:     webOrigin,
		Mounts:        []server.Mount{m},
	})
	e.srv = httptest.NewServer(e.h)
	t.Cleanup(e.srv.Close)
	return e
}

func (e *env) mcpToken(claims map[string]any) string {
	full := map[string]any{"sub": "mcp-user", "aud": mcpAudience, "mcpa_mfa": true}
	for k, v := range claims {
		if v == nil {
			delete(full, k)
		} else {
			full[k] = v
		}
	}
	return e.logto.Sign(e.t, full)
}

// post sends one JSON-RPC message to /mcp.
func (e *env) post(header map[string]string, body string) *httptest.ResponseRecorder {
	req := httptest.NewRequest(http.MethodPost, mcp.Path, strings.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json, text/event-stream")
	for k, v := range header {
		req.Header.Set(k, v)
	}
	rec := httptest.NewRecorder()
	e.h.ServeHTTP(rec, req)
	return rec
}

const toolsList = `{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}`

type bearerTransport struct{ token string }

func (b bearerTransport) RoundTrip(r *http.Request) (*http.Response, error) {
	r = r.Clone(r.Context())
	r.Header.Set("Authorization", "Bearer "+b.token)
	return http.DefaultTransport.RoundTrip(r)
}

// session connects the SDK client to /mcp with a bearer credential.
func (e *env) session(token string) *gomcp.ClientSession {
	e.t.Helper()
	client := gomcp.NewClient(&gomcp.Implementation{Name: "test", Version: "v0"}, nil)
	cs, err := client.Connect(context.Background(), &gomcp.StreamableClientTransport{
		Endpoint:   e.srv.URL + mcp.Path,
		HTTPClient: &http.Client{Transport: bearerTransport{token: token}},
	}, nil)
	if err != nil {
		e.t.Fatal(err)
	}
	e.t.Cleanup(func() { _ = cs.Close() })
	return cs
}

func TestToolName(t *testing.T) {
	for in, want := range map[protoreflect.Name]string{
		"CreateResource":           "create_resource",
		"ListCallRecords":          "list_call_records",
		"RegisterApiKeyCredential": "register_api_key_credential",
		"GetViewer":                "get_viewer",
		"ListAclEntries":           "list_acl_entries",
	} {
		if got := mcp.ToolName(in); got != want {
			t.Errorf("%s: %s", in, got)
		}
	}
}

func TestEveryRPCHasATool(t *testing.T) {
	tools, handlers, err := mcp.Tools(&fakeService{}, discard())
	if err != nil {
		t.Fatal(err)
	}
	methods := mcpav1.File_mcpa_v1_mcpa_proto.Services().ByName("McpaService").Methods()
	if len(tools) != methods.Len() || len(handlers) != len(tools) {
		t.Fatalf("%d tools for %d RPCs", len(tools), methods.Len())
	}
	byName := map[string]*gomcp.Tool{}
	for _, tool := range tools {
		if _, dup := byName[tool.Name]; dup {
			t.Fatalf("duplicate tool %s", tool.Name)
		}
		byName[tool.Name] = tool
	}
	for i := range methods.Len() {
		md := methods.Get(i)
		tool, ok := byName[mcp.ToolName(md.Name())]
		if !ok {
			t.Fatalf("RPC %s has no tool", md.Name())
		}
		in, _ := json.Marshal(tool.InputSchema)
		out, _ := json.Marshal(tool.OutputSchema)
		if !strings.Contains(string(in), `"type":"object"`) || !strings.Contains(string(out), `"metadata"`) || tool.Description == "" {
			t.Fatalf("%s: schemas %s / %s", tool.Name, in, out)
		}
		for j := range md.Input().Fields().Len() {
			f := md.Input().Fields().Get(j)
			if !strings.Contains(string(in), `"`+string(f.Name())+`"`) {
				t.Fatalf("%s input schema lacks %s", tool.Name, f.Name())
			}
		}
	}
}

// TestToolAnnotations pins the MCP hints of every tool to the side effects
// of its handler: read-only tools modify nothing, and non-destructive tools
// only add state.
func TestToolAnnotations(t *testing.T) {
	type hints struct{ readOnly, destructive, idempotent bool }
	read := hints{readOnly: true, idempotent: true}
	add := hints{}
	replace := hints{destructive: true}
	remove := hints{destructive: true, idempotent: true}
	want := map[string]hints{
		"get_viewer":                  read,
		"list_tenant_groups":          read,
		createResource:                add,
		"get_resource":                read,
		"list_resources":              read,
		"update_resource":             replace,
		"delete_resource":             remove,
		"update_routing":              replace,
		"list_acl_entries":            read,
		"put_acl_entry":               replace,
		"remove_acl_entry":            remove,
		"list_credentials":            read,
		"register_api_key_credential": add,
		"remove_credential":           remove,
		"start_provider_o_auth":       add,
		"complete_provider_o_auth":    add,
		// Polling a device flow registers the credential when it completes.
		"get_provider_o_auth_flow": add,
		"issue_call_key":           add,
		"list_call_keys":           read,
		"revoke_call_key":          remove,
		"get_resource_budget":      read,
		"set_resource_budget":      replace,
		"get_tenant_budget":        read,
		"set_tenant_budget":        replace,
		"get_usage":                read,
		"list_call_records":        read,
		"list_audit_events":        read,
		"list_tenant_audit_events": read,
	}
	tools, _, err := mcp.Tools(&fakeService{}, discard())
	if err != nil {
		t.Fatal(err)
	}
	if len(tools) != len(want) {
		t.Fatalf("%d tools, %d expectations", len(tools), len(want))
	}
	for _, tool := range tools {
		w, ok := want[tool.Name]
		if !ok {
			t.Fatalf("no expectation for %s", tool.Name)
		}
		a := tool.Annotations
		// destructiveHint is meaningful only for tools that modify state.
		if a == nil || a.OpenWorldHint == nil || (!a.ReadOnlyHint && a.DestructiveHint == nil) {
			t.Fatalf("%s: incomplete annotations %+v", tool.Name, a)
		}
		got := hints{readOnly: a.ReadOnlyHint, idempotent: a.IdempotentHint}
		if !a.ReadOnlyHint {
			got.destructive = *a.DestructiveHint
		}
		if got != w || *a.OpenWorldHint {
			t.Errorf("%s: hints %+v open world %v, want %+v", tool.Name, got, *a.OpenWorldHint, w)
		}
	}
}

func TestMessageSchema(t *testing.T) {
	s := mcp.MessageSchema((&mcpav1.CreateResourceResponse{}).ProtoReflect().Descriptor())
	raw, _ := json.Marshal(s)
	for _, want := range []string{
		`"create_time":{"format":"date-time","type":"string"}`,
		`"caller_role":{"enum":["RESOURCE_ROLE_UNSPECIFIED","RESOURCE_ROLE_MANAGE","RESOURCE_ROLE_INVOKE"],"type":"string"}`,
		`"endpoints":{"items":{`,
		`"additionalProperties":false`,
	} {
		if !strings.Contains(string(raw), want) {
			t.Fatalf("schema lacks %s: %s", want, raw)
		}
	}
}

func TestUnauthenticatedRequestGetsMetadataChallenge(t *testing.T) {
	e := newEnv(t)
	rec := e.post(nil, toolsList)
	if rec.Code != http.StatusUnauthorized || rec.Header().Get("WWW-Authenticate") != wantChallenge {
		t.Fatalf("%d %q", rec.Code, rec.Header().Get("WWW-Authenticate"))
	}
	if rec.Header().Get(server.RequestIDHeader) == "" {
		t.Fatal("no request ID")
	}
	rec = e.post(map[string]string{"Authorization": "Basic Zm9vOmJhcg=="}, toolsList)
	if rec.Code != http.StatusUnauthorized || !strings.Contains(rec.Header().Get("WWW-Authenticate"), `resource_metadata="`+metadataURL+`"`) {
		t.Fatalf("basic: %d %q", rec.Code, rec.Header().Get("WWW-Authenticate"))
	}
	if n := len(e.svc.calls); n != 0 {
		t.Fatalf("service called %d times", n)
	}
}

func TestBearerAuthentication(t *testing.T) {
	e := newEnv(t)
	cases := []struct {
		name      string
		token     string
		status    int
		challenge string
		body      string
	}{
		{"mcp audience", e.mcpToken(nil), http.StatusOK, "", ""},
		{"console audience", e.mcpToken(map[string]any{"aud": apiAudience}), http.StatusUnauthorized, `error="invalid_token"`, "unauthenticated"},
		{"console and another audience", e.mcpToken(map[string]any{"aud": []string{apiAudience, "other"}}), http.StatusUnauthorized, `error="invalid_token"`, "unauthenticated"},
		{"no grant", e.mcpToken(map[string]any{"sub": "stranger"}), http.StatusUnauthorized, `error="invalid_token"`, "unauthenticated"},
		{"missing MFA", e.mcpToken(map[string]any{"mcpa_mfa": nil}), http.StatusForbidden, `error="insufficient_user_authentication"`, `"reauthentication_url":"` + reauthURL + `"`},
		{"personal key", personalKey, http.StatusOK, "", ""},
		{"tenant key", tenantKey, http.StatusForbidden, "", "tenant_service_key_rejected"},
		{"invalid key", "dlk_unknown", http.StatusUnauthorized, `error="invalid_token"`, "unauthenticated"},
		{"garbage", "not-a-token", http.StatusUnauthorized, `error="invalid_token"`, "unauthenticated"},
	}
	for _, c := range cases {
		rec := e.post(map[string]string{"Authorization": "Bearer " + c.token}, toolsList)
		if rec.Code != c.status {
			t.Fatalf("%s: status %d body %s", c.name, rec.Code, rec.Body.String())
		}
		challenge := rec.Header().Get("WWW-Authenticate")
		if c.challenge != "" && (!strings.Contains(challenge, c.challenge) || !strings.Contains(challenge, `resource_metadata="`+metadataURL+`"`)) {
			t.Fatalf("%s: challenge %q", c.name, challenge)
		}
		if c.body != "" && !strings.Contains(rec.Body.String(), c.body) {
			t.Fatalf("%s: body %s", c.name, rec.Body.String())
		}
		if c.status == http.StatusOK && !strings.Contains(rec.Body.String(), createResource) {
			t.Fatalf("%s: tools/list body %s", c.name, rec.Body.String())
		}
	}
	if n := len(e.svc.calls); n != 0 {
		t.Fatalf("service called %d times by tools/list", n)
	}
}

func TestSigningKeyOutageAnswers503(t *testing.T) {
	e := newEnv(t)
	bearer := func(tok string) map[string]string { return map[string]string{"Authorization": "Bearer " + tok} }
	good := e.mcpToken(nil)

	e.logto.SetJWKSStatus(503)
	rec := e.post(bearer(good), toolsList)
	if rec.Code != http.StatusServiceUnavailable || !strings.Contains(rec.Body.String(), "dependency_unavailable") {
		t.Fatalf("outage: %d %s", rec.Code, rec.Body.String())
	}
	if c := rec.Header().Get("WWW-Authenticate"); c != "" {
		t.Fatalf("outage must not challenge the client: %q", c)
	}

	// Recovery needs no restart, and the keys are then cached.
	e.logto.SetJWKSStatus(0)
	for range 3 {
		if rec := e.post(bearer(good), toolsList); rec.Code != http.StatusOK {
			t.Fatalf("recovered: %d %s", rec.Code, rec.Body.String())
		}
	}
	forged := e.logto.SignForged(t, "k1", map[string]any{"sub": "mcp-user", "aud": mcpAudience, "mcpa_mfa": true})
	rec = e.post(bearer(forged), toolsList)
	if rec.Code != http.StatusUnauthorized || !strings.Contains(rec.Header().Get("WWW-Authenticate"), `error="invalid_token"`) {
		t.Fatalf("bad signature: %d %q", rec.Code, rec.Header().Get("WWW-Authenticate"))
	}
	if n := e.logto.JWKSRequests(); n != 2 {
		t.Fatalf("JWKS requests %d, want 2 (failed outage fetch and one successful fetch)", n)
	}
}

func TestOriginPolicy(t *testing.T) {
	e := newEnv(t)
	token := e.mcpToken(nil)
	for _, origin := range []string{"https://evil.example.test", "null", publicURL} {
		rec := e.post(map[string]string{"Authorization": "Bearer " + token, "Origin": origin}, toolsList)
		if rec.Code != http.StatusForbidden || rec.Header().Get("Access-Control-Allow-Origin") != "" {
			t.Fatalf("origin %s: %d", origin, rec.Code)
		}
	}
	rec := e.post(map[string]string{"Authorization": "Bearer " + token, "Origin": webOrigin}, toolsList)
	if rec.Code != http.StatusOK || rec.Header().Get("Access-Control-Allow-Credentials") != "" {
		t.Fatalf("console origin: %d %v", rec.Code, rec.Header())
	}
}

func TestToolsCallTheService(t *testing.T) {
	e := newEnv(t)
	cs := e.session(e.mcpToken(nil))
	list, err := cs.ListTools(context.Background(), nil)
	if err != nil {
		t.Fatal(err)
	}
	methods := mcpav1.File_mcpa_v1_mcpa_proto.Services().ByName("McpaService").Methods()
	if len(list.Tools) != methods.Len() {
		t.Fatalf("listed %d tools", len(list.Tools))
	}

	res, err := cs.CallTool(context.Background(), &gomcp.CallToolParams{Name: createResource, Arguments: map[string]any{
		"tenant_id": keyTenantID, "name": "prod", "request_key": "0190f000-0000-7000-8000-0000000000ff",
	}})
	if err != nil {
		t.Fatal(err)
	}
	if res.IsError {
		t.Fatalf("error result %+v", res.Content)
	}
	call := e.svc.last()
	req, ok := call.msg.(*mcpav1.CreateResourceRequest)
	if call.method != "CreateResource" || !ok || req.GetName() != "prod" || req.GetTenantId() != keyTenantID || req.GetRequestKey() == "" {
		t.Fatalf("service call %+v", call)
	}
	if call.kind != mcpav1.ActorKind_ACTOR_KIND_MCP || call.userID != "delcore-user-1" {
		t.Fatalf("actor %v %s", call.kind, call.userID)
	}
	structured, _ := json.Marshal(res.StructuredContent)
	text := res.Content[0].(*gomcp.TextContent).Text
	for _, s := range []string{string(structured), text} {
		var out struct {
			Metadata struct {
				TraceID string `json:"trace_id"`
			} `json:"metadata"`
			Resource struct {
				TenantID   string `json:"tenant_id"`
				CallerRole string `json:"caller_role"`
			} `json:"resource"`
		}
		if err := json.Unmarshal([]byte(s), &out); err != nil || out.Resource.TenantID != keyTenantID ||
			out.Resource.CallerRole != "RESOURCE_ROLE_MANAGE" || out.Metadata.TraceID == "" {
			t.Fatalf("ProtoJSON result %s: %v", s, err)
		}
	}

	// camelCase ProtoJSON names are accepted as well; unknown fields fail.
	if res, err := cs.CallTool(context.Background(), &gomcp.CallToolParams{Name: createResource, Arguments: map[string]any{"tenantId": "t", "requestKey": "k", "name": "n"}}); err != nil || res.IsError {
		t.Fatalf("camelCase: %v %+v", err, res)
	}
	res, err = cs.CallTool(context.Background(), &gomcp.CallToolParams{Name: createResource, Arguments: map[string]any{"api_key": "sk-secret-value"}})
	if err != nil {
		t.Fatal(err)
	}
	detail := toolError(t, res)
	if detail.Reason != "ERROR_REASON_INVALID_INPUT" || strings.Contains(detail.Message, "sk-secret-value") {
		t.Fatalf("unknown field: %+v", detail)
	}
}

type errorDetail struct {
	Reason              string `json:"reason"`
	Message             string `json:"message"`
	TraceID             string `json:"trace_id"`
	ReauthenticationURL string `json:"reauthentication_url"`
}

func toolError(t *testing.T, res *gomcp.CallToolResult) errorDetail {
	t.Helper()
	if !res.IsError || len(res.Content) != 1 {
		t.Fatalf("not an error result: %+v", res)
	}
	var body struct {
		Error errorDetail `json:"error"`
	}
	if err := json.Unmarshal([]byte(res.Content[0].(*gomcp.TextContent).Text), &body); err != nil {
		t.Fatal(err)
	}
	if body.Error.TraceID == "" {
		t.Fatal("tool error without trace ID")
	}
	return body.Error
}

func TestToolErrorsCarryTheReason(t *testing.T) {
	e := newEnv(t)
	cs := e.session(personalKey)

	res, err := cs.CallTool(context.Background(), &gomcp.CallToolParams{Name: "get_viewer", Arguments: map[string]any{}})
	if err != nil || res.IsError {
		t.Fatalf("get_viewer: %v %+v", err, res)
	}
	if call := e.svc.last(); call.kind != mcpav1.ActorKind_ACTOR_KIND_PERSONAL_KEY || call.userID != keyUserID {
		t.Fatalf("personal key actor %+v", call)
	}

	cases := map[string]struct {
		reason string
		reauth string
	}{
		"delete_resource":   {"ERROR_REASON_PERMISSION_DENIED", ""},
		"get_tenant_budget": {"ERROR_REASON_MFA_REQUIRED", reauthURL},
		// Unimplemented hooks surface the service's current behavior.
		"get_usage": {"ERROR_REASON_UNSPECIFIED", ""},
	}
	for name, c := range cases {
		res, err := cs.CallTool(context.Background(), &gomcp.CallToolParams{Name: name, Arguments: map[string]any{}})
		if err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		d := toolError(t, res)
		if d.Reason != c.reason || d.ReauthenticationURL != c.reauth || d.Message == "" {
			t.Fatalf("%s: %+v", name, d)
		}
	}
}

func TestMCPRequestsAreCountedOnTheMCPSurface(t *testing.T) {
	e := newEnv(t)
	e.post(nil, toolsList)
	e.post(map[string]string{"Authorization": "Bearer " + e.mcpToken(nil)}, toolsList)
	count := func(class string) float64 {
		var m dto.Metric
		if err := e.metrics.HTTPRequests.WithLabelValues(string(obs.SurfaceMCP), class).Write(&m); err != nil {
			t.Fatal(err)
		}
		return m.GetCounter().GetValue()
	}
	if got := count("4xx"); got != 1 {
		t.Fatalf("mcp 4xx = %v", got)
	}
	if got := count("2xx"); got != 1 {
		t.Fatalf("mcp 2xx = %v", got)
	}
	if slices.ContainsFunc(e.svc.calls, func(r recorded) bool { return r.method != "" }) {
		t.Fatal("service called")
	}
}
