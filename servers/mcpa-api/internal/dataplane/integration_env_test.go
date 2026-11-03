// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package dataplane

import (
	"context"
	"crypto/rand"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"os/signal"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"testing"
	"time"

	"connectrpc.com/connect"
	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/tidwall/gjson"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/protos/mcpa/v1/mcpav1connect"
	"github.com/delinoio/apps/servers/mcpa-api/internal/app"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth/authtest"
	"github.com/delinoio/apps/servers/mcpa-api/internal/config"
	"github.com/delinoio/apps/servers/mcpa-api/internal/obs"
	"github.com/delinoio/apps/servers/mcpa-api/internal/provideroauth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

// childEnv marks a re-executed test binary that serves mcpa-api like
// `mcpa-api serve`, so tests can kill a real process mid-stream.
const childEnv = "MCPA_IT_CHILD"

func TestMain(m *testing.M) {
	if os.Getenv(childEnv) == "1" {
		os.Exit(runChild())
	}
	os.Exit(m.Run())
}

// runChild runs the same wiring as `mcpa-api serve` from the process
// environment. Provider requests go to MCPA_IT_UPSTREAM.
func runChild() int {
	log := slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelWarn}))
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	cfg, err := config.LoadServe(os.LookupEnv)
	if err != nil {
		log.Error("child config", "error", err)
		return 1
	}
	a, err := app.New(ctx, cfg, log, obs.NewMetrics())
	if err != nil {
		log.Error("child app", "error", err)
		return 1
	}
	defer a.Close()
	upstream, _ := url.Parse(os.Getenv("MCPA_IT_UPSTREAM"))
	if _, err := Install(a, InstallOptions{BaseTransport: rewriteTransport{target: upstream}}); err != nil {
		log.Error("child install", "error", err)
		return 1
	}
	if err := a.Run(ctx); err != nil {
		log.Error("child run", "error", err)
		return 1
	}
	return 0
}

// rewriteTransport sends every provider request to a local fake upstream,
// keeping the path and query. Provider base URLs must be public https URLs,
// so tests register them with public names and route them here.
type rewriteTransport struct{ target *url.URL }

func (t rewriteTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	out := req.Clone(req.Context())
	out.URL.Scheme = t.target.Scheme
	out.URL.Host = t.target.Host
	out.Host = t.target.Host
	return http.DefaultTransport.RoundTrip(out)
}

// Canary strings must never reach the database.
const (
	promptCanary   = "PROMPT-CANARY-7f3a"
	outputCanary   = "OUTPUT-CANARY-91c2"
	upstreamAPIKey = "sk-upstream-canary-5d8e9b"
)

// fakeUpstream is an OpenAI-compatible provider.
type fakeUpstream struct {
	srv  *httptest.Server
	hits atomic.Int64
	// padding is appended to every completion to make large responses.
	padding atomic.Int64
	// stall, when non-nil, makes streams stop after the first chunk until it
	// is closed or the client goes away.
	mu     sync.Mutex
	stall  chan struct{}
	stalls chan struct{}
	auths  []string
}

func newFakeUpstream(t testing.TB) *fakeUpstream {
	u := &fakeUpstream{stalls: make(chan struct{}, 16)}
	u.srv = httptest.NewServer(http.HandlerFunc(u.serve))
	t.Cleanup(u.srv.Close)
	return u
}

func (u *fakeUpstream) setStall(ch chan struct{}) {
	u.mu.Lock()
	u.stall = ch
	u.mu.Unlock()
}

func chunk(model, text string, finish bool) string {
	reason := "null"
	if finish {
		reason = `"stop"`
	}
	return fmt.Sprintf(`data: {"id":"chatcmpl-1","object":"chat.completion.chunk","created":1700000000,"model":%q,"choices":[{"index":0,"delta":{"content":%q},"finish_reason":%s}]}`+"\n\n", model, text, reason)
}

func (u *fakeUpstream) serve(w http.ResponseWriter, r *http.Request) {
	u.hits.Add(1)
	u.mu.Lock()
	u.auths = append(u.auths, r.Header.Get("Authorization"))
	stall := u.stall
	u.mu.Unlock()
	body, _ := io.ReadAll(r.Body)
	model := gjson.GetBytes(body, "model").String()
	text := outputCanary + strings.Repeat("x", int(u.padding.Load()))
	if !strings.HasSuffix(r.URL.Path, "/chat/completions") {
		http.NotFound(w, r)
		return
	}
	if gjson.GetBytes(body, "stream").Bool() {
		w.Header().Set("Content-Type", "text/event-stream")
		flusher := w.(http.Flusher)
		_, _ = io.WriteString(w, chunk(model, text, false))
		flusher.Flush()
		if stall != nil {
			u.stalls <- struct{}{}
			select {
			case <-stall:
			case <-r.Context().Done():
				return
			}
		}
		_, _ = io.WriteString(w, chunk(model, " done", false))
		_, _ = io.WriteString(w, chunk(model, "", true))
		_, _ = io.WriteString(w, "data: [DONE]\n\n")
		flusher.Flush()
		return
	}
	w.Header().Set("Content-Type", "application/json")
	_, _ = fmt.Fprintf(w, `{"id":"chatcmpl-1","object":"chat.completion","created":1700000000,"model":%q,"choices":[{"index":0,"message":{"role":"assistant","content":%q},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}`, model, text)
}

// testClock is the data plane clock; tests move it forward.
type testClock struct{ offset atomic.Int64 }

func (c *testClock) Now() time.Time          { return time.Now().Add(time.Duration(c.offset.Load())) }
func (c *testClock) advance(d time.Duration) { c.offset.Add(int64(d)) }

// itEnv is one mcpa-api instance on a fresh PostgreSQL schema.
type itEnv struct {
	t      *testing.T
	ctx    context.Context
	dbURL  string
	db     *store.Store
	dc     *fakeDelcore
	logto  *authtest.Provider
	up     *fakeUpstream
	clock  *testClock
	app    *app.App
	dp     *DataPlane
	srv    *httptest.Server
	rpc    mcpav1connect.McpaServiceClient
	tenant string
	// users maps test user names to Delcore user UUIDs.
	users map[string]string
	vars  map[string]string
	// oauth is the fake provider for OAuth broker tests.
	oauth *httptest.Server
	// oauthHandler answers fake provider requests.
	oauthMu      sync.Mutex
	oauthHandler http.HandlerFunc
}

type envOption func(map[string]string)

// testDatabase returns a URL of a new schema of TEST_DATABASE_URL and drops
// it after the test.
func testDatabase(t *testing.T) string {
	t.Helper()
	base := os.Getenv("TEST_DATABASE_URL")
	if base == "" {
		t.Skip("TEST_DATABASE_URL is not set; skipping the PostgreSQL integration test")
	}
	ctx := context.Background()
	admin, err := pgx.Connect(ctx, base)
	if err != nil {
		t.Fatalf("connect TEST_DATABASE_URL: %v", err)
	}
	raw := make([]byte, 6)
	_, _ = rand.Read(raw)
	schema := "it_" + hex.EncodeToString(raw)
	if _, err := admin.Exec(ctx, "CREATE SCHEMA "+schema); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		_, _ = admin.Exec(context.Background(), "DROP SCHEMA "+schema+" CASCADE")
		_ = admin.Close(context.Background())
	})
	sep := "?"
	if strings.Contains(base, "?") {
		sep = "&"
	}
	return base + sep + "search_path=" + schema
}

func newEnv(t *testing.T, options ...envOption) *itEnv {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel)
	e := &itEnv{t: t, ctx: ctx, dbURL: testDatabase(t), dc: newFakeDelcore(t), logto: authtest.New(t), up: newFakeUpstream(t),
		clock: &testClock{}, tenant: uuid.NewString(), users: map[string]string{}}
	e.logto.SetTokenHandler(func(url.Values) (int, any) {
		return 200, map[string]any{"access_token": "m2m-token", "token_type": "Bearer", "expires_in": 3600}
	})
	e.oauth = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		e.oauthMu.Lock()
		h := e.oauthHandler
		e.oauthMu.Unlock()
		if h == nil {
			http.NotFound(w, r)
			return
		}
		h(w, r)
	}))
	t.Cleanup(e.oauth.Close)

	db, err := store.Open(ctx, e.dbURL, 5*time.Second, store.ModeServe)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := db.Migrate(ctx); err != nil {
		t.Fatal(err)
	}
	db.Close()

	masterKey := make([]byte, 32)
	_, _ = rand.Read(masterKey)
	keys, _ := json.Marshal(map[string]string{"k1": base64.RawURLEncoding.EncodeToString(masterKey)})
	e.vars = map[string]string{
		"DATABASE_URL":             e.dbURL,
		"MCPA_PUBLIC_URL":          "http://127.0.0.1:46329",
		"WEB_ORIGIN":               "http://127.0.0.1:46328",
		"MCPA_WEB_RETURN_URLS":     `["http://127.0.0.1:46328/"]`,
		"MCPA_MASTER_KEYS":         string(keys),
		"MCPA_ACTIVE_KEY_ID":       "k1",
		"MCPA_SERVICE_ID":          "019a0000-0000-7000-8000-000000000001",
		"DELCORE_URL":              e.dc.srv.URL,
		"DELCORE_RESOURCE":         "https://delcore.test",
		"LOGTO_TOKEN_URL":          e.logto.Issuer() + "/token",
		"CLIENT_ID":                "mcpa-m2m",
		"CLIENT_SECRET":            "m2m-secret",
		"LOGTO_ISSUER":             e.logto.Issuer(),
		"MCPA_LOGTO_CLIENT_ID":     "mcpa-web",
		"MCPA_LOGTO_CLIENT_SECRET": "web-secret",
		"MCPA_API_RESOURCE":        "https://mcpa.test",
		"MCPA_MCP_RESOURCE":        "https://mcpa.test/mcp",
		"MCPA_MFA_REAUTH_URL":      "https://logto.test/mfa",
		"MCPA_OAUTH_CLIENTS":       "{}",
		"MCPA_AUTHORIZE_CACHE_TTL": "30s",
		"MCPA_CHECKPOINT_INTERVAL": "100ms",
		"MCPA_OUTBOX_INTERVAL":     "10s",
		"DEPENDENCY_TIMEOUT":       "5s",
		"LOG_FORMAT":               "json",
	}
	for _, o := range options {
		o(e.vars)
	}
	cfg, err := config.LoadServe(func(k string) (string, bool) { v, ok := e.vars[k]; return v, ok })
	if err != nil {
		t.Fatalf("config: %v", err)
	}
	log := slog.New(slog.NewTextHandler(io.Discard, nil))
	if os.Getenv("MCPA_IT_VERBOSE") != "" {
		log = slog.New(slog.NewTextHandler(os.Stderr, nil))
	}
	a, err := app.New(ctx, cfg, log, obs.NewMetrics())
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(a.Close)
	base := e.oauth.URL
	oauthClient := &http.Client{Timeout: 5 * time.Second}
	dp, err := Install(a, InstallOptions{
		BaseTransport: rewriteTransport{target: mustURL(e.up.srv.URL)},
		Now:           e.clock.Now,
		OAuth: provideroauth.Options{
			HTTPClient: oauthClient, ClaudeHTTPClient: oauthClient,
			Endpoints: provideroauth.Endpoints{
				ClaudeAuthURL: base + "/claude/authorize", ClaudeTokenURL: base + "/claude/token",
				KimiDeviceAuthorizationURL: base + "/kimi/device", KimiTokenURL: base + "/kimi/token",
			},
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = dp.Close() })
	e.app, e.dp, e.db = a, dp, a.Store
	e.srv = httptest.NewServer(a.Handler())
	t.Cleanup(e.srv.Close)
	e.rpc = mcpav1connect.NewMcpaServiceClient(http.DefaultClient, e.srv.URL)
	e.addUser("owner")
	e.addUser("invoker")
	return e
}

func mustURL(raw string) *url.URL {
	u, err := url.Parse(raw)
	if err != nil {
		panic(err)
	}
	return u
}

// addUser registers a Delcore personal key "dlk_<name>" for a new user.
func (e *itEnv) addUser(name string) string {
	id := uuid.NewString()
	e.users[name] = id
	e.dc.set(func(f *fakeDelcore) { f.keys["dlk_"+name] = [2]string{id, e.tenant} })
	return id
}

func (e *itEnv) setOAuthHandler(h http.HandlerFunc) {
	e.oauthMu.Lock()
	e.oauthHandler = h
	e.oauthMu.Unlock()
}

// do sends a management RPC as user name.
func do[Req, Resp any](e *itEnv, name string, fn func(context.Context, *connect.Request[Req]) (*connect.Response[Resp], error), msg *Req) (*Resp, error) {
	req := connect.NewRequest(msg)
	req.Header().Set("Authorization", "Bearer dlk_"+name)
	resp, err := fn(e.ctx, req)
	if err != nil {
		return nil, err
	}
	return resp.Msg, nil
}

// mustDo sends a management RPC as user name and fails the test on error.
func mustDo[Req, Resp any](e *itEnv, name string, fn func(context.Context, *connect.Request[Req]) (*connect.Response[Resp], error), msg *Req) *Resp {
	e.t.Helper()
	resp, err := do(e, name, fn, msg)
	if err != nil {
		e.t.Fatalf("rpc: %v", err)
	}
	return resp
}

func newKey() string { return uuid.Must(uuid.NewV7()).String() }

// resource creates a resource with an OpenAI-compatible API-key
// credential serving model "compat-model" and returns its ID.
func (e *itEnv) resource(name string) string {
	res := mustDo(e, "owner", e.rpc.CreateResource, &mcpav1.CreateResourceRequest{RequestKey: newKey(), TenantId: e.tenant, Name: name})
	id := res.GetResource().GetId()
	mustDo(e, "owner", e.rpc.RegisterApiKeyCredential, &mcpav1.RegisterApiKeyCredentialRequest{
		ResourceId: id, Provider: mcpav1.Provider_PROVIDER_OPENAI_COMPATIBLE, Name: "upstream",
		ApiKey: upstreamAPIKey, BaseUrl: "https://upstream.example.com/v1", Models: []string{"compat-model"},
	})
	return id
}

// callKey issues a call key as user name.
func (e *itEnv) callKey(name, resourceID string) (string, string) {
	resp := mustDo(e, name, e.rpc.IssueCallKey, &mcpav1.IssueCallKeyRequest{RequestKey: newKey(), ResourceId: resourceID, Name: "key"})
	return resp.GetSecret(), resp.GetCallKey().GetId()
}

// callResult is a model call response.
type callResult struct {
	status int
	body   []byte
	callID string
}

func (e *itEnv) call(base, resourceID, key, path, body string, header map[string]string) callResult {
	e.t.Helper()
	req, err := http.NewRequestWithContext(e.ctx, http.MethodPost, base+"/r/"+resourceID+path, strings.NewReader(body))
	if err != nil {
		e.t.Fatal(err)
	}
	req.Header.Set("Content-Type", "application/json")
	if key != "" {
		req.Header.Set("Authorization", "Bearer "+key)
	}
	for k, v := range header {
		req.Header.Set(k, v)
	}
	client := &http.Client{Transport: &http.Transport{DisableCompression: true}}
	resp, err := client.Do(req)
	if err != nil {
		e.t.Fatal(err)
	}
	defer resp.Body.Close()
	data, _ := io.ReadAll(resp.Body)
	return callResult{status: resp.StatusCode, body: data, callID: resp.Header.Get("X-Mcpa-Request-Id")}
}

func chatBody(stream bool) string {
	return fmt.Sprintf(`{"model":"compat-model","stream":%t,"messages":[{"role":"user","content":%q}]}`, stream, promptCanary)
}

// waitFinal waits until the call record of callID is final.
func (e *itEnv) waitFinal(callID string) store.CallRecord {
	e.t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		var final bool
		err := e.db.Pool.QueryRow(e.ctx, `SELECT final FROM call_records WHERE id = $1`, callID).Scan(&final)
		if err == nil && final {
			return e.record(callID)
		}
		time.Sleep(20 * time.Millisecond)
	}
	e.t.Fatalf("call %s has no final record", callID)
	return store.CallRecord{}
}

func (e *itEnv) record(callID string) store.CallRecord {
	e.t.Helper()
	var r store.CallRecord
	var status, provider int16
	err := e.db.Pool.QueryRow(e.ctx, `SELECT id, resource_id, call_key_id, status, provider, model, request_bytes, response_bytes, start_time, final
		FROM call_records WHERE id = $1`, callID).Scan(&r.ID, &r.ResourceID, &r.CallKeyID, &status, &provider, &r.Model,
		&r.RequestBytes, &r.ResponseBytes, &r.StartTime, &r.Final)
	if err != nil {
		e.t.Fatalf("call record %s: %v", callID, err)
	}
	r.Status, r.Provider = mcpav1.CallStatus(status), mcpav1.Provider(provider)
	return r
}

func (e *itEnv) scalar(sql string, args ...any) int64 {
	e.t.Helper()
	var n int64
	if err := e.db.Pool.QueryRow(e.ctx, sql, args...).Scan(&n); err != nil {
		e.t.Fatalf("%s: %v", sql, err)
	}
	return n
}

// freePort returns an unused loopback port.
func freePort(t *testing.T) string {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()
	return ln.Addr().String()
}
