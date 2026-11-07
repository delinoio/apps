// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"net"
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

// rawUpstream accepts one connection, reads the request header block and
// reports its size, then runs after.
func rawUpstream(t *testing.T, after func(conn net.Conn, r *bufio.Reader)) (string, <-chan int) {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	headerBytes := make(chan int, 1)
	done := make(chan struct{})
	go func() {
		defer close(done)
		conn, err := ln.Accept()
		if err != nil {
			return
		}
		defer conn.Close()
		r := bufio.NewReader(conn)
		n := 0
		for {
			line, err := r.ReadString('\n')
			n += len(line)
			if err != nil {
				return
			}
			if line == "\r\n" {
				break
			}
		}
		headerBytes <- n
		after(conn, r)
	}()
	t.Cleanup(func() {
		_ = ln.Close()
		<-done
	})
	return "http://" + ln.Addr().String(), headerBytes
}

// blockingBody returns head and then blocks until ctx ends.
type blockingBody struct {
	head []byte
	ctx  context.Context
}

func (b *blockingBody) Read(p []byte) (int, error) {
	if len(b.head) > 0 {
		n := copy(p, b.head)
		b.head = b.head[n:]
		return n, nil
	}
	<-b.ctx.Done()
	return 0, b.ctx.Err()
}

// An upload that does not complete bills its request line and headers but
// not the declared body size.
func TestTraceMeteringBillsNoBodyOfIncompleteUploads(t *testing.T) {
	const declared = 8 << 20
	t.Run("client cancellation", func(t *testing.T) {
		received := make(chan struct{})
		base, headerBytes := rawUpstream(t, func(conn net.Conn, r *bufio.Reader) {
			buf := make([]byte, 1024)
			if _, err := io.ReadFull(r, buf); err == nil {
				close(received)
			}
			_, _ = io.Copy(io.Discard, r)
		})
		call := &callState{id: "c1"}
		ctx, cancel := context.WithCancel(withTraceMetering(withCall(context.Background(), call), call, "/v1/messages?beta=true"))
		defer cancel()
		req, _ := http.NewRequestWithContext(ctx, http.MethodPost, base+"/v1/messages?beta=true",
			&blockingBody{head: bytes.Repeat([]byte("x"), 1024), ctx: ctx})
		req.ContentLength = declared
		errs := make(chan error, 1)
		go func() {
			resp, err := (&http.Client{Transport: &http.Transport{}}).Do(req)
			if err == nil {
				_ = resp.Body.Close()
			}
			errs <- err
		}()
		<-received
		cancel()
		if err := <-errs; !errors.Is(err, context.Canceled) {
			t.Fatalf("cancelled upload: %v", err)
		}
		if got, want := call.requestBytes.Load(), int64(<-headerBytes); got != want {
			t.Fatalf("metered %d bytes for a cancelled upload, its headers are %d bytes", got, want)
		}
	})
	t.Run("provider rejection", func(t *testing.T) {
		base, headerBytes := rawUpstream(t, func(conn net.Conn, _ *bufio.Reader) {
			_, _ = io.WriteString(conn, "HTTP/1.1 413 Request Entity Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
			// Discard unread input so closing resets the connection, as a
			// provider that stops reading does.
			if tcp, ok := conn.(*net.TCPConn); ok {
				_ = tcp.SetLinger(0)
			}
		})
		call := &callState{id: "c1"}
		ctx := withTraceMetering(withCall(context.Background(), call), call, "/v1/messages?beta=true")
		req, _ := http.NewRequestWithContext(ctx, http.MethodPost, base+"/v1/messages?beta=true", bytes.NewReader(make([]byte, declared)))
		resp, err := (&http.Client{Transport: &http.Transport{}}).Do(req)
		if err == nil {
			_ = resp.Body.Close()
		}
		if got, want := call.requestBytes.Load(), int64(<-headerBytes); got != want {
			t.Fatalf("metered %d bytes for a rejected upload, its headers are %d bytes", got, want)
		}
	})
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
	// The SDK Claude executor concatenates the base URL verbatim, so the
	// request line keeps every slash of the stored base URL.
	cases := []struct{ name, base, want string }{
		{"host only", "https://gw", "/v1/messages?beta=true"},
		{"host trailing slash", "https://gw/", "//v1/messages?beta=true"},
		{"path", "https://gw/path", "/path/v1/messages?beta=true"},
		{"path trailing slash", "https://gw/path/", "/path//v1/messages?beta=true"},
		{"kimi trailing slash", "https://api.kimi.com/coding/", "/coding//v1/messages?beta=true"},
		{"escaped segment", "https://gw/a%2Fb", "/a%2Fb/v1/messages?beta=true"},
		{"unparseable", "http://[::1", claudeMessagesURI},
		{"surrounding space", " https://gw/path ", "/path/v1/messages?beta=true"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := requestURIFor(tc.base, claudeMessagesURI); got != tc.want {
				t.Fatalf("requestURIFor(%q) = %q, want %q", tc.base, got, tc.want)
			}
		})
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
