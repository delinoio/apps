// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/google/uuid"
	"github.com/tidwall/gjson"
)

const (
	geminiCLIHost   = "cloudcode-pa.googleapis.com"
	googleTokenHost = "oauth2.googleapis.com"
	geminiTestModel = "gemini-2.5-pro"
	geminiChatBody  = `{"model":"` + geminiTestModel + `","messages":[{"role":"user","content":"hi"}]}`
)

// hostRoutes sends each provider host to its fake server. The SDK Gemini CLI
// executor hardcodes cloudcode-pa.googleapis.com and the refresh uses
// oauth2.googleapis.com. A host without a route fails like an unreachable
// network.
type hostRoutes struct {
	routes map[string]string
	base   http.RoundTripper
}

func (r hostRoutes) RoundTrip(req *http.Request) (*http.Response, error) {
	target, ok := r.routes[req.URL.Hostname()]
	if !ok {
		return nil, errors.New("test network: host unreachable")
	}
	out := req.Clone(req.Context())
	out.URL.Scheme = "http"
	out.URL.Host = target
	out.Host = target
	return r.base.RoundTrip(out)
}

// tokenServer is a fake Google token endpoint.
type tokenServer struct {
	*upstream
	requests atomic.Int64
	mu       sync.Mutex
	forms    []url.Values
}

func (s *tokenServer) formsSeen() []url.Values {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]url.Values(nil), s.forms...)
}

func newTokenServer(t *testing.T, delay time.Duration, respond func(w http.ResponseWriter, n int64)) *tokenServer {
	s := &tokenServer{}
	s.upstream = newUpstream(t, func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		form, _ := url.ParseQuery(string(body))
		s.mu.Lock()
		s.forms = append(s.forms, form)
		s.mu.Unlock()
		n := s.requests.Add(1)
		time.Sleep(delay)
		respond(w, n)
	})
	return s
}

func grantToken(w http.ResponseWriter, _ int64) {
	w.Header().Set("Content-Type", "application/json")
	_, _ = io.WriteString(w, `{"access_token":"refreshed-access","token_type":"Bearer","expires_in":3600}`)
}

// geminiCLIUpstream serves the Cloud Code Assist generateContent endpoint.
func geminiCLIUpstream(t *testing.T) *upstream {
	return newUpstream(t, func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.Copy(io.Discard, r.Body)
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, `{"response":`+geminiResponse("hello cli")+`}`)
	})
}

type persistedSecret struct {
	resourceID, credentialID string
	secret                   []byte
}

type memPersister struct {
	mu     sync.Mutex
	writes []persistedSecret
}

func (p *memPersister) PersistCredentialSecret(_ context.Context, resourceID, credentialID string, secret []byte) error {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.writes = append(p.writes, persistedSecret{resourceID, credentialID, append([]byte(nil), secret...)})
	return nil
}

func (p *memPersister) waitWrites(t *testing.T, n int) []persistedSecret {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		p.mu.Lock()
		if len(p.writes) >= n {
			out := append([]persistedSecret(nil), p.writes...)
			p.mu.Unlock()
			return out
		}
		p.mu.Unlock()
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatalf("expected %d persisted payloads", n)
	return nil
}

func (p *memPersister) count() int {
	p.mu.Lock()
	defer p.mu.Unlock()
	return len(p.writes)
}

type geminiFixture struct {
	h         *harness
	cli       *upstream
	token     *tokenServer
	persister *memPersister
	resource  string
	key       string
	cred      Credential
	now       time.Time
}

// geminiCLICredential builds a payload shaped like the OAuth broker's.
func geminiCLICredential(resourceID string, expiry time.Time, mutate func(token map[string]any)) Credential {
	token := map[string]any{
		"access_token":  "expired-access",
		"token_type":    "Bearer",
		"refresh_token": "refresh-secret",
		"expiry":        expiry.UTC().Format(time.RFC3339Nano),
		"token_uri":     googleTokenURL,
		"client_id":     "payload-client-id",
		"client_secret": "payload-client-secret",
	}
	if mutate != nil {
		mutate(token)
	}
	payload, _ := json.Marshal(map[string]any{
		"type":       "gemini",
		"token":      token,
		"project_id": "test-project",
		"email":      "user@example.com",
	})
	return Credential{
		ID:         uuid.Must(uuid.NewV7()).String(),
		ResourceID: resourceID,
		Provider:   mcpav1.Provider_PROVIDER_GEMINI_CLI,
		Kind:       mcpav1.CredentialKind_CREDENTIAL_KIND_OAUTH,
		Secret:     payload,
		Models:     []string{geminiTestModel},
	}
}

func newGeminiFixture(t *testing.T, token *tokenServer, expiry time.Time, mutateToken func(map[string]any), mutateOptions func(*Options)) *geminiFixture {
	t.Helper()
	f := &geminiFixture{token: token, persister: &memPersister{}, cli: geminiCLIUpstream(t), now: time.Now()}
	routes := map[string]string{geminiCLIHost: strings.TrimPrefix(f.cli.URL(), "http://")}
	if token != nil {
		routes[googleTokenHost] = strings.TrimPrefix(token.URL(), "http://")
	}
	f.h = newHarness(t, func(o *Options) {
		o.BaseTransport = hostRoutes{routes: routes, base: http.DefaultTransport}
		o.Tokens = f.persister
		if mutateOptions != nil {
			mutateOptions(o)
		}
	})
	f.resource = newResourceID()
	f.key = f.h.addKey(f.resource)
	f.cred = geminiCLICredential(f.resource, expiry, mutateToken)
	f.h.addCredential(f.cred)
	return f
}

func (f *geminiFixture) call() callResult {
	f.h.t.Helper()
	return f.h.call(http.MethodPost, "/r/"+f.resource+"/v1/chat/completions", f.key, geminiChatBody, nil)
}

func TestGeminiCLIExpiredTokenRefreshesOnceAndPersists(t *testing.T) {
	token := newTokenServer(t, 0, grantToken)
	f := newGeminiFixture(t, token, time.Now().Add(-time.Hour), nil, nil)

	var results []callResult
	var billed []int64
	for i := 0; i < 3; i++ {
		before := f.cli.read.Load()
		res := f.call()
		if res.resp.StatusCode != http.StatusOK || !strings.Contains(string(res.body), "hello cli") {
			t.Fatalf("call %d: status %d body %s", i, res.resp.StatusCode, res.body)
		}
		record := f.h.sink.waitFinal(t, res.callID)
		if record.Status != mcpav1.CallStatus_CALL_STATUS_SUCCEEDED {
			t.Fatalf("call %d record %+v", i, record)
		}
		req, _ := f.h.sink.sums(res.callID)
		// Billed request bytes are exactly the model request: the token
		// request is not metered as call bytes.
		if got := f.cli.read.Load() - before; req != got {
			t.Fatalf("call %d billed %d request bytes, model upstream received %d", i, req, got)
		}
		results = append(results, res)
		billed = append(billed, req)
	}
	if billed[0] != billed[1] || billed[1] != billed[2] {
		t.Fatalf("billed request bytes differ between calls: %v", billed)
	}
	if got := token.requests.Load(); got != 1 {
		t.Fatalf("%d token requests, want 1", got)
	}
	form := token.formsSeen()[0]
	if form.Get("grant_type") != "refresh_token" || form.Get("refresh_token") != "refresh-secret" ||
		form.Get("client_id") != "payload-client-id" || form.Get("client_secret") != "payload-client-secret" {
		t.Fatalf("token request form %v", form)
	}
	for i, auth := range f.cli.authsSeen() {
		if auth != "Bearer refreshed-access" {
			t.Fatalf("model request %d used %q", i, auth)
		}
	}

	writes := f.persister.waitWrites(t, 1)
	time.Sleep(50 * time.Millisecond)
	if got := f.persister.count(); got != 1 {
		t.Fatalf("%d persisted payloads, want 1", got)
	}
	write := writes[0]
	if write.resourceID != f.resource || write.credentialID != f.cred.ID {
		t.Fatalf("persisted for %s/%s", write.resourceID, write.credentialID)
	}
	payload := gjson.ParseBytes(write.secret)
	if payload.Get("type").String() != "gemini" || payload.Get("project_id").String() != "test-project" ||
		payload.Get("token.access_token").String() != "refreshed-access" ||
		payload.Get("token.refresh_token").String() != "refresh-secret" ||
		payload.Get("token.client_id").String() != "payload-client-id" {
		t.Fatalf("persisted payload %s", write.secret)
	}
	expiry, err := time.Parse(time.RFC3339, payload.Get("token.expiry").String())
	if err != nil || time.Until(expiry) < 50*time.Minute {
		t.Fatalf("persisted expiry %q (%v) did not advance", payload.Get("token.expiry").String(), err)
	}
	_ = results
}

func TestGeminiCLIStreamingCallRefreshes(t *testing.T) {
	token := newTokenServer(t, 0, grantToken)
	f := newGeminiFixture(t, token, time.Now().Add(-time.Hour), nil, nil)
	f.cli.handler = func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.Copy(io.Discard, r.Body)
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "data: {\"response\":"+geminiResponse("hello stream")+"}\n\n")
	}
	body := `{"model":"` + geminiTestModel + `","stream":true,"messages":[{"role":"user","content":"hi"}]}`
	before := f.cli.read.Load()
	res := f.h.call(http.MethodPost, "/r/"+f.resource+"/v1/chat/completions", f.key, body, nil)
	if res.resp.StatusCode != http.StatusOK || !strings.Contains(string(res.body), "hello stream") {
		t.Fatalf("status %d body %s", res.resp.StatusCode, res.body)
	}
	record := f.h.sink.waitFinal(t, res.callID)
	req, _ := f.h.sink.sums(res.callID)
	if record.Status != mcpav1.CallStatus_CALL_STATUS_SUCCEEDED || req != f.cli.read.Load()-before {
		t.Fatalf("record %+v billed %d received %d", record, req, f.cli.read.Load()-before)
	}
	if got := token.requests.Load(); got != 1 {
		t.Fatalf("%d token requests, want 1", got)
	}
}

func TestGeminiCLIValidTokenMakesNoTokenRequest(t *testing.T) {
	token := newTokenServer(t, 0, grantToken)
	f := newGeminiFixture(t, token, time.Now().Add(time.Hour), func(m map[string]any) { m["access_token"] = "still-valid" }, nil)
	for i := 0; i < 3; i++ {
		if res := f.call(); res.resp.StatusCode != http.StatusOK {
			t.Fatalf("call %d status %d body %s", i, res.resp.StatusCode, res.body)
		}
	}
	if got := token.requests.Load(); got != 0 {
		t.Fatalf("%d token requests for a valid token", got)
	}
	if got := f.persister.count(); got != 0 {
		t.Fatalf("%d payloads persisted for a valid token", got)
	}
	for _, auth := range f.cli.authsSeen() {
		if auth != "Bearer still-valid" {
			t.Fatalf("model request used %q", auth)
		}
	}
}

func TestGeminiCLITokenExpiringSoonIsRefreshed(t *testing.T) {
	token := newTokenServer(t, 0, grantToken)
	f := newGeminiFixture(t, token, time.Now().Add(20*time.Second), nil, nil)
	if res := f.call(); res.resp.StatusCode != http.StatusOK {
		t.Fatalf("status %d body %s", res.resp.StatusCode, res.body)
	}
	if got := token.requests.Load(); got != 1 {
		t.Fatalf("%d token requests, want 1", got)
	}
}

func TestGeminiCLIConcurrentCallsShareOneRefresh(t *testing.T) {
	token := newTokenServer(t, 150*time.Millisecond, grantToken)
	f := newGeminiFixture(t, token, time.Now().Add(-time.Hour), nil, nil)
	const callers = 16
	statuses := make(chan int, callers)
	var wg sync.WaitGroup
	for i := 0; i < callers; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			req, _ := http.NewRequest(http.MethodPost, f.h.srv.URL+"/r/"+f.resource+"/v1/chat/completions", strings.NewReader(geminiChatBody))
			req.Header.Set("Content-Type", "application/json")
			req.Header.Set("Authorization", "Bearer "+f.key)
			resp, err := f.h.client.Do(req)
			if err != nil {
				statuses <- -1
				return
			}
			_, _ = io.Copy(io.Discard, resp.Body)
			_ = resp.Body.Close()
			statuses <- resp.StatusCode
		}()
	}
	wg.Wait()
	close(statuses)
	for status := range statuses {
		if status != http.StatusOK {
			t.Fatalf("a concurrent call returned %d", status)
		}
	}
	if got := token.requests.Load(); got != 1 {
		t.Fatalf("%d token requests for %d concurrent calls, want 1", got, callers)
	}
	f.persister.waitWrites(t, 1)
}

func TestGeminiCLIRevokedRefreshTokenIsProviderAuthError(t *testing.T) {
	token := newTokenServer(t, 0, func(w http.ResponseWriter, _ int64) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusBadRequest)
		_, _ = io.WriteString(w, `{"error":"invalid_grant","error_description":"Token has been expired or revoked."}`)
	})
	for name, stream := range map[string]bool{"non-streaming": false, "streaming": true} {
		t.Run(name, func(t *testing.T) {
			f := newGeminiFixture(t, token, time.Now().Add(-time.Hour), nil, nil)
			body := geminiChatBody
			if stream {
				body = `{"model":"` + geminiTestModel + `","stream":true,"messages":[{"role":"user","content":"hi"}]}`
			}
			res := f.h.call(http.MethodPost, "/r/"+f.resource+"/v1/chat/completions", f.key, body, nil)
			if res.resp.StatusCode != http.StatusUnauthorized {
				t.Fatalf("status %d body %s", res.resp.StatusCode, res.body)
			}
			if !strings.Contains(string(res.body), "invalid_grant") {
				t.Fatalf("body lacks the provider error code: %s", res.body)
			}
			if strings.Contains(string(res.body), "refresh-secret") || strings.Contains(string(res.body), "payload-client-secret") {
				t.Fatalf("body leaks a secret: %s", res.body)
			}
			record := f.h.sink.waitFinal(t, res.callID)
			if record.Status != mcpav1.CallStatus_CALL_STATUS_PROVIDER_AUTH_ERROR || record.ErrorClass != ErrorClassProviderAuth ||
				record.CredentialID != f.cred.ID || record.Provider != mcpav1.Provider_PROVIDER_GEMINI_CLI {
				t.Fatalf("record %+v", record)
			}
			// The model endpoint was never contacted, so no provider bytes
			// are billed.
			if req, _ := f.h.sink.sums(res.callID); req != 0 || f.cli.hits.Load() != 0 {
				t.Fatalf("billed %d request bytes, model endpoint hits %d", req, f.cli.hits.Load())
			}
			if got := f.persister.count(); got != 0 {
				t.Fatalf("%d payloads persisted after a failed refresh", got)
			}
		})
	}
}

func TestGeminiCLIUnauthorizedClientIsProviderAuthError(t *testing.T) {
	token := newTokenServer(t, 0, func(w http.ResponseWriter, _ int64) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusUnauthorized)
		_, _ = io.WriteString(w, `{"error":"unauthorized_client","error_description":"Unauthorized"}`)
	})
	f := newGeminiFixture(t, token, time.Now().Add(-time.Hour), nil, nil)
	res := f.call()
	if res.resp.StatusCode != http.StatusUnauthorized || gjson.GetBytes(res.body, "error.code").String() != "unauthorized_client" {
		t.Fatalf("status %d body %s", res.resp.StatusCode, res.body)
	}
	record := f.h.sink.waitFinal(t, res.callID)
	if record.Status != mcpav1.CallStatus_CALL_STATUS_PROVIDER_AUTH_ERROR {
		t.Fatalf("record %+v", record)
	}
}

func TestGeminiCLITransportFailureIs502(t *testing.T) {
	// No route for the token host: the token request gets no response.
	f := newGeminiFixture(t, nil, time.Now().Add(-time.Hour), nil, nil)
	res := f.call()
	if res.resp.StatusCode != http.StatusBadGateway {
		t.Fatalf("status %d body %s", res.resp.StatusCode, res.body)
	}
	if strings.Contains(string(res.body), "unreachable") {
		t.Fatalf("body leaks the transport error: %s", res.body)
	}
	record := f.h.sink.waitFinal(t, res.callID)
	if record.Status != mcpav1.CallStatus_CALL_STATUS_PROVIDER_ERROR || record.ErrorClass != ErrorClassNetwork {
		t.Fatalf("record %+v", record)
	}
	if req, _ := f.h.sink.sums(res.callID); req != 0 || f.cli.hits.Load() != 0 {
		t.Fatalf("billed %d request bytes, model endpoint hits %d", req, f.cli.hits.Load())
	}
}

func TestGeminiCLITokenEndpoint5xxIs502(t *testing.T) {
	token := newTokenServer(t, 0, func(w http.ResponseWriter, _ int64) {
		w.WriteHeader(http.StatusServiceUnavailable)
	})
	f := newGeminiFixture(t, token, time.Now().Add(-time.Hour), nil, nil)
	res := f.call()
	if res.resp.StatusCode != http.StatusBadGateway {
		t.Fatalf("status %d body %s", res.resp.StatusCode, res.body)
	}
	if record := f.h.sink.waitFinal(t, res.callID); record.Status != mcpav1.CallStatus_CALL_STATUS_PROVIDER_ERROR {
		t.Fatalf("record %+v", record)
	}
}

func TestGeminiCLIUsesOperatorClientWhenPayloadHasNone(t *testing.T) {
	token := newTokenServer(t, 0, grantToken)
	f := newGeminiFixture(t, token, time.Now().Add(-time.Hour), func(m map[string]any) {
		delete(m, "client_id")
		delete(m, "client_secret")
	}, func(o *Options) {
		o.GeminiCLIClientID, o.GeminiCLIClientSecret = "operator-client-id", "operator-client-secret"
	})
	if res := f.call(); res.resp.StatusCode != http.StatusOK {
		t.Fatalf("status %d body %s", res.resp.StatusCode, res.body)
	}
	form := token.formsSeen()[0]
	if form.Get("client_id") != "operator-client-id" || form.Get("client_secret") != "operator-client-secret" {
		t.Fatalf("token request form %v", form)
	}
}

// A payload reloaded after another replica refreshed (and notified) carries a
// valid token, so this replica makes no token request.
func TestGeminiCLIReloadedPayloadNeedsNoRefresh(t *testing.T) {
	token := newTokenServer(t, 0, grantToken)
	f := newGeminiFixture(t, token, time.Now().Add(-time.Hour), nil, nil)
	if res := f.call(); res.resp.StatusCode != http.StatusOK {
		t.Fatalf("status %d body %s", res.resp.StatusCode, res.body)
	}
	persisted := f.persister.waitWrites(t, 1)[0]
	if token.requests.Load() != 1 {
		t.Fatalf("%d token requests, want 1", token.requests.Load())
	}

	// Simulate the reload: the credential source now returns the payload the
	// first replica stored.
	reloaded := f.cred
	reloaded.Secret = persisted.secret
	f.h.creds.put(reloaded)
	f.h.engine.InvalidateResource(f.resource)
	for i := 0; i < 2; i++ {
		if res := f.call(); res.resp.StatusCode != http.StatusOK {
			t.Fatalf("call %d status %d body %s", i, res.resp.StatusCode, res.body)
		}
	}
	if got := token.requests.Load(); got != 1 {
		t.Fatalf("%d token requests after the reload, want 1", got)
	}
	if got := f.persister.count(); got != 1 {
		t.Fatalf("%d payloads persisted, want 1", got)
	}
}

// A replaced credential payload supersedes the refreshed one.
func TestGeminiCLIReplacedPayloadSupersedesRefreshedToken(t *testing.T) {
	token := newTokenServer(t, 0, grantToken)
	f := newGeminiFixture(t, token, time.Now().Add(-time.Hour), nil, nil)
	if res := f.call(); res.resp.StatusCode != http.StatusOK {
		t.Fatalf("status %d", res.resp.StatusCode)
	}
	replaced := geminiCLICredential(f.resource, time.Now().Add(time.Hour), func(m map[string]any) {
		m["access_token"] = "reconnected-access"
	})
	replaced.ID = f.cred.ID
	f.h.addCredential(replaced)
	if res := f.call(); res.resp.StatusCode != http.StatusOK {
		t.Fatalf("status %d", res.resp.StatusCode)
	}
	auths := f.cli.authsSeen()
	if last := auths[len(auths)-1]; last != "Bearer reconnected-access" {
		t.Fatalf("last model request used %q", last)
	}
	if got := token.requests.Load(); got != 1 {
		t.Fatalf("%d token requests, want 1", got)
	}
}

func TestMapGeminiRefreshError(t *testing.T) {
	err := mapGeminiRefreshError(errors.New("dial tcp 10.0.0.1:443: refused"))
	if err.StatusCode() != http.StatusBadGateway || !err.networkFailure() || strings.Contains(err.Error(), "10.0.0.1") {
		t.Fatalf("transport error mapped to %d %q", err.StatusCode(), err.Error())
	}
	if !json.Valid([]byte(fmt.Sprint(err.Error()))) {
		t.Fatalf("error text is not JSON: %s", err.Error())
	}
}
