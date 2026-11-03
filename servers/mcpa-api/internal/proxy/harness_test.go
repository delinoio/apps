// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"context"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net"
	"net/http"
	"net/http/httptest"
	"sort"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/google/uuid"
	"github.com/tidwall/gjson"
)

// countingListener counts the raw bytes the server reads from its clients,
// which is the exact wire form the provider receives.
type countingListener struct {
	net.Listener
	read *atomic.Int64
}

func (l countingListener) Accept() (net.Conn, error) {
	conn, err := l.Listener.Accept()
	if err != nil {
		return nil, err
	}
	return countingConn{Conn: conn, read: l.read}, nil
}

type countingConn struct {
	net.Conn
	read *atomic.Int64
}

func (c countingConn) Read(p []byte) (int, error) {
	n, err := c.Conn.Read(p)
	c.read.Add(int64(n))
	return n, err
}

// upstream is a fake provider.
type upstream struct {
	t       *testing.T
	srv     *httptest.Server
	read    atomic.Int64
	hits    atomic.Int64
	mu      sync.Mutex
	auths   []string
	paths   []string
	headers []http.Header
	handler http.HandlerFunc
}

func newUpstream(t *testing.T, handler http.HandlerFunc) *upstream {
	t.Helper()
	u := &upstream{t: t, handler: handler}
	srv := httptest.NewUnstartedServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		u.hits.Add(1)
		auth := r.Header.Get("Authorization")
		if auth == "" {
			auth = r.Header.Get("X-Api-Key")
		}
		if auth == "" {
			auth = r.Header.Get("X-Goog-Api-Key")
		}
		u.mu.Lock()
		u.auths = append(u.auths, auth)
		u.paths = append(u.paths, r.URL.RequestURI())
		u.headers = append(u.headers, r.Header.Clone())
		u.mu.Unlock()
		// Without an upstream Date header, the only Date the caller sees is
		// the one net/http's server adds, which is not billed.
		w.Header()["Date"] = nil
		u.handler(w, r)
	}))
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	srv.Listener = countingListener{Listener: listener, read: &u.read}
	srv.Start()
	t.Cleanup(srv.Close)
	u.srv = srv
	return u
}

func (u *upstream) URL() string { return u.srv.URL }

func (u *upstream) authsSeen() []string {
	u.mu.Lock()
	defer u.mu.Unlock()
	return append([]string(nil), u.auths...)
}

func (u *upstream) headersSeen() []http.Header {
	u.mu.Lock()
	defer u.mu.Unlock()
	return append([]http.Header(nil), u.headers...)
}

type memCredentials struct {
	mu    sync.Mutex
	creds map[string][]Credential
	loads atomic.Int64
}

func (m *memCredentials) LoadResourceCredentials(_ context.Context, resourceID string) ([]Credential, error) {
	m.loads.Add(1)
	m.mu.Lock()
	defer m.mu.Unlock()
	return append([]Credential(nil), m.creds[resourceID]...), nil
}

func (m *memCredentials) put(cred Credential) {
	m.mu.Lock()
	defer m.mu.Unlock()
	list := m.creds[cred.ResourceID]
	for i := range list {
		if list[i].ID == cred.ID {
			list[i] = cred
			return
		}
	}
	m.creds[cred.ResourceID] = append(list, cred)
}

func (m *memCredentials) remove(resourceID, credentialID string) {
	m.mu.Lock()
	defer m.mu.Unlock()
	list := m.creds[resourceID]
	out := list[:0]
	for _, cred := range list {
		if cred.ID != credentialID {
			out = append(out, cred)
		}
	}
	m.creds[resourceID] = out
}

type memKeys struct {
	mu   sync.Mutex
	keys map[string]CallPrincipal
}

func (m *memKeys) AuthenticateCallKey(_ context.Context, resourceID, secret string) (CallPrincipal, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	principal, ok := m.keys[secret]
	if !ok || secret == "" || principal.ResourceID != resourceID {
		return CallPrincipal{}, Reject(RejectionInvalidCallKey, "invalid call key")
	}
	return principal, nil
}

type funcAuthorizer struct {
	mu        sync.Mutex
	err       error
	budgetErr error
	// calls records the order of gate invocations.
	calls []string
}

func (a *funcAuthorizer) AuthorizeCall(context.Context, CallPrincipal) error {
	a.mu.Lock()
	defer a.mu.Unlock()
	a.calls = append(a.calls, "authorize")
	return a.err
}

func (a *funcAuthorizer) CheckBudgets(context.Context, CallPrincipal) error {
	a.mu.Lock()
	defer a.mu.Unlock()
	a.calls = append(a.calls, "budgets")
	return a.budgetErr
}

func (a *funcAuthorizer) set(err error) {
	a.mu.Lock()
	a.err = err
	a.mu.Unlock()
}

func (a *funcAuthorizer) setBudget(err error) {
	a.mu.Lock()
	a.budgetErr = err
	a.mu.Unlock()
}

func (a *funcAuthorizer) reset() {
	a.mu.Lock()
	a.calls = nil
	a.mu.Unlock()
}

func (a *funcAuthorizer) gates() []string {
	a.mu.Lock()
	defer a.mu.Unlock()
	return append([]string(nil), a.calls...)
}

type memRouting struct {
	mu        sync.Mutex
	allowlist map[string][]string
}

func (m *memRouting) ModelAllowlist(_ context.Context, resourceID string) ([]string, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	return append([]string(nil), m.allowlist[resourceID]...), nil
}

func (m *memRouting) set(resourceID string, models ...string) {
	m.mu.Lock()
	m.allowlist[resourceID] = models
	m.mu.Unlock()
}

type checkpointEntry struct {
	callID    string
	deltaReq  int64
	deltaResp int64
	final     bool
	record    CallRecordUpdate
}

type recordingSink struct {
	mu      sync.Mutex
	entries []checkpointEntry
	// failNext makes the next n checkpoints fail.
	failNext atomic.Int64
	finals   chan string
}

func newRecordingSink() *recordingSink {
	return &recordingSink{finals: make(chan string, 1024)}
}

func (s *recordingSink) Checkpoint(_ context.Context, callID string, deltaReq, deltaResp int64, final bool, record CallRecordUpdate) error {
	s.mu.Lock()
	s.entries = append(s.entries, checkpointEntry{callID: callID, deltaReq: deltaReq, deltaResp: deltaResp, final: final, record: record})
	s.mu.Unlock()
	if s.failNext.Load() > 0 {
		s.failNext.Add(-1)
		return errors.New("sink unavailable")
	}
	if final {
		s.finals <- callID
	}
	return nil
}

func (s *recordingSink) waitFinal(t *testing.T, callID string) CallRecordUpdate {
	t.Helper()
	deadline := time.After(10 * time.Second)
	for {
		if record, ok := s.final(callID); ok {
			return record
		}
		select {
		case <-s.finals:
		case <-deadline:
			t.Fatalf("no final checkpoint for call %s", callID)
		case <-time.After(10 * time.Millisecond):
		}
	}
}

// final returns the committed final record. Failed attempts are recorded too,
// so the last final entry is the committed one.
func (s *recordingSink) final(callID string) (CallRecordUpdate, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for i := len(s.entries) - 1; i >= 0; i-- {
		if s.entries[i].callID == callID && s.entries[i].final {
			return s.entries[i].record, true
		}
	}
	return CallRecordUpdate{}, false
}

// committed returns the checkpoints of a call deduplicated by sequence
// number, as an idempotent sink would store them.
func (s *recordingSink) committed(callID string) []checkpointEntry {
	s.mu.Lock()
	defer s.mu.Unlock()
	bySeq := map[int64]checkpointEntry{}
	for _, entry := range s.entries {
		if entry.callID == callID {
			bySeq[entry.record.CheckpointSeq] = entry
		}
	}
	out := make([]checkpointEntry, 0, len(bySeq))
	for _, entry := range bySeq {
		out = append(out, entry)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].record.CheckpointSeq < out[j].record.CheckpointSeq })
	return out
}

func (s *recordingSink) sums(callID string) (int64, int64) {
	var req, resp int64
	for _, entry := range s.committed(callID) {
		req += entry.deltaReq
		resp += entry.deltaResp
	}
	return req, resp
}

func (s *recordingSink) count() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return len(s.entries)
}

type harness struct {
	t        *testing.T
	engine   *Engine
	srv      *httptest.Server
	creds    *memCredentials
	keys     *memKeys
	authz    *funcAuthorizer
	routing  *memRouting
	sink     *recordingSink
	client   *http.Client
	interval time.Duration
}

type harnessOption func(*Options)

func newHarness(t *testing.T, options ...harnessOption) *harness {
	t.Helper()
	h := &harness{
		t:       t,
		creds:   &memCredentials{creds: map[string][]Credential{}},
		keys:    &memKeys{keys: map[string]CallPrincipal{}},
		authz:   &funcAuthorizer{},
		routing: &memRouting{allowlist: map[string][]string{}},
		sink:    newRecordingSink(),
	}
	opts := Options{
		Credentials:         h.creds,
		Keys:                h.keys,
		Authorizer:          h.authz,
		Routing:             h.routing,
		Sink:                h.sink,
		Logger:              slog.New(slog.NewTextHandler(io.Discard, nil)),
		CheckpointInterval:  time.Hour,
		AutoRefreshInterval: -1,
	}
	for _, option := range options {
		option(&opts)
	}
	engine, err := New(opts)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = engine.Close() })
	h.engine = engine
	h.srv = httptest.NewServer(engine)
	t.Cleanup(h.srv.Close)
	// The test client must not decompress so response bytes stay exact.
	h.client = &http.Client{Transport: &http.Transport{DisableCompression: true}}
	return h
}

func newResourceID() string { return uuid.Must(uuid.NewV7()).String() }

// addKey registers a call key for a resource and returns it.
func (h *harness) addKey(resourceID string) string {
	key := "mcpa_test_" + uuid.NewString()
	h.keys.mu.Lock()
	h.keys.keys[key] = CallPrincipal{ResourceID: resourceID, TenantID: "tenant-" + resourceID, CallKeyID: "key-" + key[len(key)-6:], IssuerUserID: "user-1"}
	h.keys.mu.Unlock()
	return key
}

func (h *harness) addCredential(cred Credential) {
	h.t.Helper()
	h.creds.put(cred)
	if err := h.engine.UpsertCredential(context.Background(), cred); err != nil {
		h.t.Fatal(err)
	}
}

func compatCredential(resourceID, baseURL, apiKey string, models ...string) Credential {
	return Credential{
		ID:         uuid.Must(uuid.NewV7()).String(),
		ResourceID: resourceID,
		Provider:   mcpav1.Provider_PROVIDER_OPENAI_COMPATIBLE,
		Kind:       mcpav1.CredentialKind_CREDENTIAL_KIND_API_KEY,
		Secret:     []byte(apiKey),
		BaseURL:    baseURL + "/v1",
		Models:     models,
	}
}

type callResult struct {
	resp   *http.Response
	body   []byte
	callID string
}

// call sends a request and reads the full response.
func (h *harness) call(method, path, key string, body string, header map[string]string) callResult {
	h.t.Helper()
	req, err := http.NewRequest(method, h.srv.URL+path, strings.NewReader(body))
	if err != nil {
		h.t.Fatal(err)
	}
	if body != "" {
		req.Header.Set("Content-Type", "application/json")
	}
	if key != "" {
		req.Header.Set("Authorization", "Bearer "+key)
	}
	for name, value := range header {
		req.Header.Set(name, value)
	}
	resp, err := h.client.Do(req)
	if err != nil {
		h.t.Fatal(err)
	}
	defer resp.Body.Close()
	data, err := io.ReadAll(resp.Body)
	if err != nil {
		h.t.Fatal(err)
	}
	return callResult{resp: resp, body: data, callID: resp.Header.Get(RequestIDHeader)}
}

// expectedResponseBytes computes the customer segment by hand from the
// received response: status line, the handler's header lines, blank line and
// body. Date and Content-Length are added by net/http's server after the
// handler and are not billed.
func expectedResponseBytes(resp *http.Response, body []byte) int64 {
	text := http.StatusText(resp.StatusCode)
	size := len(fmt.Sprintf("HTTP/1.1 %03d %s\r\n", resp.StatusCode, text))
	for name, values := range resp.Header {
		if name == "Date" || name == "Content-Length" {
			continue
		}
		for _, value := range values {
			size += len(name + ": " + value + "\r\n")
		}
	}
	return int64(size + 2 + len(body))
}

func openAIChatResponse(model, text string) string {
	return fmt.Sprintf(`{"id":"chatcmpl-1","object":"chat.completion","created":1700000000,"model":%q,"choices":[{"index":0,"message":{"role":"assistant","content":%q},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}`, model, text)
}

func openAIChunk(model, text string, finish bool) string {
	reason := "null"
	if finish {
		reason = `"stop"`
	}
	return fmt.Sprintf(`data: {"id":"chatcmpl-1","object":"chat.completion.chunk","created":1700000000,"model":%q,"choices":[{"index":0,"delta":{"content":%q},"finish_reason":%s}]}`+"\n\n", model, text, reason)
}

// openAIUpstream serves chat completions, streaming when requested.
func openAIUpstream(t *testing.T, text string) *upstream {
	return newUpstream(t, func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		model := gjson.GetBytes(body, "model").String()
		if gjson.GetBytes(body, "stream").Bool() {
			w.Header().Set("Content-Type", "text/event-stream")
			flusher := w.(http.Flusher)
			for i, part := range strings.Split(text, " ") {
				_, _ = io.WriteString(w, openAIChunk(model, part, false))
				flusher.Flush()
				_ = i
			}
			_, _ = io.WriteString(w, openAIChunk(model, "", true))
			_, _ = io.WriteString(w, "data: [DONE]\n\n")
			flusher.Flush()
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, openAIChatResponse(model, text))
	})
}

func discardLogger() *slog.Logger { return slog.New(slog.NewTextHandler(io.Discard, nil)) }
