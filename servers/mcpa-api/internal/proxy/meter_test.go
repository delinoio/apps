// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/gin-gonic/gin"
	coreauth "github.com/router-for-me/CLIProxyAPI/v6/sdk/cliproxy/auth"
)

func TestMeteringTransportCountsHandComputedWireForm(t *testing.T) {
	up := newUpstream(t, func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(http.StatusNoContent) })
	call := &callState{id: "c1"}
	client := &http.Client{Transport: &meteringTransport{base: http.DefaultTransport}}
	body := `{"model":"m"}`
	req, _ := http.NewRequestWithContext(withCall(context.Background(), call), http.MethodPost, up.URL()+"/v1/chat/completions?x=1", strings.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Authorization", "Bearer sk")
	req.Header.Set("User-Agent", "test-agent")
	resp, err := client.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	_ = resp.Body.Close()

	host := strings.TrimPrefix(up.URL(), "http://")
	wire := "POST /v1/chat/completions?x=1 HTTP/1.1\r\n" +
		"Host: " + host + "\r\n" +
		"User-Agent: test-agent\r\n" +
		"Content-Length: 13\r\n" +
		"Authorization: Bearer sk\r\n" +
		"Content-Type: application/json\r\n" +
		"Accept-Encoding: gzip\r\n" +
		"\r\n" + body
	if got := call.requestBytes.Load(); got != int64(len(wire)) {
		t.Fatalf("metered %d bytes, hand-computed wire form is %d", got, len(wire))
	}
	if got := up.read.Load(); got != int64(len(wire)) {
		t.Fatalf("upstream received %d bytes, hand-computed %d", got, len(wire))
	}
	if call.httpStatus != http.StatusNoContent {
		t.Fatalf("status %d", call.httpStatus)
	}
}

func TestMeteringTransportIgnoresUnattributedRequests(t *testing.T) {
	up := newUpstream(t, func(w http.ResponseWriter, r *http.Request) {})
	call := &callState{id: "c1"}
	client := &http.Client{Transport: &meteringTransport{base: http.DefaultTransport}}
	// A background request (for example a token refresh) carries no call.
	resp, err := client.Get(up.URL())
	if err != nil {
		t.Fatal(err)
	}
	_ = resp.Body.Close()
	// A trace-metered context is counted by its trace hook, not the transport.
	ctx := context.WithValue(withCall(context.Background(), call), traceMeteredContextKey{}, true)
	req, _ := http.NewRequestWithContext(ctx, http.MethodGet, up.URL(), nil)
	resp, err = client.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	_ = resp.Body.Close()
	if call.requestBytes.Load() != 0 {
		t.Fatalf("counted %d bytes", call.requestBytes.Load())
	}
}

func TestTraceMeteringMatchesWireForm(t *testing.T) {
	up := newUpstream(t, func(w http.ResponseWriter, r *http.Request) {})
	call := &callState{id: "c1"}
	ctx := withTraceMetering(withCall(context.Background(), call), call, "/v1/messages?beta=true")
	body := `{"model":"claude"}`
	req, _ := http.NewRequestWithContext(ctx, http.MethodPost, up.URL()+"/v1/messages?beta=true", strings.NewReader(body))
	req.Header.Set("X-Api-Key", "sk-ant")
	// A private transport, like the SDK Claude executor's client.
	resp, err := (&http.Client{Transport: &http.Transport{}}).Do(req)
	if err != nil {
		t.Fatal(err)
	}
	_ = resp.Body.Close()
	if got, want := call.requestBytes.Load(), up.read.Load(); got != want {
		t.Fatalf("trace metered %d bytes, upstream received %d", got, want)
	}
}

func TestCallFromContextResolvesSDKExecutionContext(t *testing.T) {
	call := &callState{id: "c1"}
	inbound := httptest.NewRequest(http.MethodPost, "/v1/chat/completions", nil).WithContext(withCall(context.Background(), call))
	ginCtx := &gin.Context{Request: inbound}
	// The SDK builds execution contexts from context.Background() and stores
	// the gin context under "gin".
	execCtx := context.WithValue(context.Background(), "gin", ginCtx) //nolint:staticcheck // mirrors the SDK key
	if got := callFromContext(execCtx); got != call {
		t.Fatalf("resolved %v", got)
	}
	if callFromContext(context.Background()) != nil {
		t.Fatal("background context resolved a call")
	}
}

func TestCountingResponseWriterHandComputed(t *testing.T) {
	call := &callState{id: "c1"}
	rec := httptest.NewRecorder()
	w := newCountingResponseWriter(rec, call)
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set(RequestIDHeader, "id-1")
	w.WriteHeader(http.StatusTooManyRequests)
	_, _ = w.Write([]byte(`{"a":1}`))
	_, _ = w.Write([]byte(`{"b":2}`))
	w.Flush()
	wire := "HTTP/1.1 429 Too Many Requests\r\n" +
		"Content-Type: application/json\r\n" +
		"X-Mcpa-Request-Id: id-1\r\n" +
		"\r\n" + `{"a":1}{"b":2}`
	if got := call.responseBytes.Load(); got != int64(len(wire)) {
		t.Fatalf("counted %d, want %d", got, len(wire))
	}
	if statusLineBytes(799) != int64(len("HTTP/1.1 799 status code 799\r\n")) {
		t.Fatal("unknown status line size")
	}
}

func TestRequestURIFor(t *testing.T) {
	if got := requestURIFor("https://api.anthropic.com", claudeMessagesURI); got != "/v1/messages?beta=true" {
		t.Fatal(got)
	}
	if got := requestURIFor("https://api.kimi.com/coding/", claudeMessagesURI); got != "/coding/v1/messages?beta=true" {
		t.Fatal(got)
	}
}

func TestClassify(t *testing.T) {
	cases := []struct {
		name string
		in   outcomeInput
		want mcpav1.CallStatus
		cls  ErrorClass
		fail string
	}{
		{"success", outcomeInput{executorInvoked: true, providerStatus: 200}, mcpav1.CallStatus_CALL_STATUS_SUCCEEDED, ErrorClassNone, ""},
		{"rate limited", outcomeInput{executorInvoked: true, providerStatus: 429}, mcpav1.CallStatus_CALL_STATUS_PROVIDER_ERROR, ErrorClassRateLimited, "429"},
		{"auth", outcomeInput{executorInvoked: true, providerStatus: 403}, mcpav1.CallStatus_CALL_STATUS_PROVIDER_AUTH_ERROR, ErrorClassProviderAuth, "auth"},
		{"network", outcomeInput{executorInvoked: true, networkError: true}, mcpav1.CallStatus_CALL_STATUS_PROVIDER_ERROR, ErrorClassNetwork, "network"},
		{"interrupted", outcomeInput{executorInvoked: true, providerStatus: 200, streamFailed: true, sentStreamData: true}, mcpav1.CallStatus_CALL_STATUS_STREAM_INTERRUPTED, ErrorClassStreamInterrupted, "network"},
		{"cancelled wins", outcomeInput{executorInvoked: true, providerStatus: 500, clientCancelled: true}, mcpav1.CallStatus_CALL_STATUS_CLIENT_CANCELLED, ErrorClassClientCancelled, "5xx"},
		{"sdk refused", outcomeInput{customerStatus: 502}, mcpav1.CallStatus_CALL_STATUS_NO_CREDENTIAL, ErrorClassNoCredential, ""},
		{"internal", outcomeInput{executorInvoked: true, providerStatus: 200, customerStatus: 500}, mcpav1.CallStatus_CALL_STATUS_INTERNAL_ERROR, ErrorClassInternal, ""},
		{"rejected", outcomeInput{rejection: Reject(RejectionBudgetExhausted, "")}, mcpav1.CallStatus_CALL_STATUS_REJECTED_BUDGET, ErrorClassBudgetExhausted, ""},
	}
	for _, tc := range cases {
		got := classify(tc.in)
		if got.status != tc.want || got.errorClass != tc.cls || got.failureClass != tc.fail {
			t.Errorf("%s: %+v", tc.name, got)
		}
	}
}

type recordingPersister struct {
	mu    sync.Mutex
	saved map[string][]byte
	calls int
}

func (p *recordingPersister) PersistCredentialSecret(_ context.Context, _, credentialID string, secret []byte) error {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.calls++
	p.saved[credentialID] = secret
	return nil
}

func TestResourceStorePersistsOnlyChangedOAuthPayloads(t *testing.T) {
	persister := &recordingPersister{saved: map[string][]byte{}}
	async := newAsyncPersister(persister, discardLogger(), time.Second)
	store := newResourceStore("r1", async)
	auth := &coreauth.Auth{
		ID:         "cred-1",
		Attributes: map[string]string{attrResourceID: "r1", attrCredentialID: "cred-1"},
		Metadata:   map[string]any{"type": "claude", "access_token": "old"},
	}
	store.track(auth)
	// A call result persists unchanged metadata: nothing is written.
	_, _ = store.Save(context.Background(), auth)
	// A refresh changes the token.
	auth.Metadata["access_token"] = "new"
	_, _ = store.Save(context.Background(), auth)
	// Another resource's auth and unknown credentials are ignored.
	_, _ = store.Save(context.Background(), &coreauth.Auth{ID: "x", Attributes: map[string]string{attrResourceID: "r2"}, Metadata: map[string]any{"a": 1}})
	_, _ = store.Save(context.Background(), &coreauth.Auth{ID: "unknown", Attributes: map[string]string{attrResourceID: "r1"}, Metadata: map[string]any{"a": 1}})
	async.close()
	if persister.calls != 1 {
		t.Fatalf("persist calls %d", persister.calls)
	}
	var saved map[string]any
	if err := json.Unmarshal(persister.saved["cred-1"], &saved); err != nil || saved["access_token"] != "new" {
		t.Fatalf("saved %s", persister.saved["cred-1"])
	}
}

func TestBuildAuthMapsProviders(t *testing.T) {
	now := time.Now()
	oauth, _ := json.Marshal(map[string]any{"type": "gemini", "email": "a@example.com", "token": map[string]any{"access_token": "t"}})
	cases := []struct {
		cred     Credential
		provider string
		wantErr  bool
	}{
		{Credential{ID: "1", ResourceID: "r", Provider: mcpav1.Provider_PROVIDER_GEMINI, Kind: mcpav1.CredentialKind_CREDENTIAL_KIND_API_KEY, Secret: []byte("k")}, sdkProviderGemini, false},
		{Credential{ID: "2", ResourceID: "r", Provider: mcpav1.Provider_PROVIDER_CLAUDE, Kind: mcpav1.CredentialKind_CREDENTIAL_KIND_API_KEY, Secret: []byte("k")}, sdkProviderClaude, false},
		{Credential{ID: "3", ResourceID: "r", Provider: mcpav1.Provider_PROVIDER_GEMINI_CLI, Kind: mcpav1.CredentialKind_CREDENTIAL_KIND_OAUTH, Secret: oauth}, sdkProviderGeminiCLI, false},
		{Credential{ID: "4", ResourceID: "r", Provider: mcpav1.Provider_PROVIDER_OPENAI_COMPATIBLE, Kind: mcpav1.CredentialKind_CREDENTIAL_KIND_API_KEY, Secret: []byte("k")}, "", true},
		{Credential{ID: "5", ResourceID: "r", Provider: mcpav1.Provider_PROVIDER_ANTIGRAVITY, Kind: mcpav1.CredentialKind_CREDENTIAL_KIND_API_KEY, Secret: []byte("k")}, "", true},
		{Credential{ID: "6", ResourceID: "r", Provider: mcpav1.Provider_PROVIDER_CLAUDE, Kind: mcpav1.CredentialKind_CREDENTIAL_KIND_OAUTH, Secret: oauth}, "", true},
	}
	for _, tc := range cases {
		built, err := buildAuth(tc.cred, now)
		if (err != nil) != tc.wantErr {
			t.Fatalf("%s: err %v", tc.cred.ID, err)
		}
		if err != nil {
			continue
		}
		if built.auth.Provider != tc.provider || built.auth.Attributes[attrResourceID] != "r" || len(built.models) == 0 {
			t.Fatalf("%s: %+v (%d models)", tc.cred.ID, built.auth, len(built.models))
		}
	}
}
