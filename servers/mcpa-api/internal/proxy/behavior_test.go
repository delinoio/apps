// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"bufio"
	"context"
	"errors"
	"io"
	"net/http"
	"strings"
	"sync"
	"testing"
	"time"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/testutil"
	"github.com/tidwall/gjson"
)

const chatBody = `{"model":"m1","messages":[{"role":"user","content":"hi"}]}`

func streamBody(model string) string {
	return `{"model":"` + model + `","stream":true,"messages":[{"role":"user","content":"hi"}]}`
}

// slowStreamUpstream sends one chunk, then waits until release is closed or
// the request ends.
func slowStreamUpstream(t *testing.T, release <-chan struct{}) *upstream {
	return newUpstream(t, func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		model := gjson.GetBytes(body, "model").String()
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, openAIChunk(model, "first", false))
		w.(http.Flusher).Flush()
		select {
		case <-release:
		case <-r.Context().Done():
			return
		}
		_, _ = io.WriteString(w, openAIChunk(model, "", true))
		_, _ = io.WriteString(w, "data: [DONE]\n\n")
	})
}

func TestClientCancellationMidStreamBillsSentBytes(t *testing.T) {
	h := newHarness(t)
	release := make(chan struct{})
	defer close(release)
	up := slowStreamUpstream(t, release)
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	h.addCredential(compatCredential(resourceID, up.URL(), "sk", "m1"))

	ctx, cancel := context.WithCancel(context.Background())
	req, _ := http.NewRequestWithContext(ctx, http.MethodPost, h.srv.URL+"/r/"+resourceID+"/v1/chat/completions", strings.NewReader(streamBody("m1")))
	req.Header.Set("Authorization", "Bearer "+key)
	resp, err := h.client.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	callID := resp.Header.Get(RequestIDHeader)
	line, err := bufio.NewReader(resp.Body).ReadString('\n')
	if err != nil || !strings.Contains(line, "first") {
		t.Fatalf("first chunk %q: %v", line, err)
	}
	cancel()
	_ = resp.Body.Close()

	record := h.sink.waitFinal(t, callID)
	if record.Status != mcpav1.CallStatus_CALL_STATUS_CLIENT_CANCELLED {
		t.Fatalf("status %s", record.Status)
	}
	if got := up.read.Load(); record.RequestBytes != got {
		t.Errorf("request bytes %d, upstream received %d", record.RequestBytes, got)
	}
	if record.ResponseBytes <= 0 {
		t.Errorf("response bytes %d", record.ResponseBytes)
	}
	if req, resp := h.sink.sums(callID); req != record.RequestBytes || resp != record.ResponseBytes {
		t.Errorf("deltas %d/%d, totals %d/%d", req, resp, record.RequestBytes, record.ResponseBytes)
	}
}

func TestProviderDisconnectMidStream(t *testing.T) {
	h := newHarness(t)
	up := newUpstream(t, func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.ReadAll(r.Body)
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, openAIChunk("m1", "partial", false))
		w.(http.Flusher).Flush()
		conn, _, err := http.NewResponseController(w).Hijack()
		if err == nil {
			_ = conn.Close()
		}
	})
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	h.addCredential(compatCredential(resourceID, up.URL(), "sk", "m1"))

	res := h.call(http.MethodPost, "/r/"+resourceID+"/v1/chat/completions", key, streamBody("m1"), nil)
	if !strings.Contains(string(res.body), "partial") {
		t.Fatalf("body %s", res.body)
	}
	record := h.sink.waitFinal(t, res.callID)
	if record.Status != mcpav1.CallStatus_CALL_STATUS_STREAM_INTERRUPTED {
		t.Fatalf("status %s class %s", record.Status, record.ErrorClass)
	}
	if record.RequestBytes != up.read.Load() || record.ResponseBytes != expectedResponseBytes(res.resp, res.body) {
		t.Errorf("bytes %d/%d", record.RequestBytes, record.ResponseBytes)
	}
}

func TestProviderErrorsPassThroughWithoutRetry(t *testing.T) {
	cases := []struct {
		status     int
		wantStatus mcpav1.CallStatus
		wantClass  mcpav1.StatusClass
		wantError  ErrorClass
	}{
		{http.StatusTooManyRequests, mcpav1.CallStatus_CALL_STATUS_PROVIDER_ERROR, mcpav1.StatusClass_STATUS_CLASS_429, ErrorClassRateLimited},
		{http.StatusInternalServerError, mcpav1.CallStatus_CALL_STATUS_PROVIDER_ERROR, mcpav1.StatusClass_STATUS_CLASS_5XX, ErrorClassProvider5xx},
		{http.StatusServiceUnavailable, mcpav1.CallStatus_CALL_STATUS_PROVIDER_ERROR, mcpav1.StatusClass_STATUS_CLASS_5XX, ErrorClassProvider5xx},
		{http.StatusUnauthorized, mcpav1.CallStatus_CALL_STATUS_PROVIDER_AUTH_ERROR, mcpav1.StatusClass_STATUS_CLASS_4XX, ErrorClassProviderAuth},
		{http.StatusBadRequest, mcpav1.CallStatus_CALL_STATUS_PROVIDER_ERROR, mcpav1.StatusClass_STATUS_CLASS_4XX, ErrorClassProvider4xx},
	}
	for _, tc := range cases {
		t.Run(http.StatusText(tc.status), func(t *testing.T) {
			h := newHarness(t)
			up := newUpstream(t, func(w http.ResponseWriter, r *http.Request) {
				_, _ = io.ReadAll(r.Body)
				w.Header().Set("Content-Type", "application/json")
				w.WriteHeader(tc.status)
				_, _ = io.WriteString(w, `{"error":{"message":"upstream says no","type":"upstream_error"}}`)
			})
			resourceID := newResourceID()
			key := h.addKey(resourceID)
			// Two accounts: a retry would reach the second one.
			h.addCredential(compatCredential(resourceID, up.URL(), "sk-one", "m1"))
			h.addCredential(compatCredential(resourceID, up.URL(), "sk-two", "m1"))
			for _, body := range []string{chatBody, streamBody("m1")} {
				hitsBefore := up.hits.Load()
				res := h.call(http.MethodPost, "/r/"+resourceID+"/v1/chat/completions", key, body, nil)
				if res.resp.StatusCode != tc.status {
					t.Fatalf("status %d, want %d (%s)", res.resp.StatusCode, tc.status, res.body)
				}
				if !strings.Contains(string(res.body), "upstream says no") {
					t.Fatalf("body %s", res.body)
				}
				if got := up.hits.Load() - hitsBefore; got != 1 {
					t.Fatalf("upstream hits %d, want 1", got)
				}
				record := h.sink.waitFinal(t, res.callID)
				if record.Status != tc.wantStatus || record.ProviderStatusClass != tc.wantClass || record.ErrorClass != tc.wantError {
					t.Fatalf("record status %s class %s error %s", record.Status, record.ProviderStatusClass, record.ErrorClass)
				}
				if record.RequestBytes <= 0 || record.ResponseBytes != expectedResponseBytes(res.resp, res.body) {
					t.Errorf("bytes %d/%d", record.RequestBytes, record.ResponseBytes)
				}
			}
		})
	}
}

func TestRoundRobinAcrossAccounts(t *testing.T) {
	h := newHarness(t)
	up := openAIUpstream(t, "ok")
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	h.addCredential(compatCredential(resourceID, up.URL(), "sk-one", "m1"))
	h.addCredential(compatCredential(resourceID, up.URL(), "sk-two", "m1"))
	for range 6 {
		if res := h.call(http.MethodPost, "/r/"+resourceID+"/v1/chat/completions", key, chatBody, nil); res.resp.StatusCode != http.StatusOK {
			t.Fatalf("status %d", res.resp.StatusCode)
		}
	}
	counts := map[string]int{}
	for _, auth := range up.authsSeen() {
		counts[auth]++
	}
	if counts["Bearer sk-one"] != 3 || counts["Bearer sk-two"] != 3 {
		t.Fatalf("distribution %v", counts)
	}
}

func TestCrossResourceIsolation(t *testing.T) {
	h := newHarness(t)
	upA := openAIUpstream(t, "from A")
	upB := openAIUpstream(t, "from B")
	resourceA, resourceB := newResourceID(), newResourceID()
	keyA, keyB := h.addKey(resourceA), h.addKey(resourceB)
	h.addCredential(compatCredential(resourceA, upA.URL(), "sk-a", "shared", "only-a"))
	h.addCredential(compatCredential(resourceB, upB.URL(), "sk-b", "shared"))

	// A's key is not valid for B.
	res := h.call(http.MethodPost, "/r/"+resourceB+"/v1/chat/completions", keyA, `{"model":"shared","messages":[]}`, nil)
	if res.resp.StatusCode != http.StatusUnauthorized || gjson.GetBytes(res.body, "error.type").String() != "invalid_call_key" {
		t.Fatalf("cross-resource key: %d %s", res.resp.StatusCode, res.body)
	}
	// B cannot reach a model only A's credentials serve.
	res = h.call(http.MethodPost, "/r/"+resourceB+"/v1/chat/completions", keyB, `{"model":"only-a","messages":[]}`, nil)
	if res.resp.StatusCode != http.StatusBadRequest || gjson.GetBytes(res.body, "error.type").String() != "no_credential" {
		t.Fatalf("model of another resource: %d %s", res.resp.StatusCode, res.body)
	}
	if upA.hits.Load() != 0 || upB.hits.Load() != 0 {
		t.Fatalf("rejected calls reached upstreams: A=%d B=%d", upA.hits.Load(), upB.hits.Load())
	}
	for range 3 {
		if res := h.call(http.MethodPost, "/r/"+resourceA+"/v1/chat/completions", keyA, `{"model":"shared","messages":[]}`, nil); !strings.Contains(string(res.body), "from A") {
			t.Fatalf("A got %s", res.body)
		}
		if res := h.call(http.MethodPost, "/r/"+resourceB+"/v1/chat/completions", keyB, `{"model":"shared","messages":[]}`, nil); !strings.Contains(string(res.body), "from B") {
			t.Fatalf("B got %s", res.body)
		}
	}
	for _, auth := range upA.authsSeen() {
		if auth != "Bearer sk-a" {
			t.Fatalf("upstream A saw %q", auth)
		}
	}
	for _, auth := range upB.authsSeen() {
		if auth != "Bearer sk-b" {
			t.Fatalf("upstream B saw %q", auth)
		}
	}
	// Listings never show the other resource's models.
	listing := h.call(http.MethodGet, "/r/"+resourceB+"/v1/models", keyB, "", nil)
	if strings.Contains(string(listing.body), "only-a") || !strings.Contains(string(listing.body), "shared") {
		t.Fatalf("listing of B: %s", listing.body)
	}
}

func TestAllowlistRejectsBeforeProviderContact(t *testing.T) {
	h := newHarness(t)
	up := openAIUpstream(t, "ok")
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	h.addCredential(compatCredential(resourceID, up.URL(), "sk", "allowed", "blocked"))
	h.routing.set(resourceID, "allowed")

	cases := []struct {
		path, body, typePath string
		header               map[string]string
	}{
		{"/v1/chat/completions", `{"model":"blocked","messages":[]}`, "error.type", nil},
		{"/v1/messages", `{"model":"blocked","max_tokens":1,"messages":[]}`, "error.type", map[string]string{"Anthropic-Version": "2023-06-01"}},
		{"/v1beta/models/blocked:generateContent", `{"contents":[]}`, "error.details.0.reason", nil},
	}
	for _, tc := range cases {
		res := h.call(http.MethodPost, "/r/"+resourceID+tc.path, key, tc.body, tc.header)
		if res.resp.StatusCode != http.StatusForbidden || gjson.GetBytes(res.body, tc.typePath).String() != "model_not_allowed" {
			t.Fatalf("%s: %d %s", tc.path, res.resp.StatusCode, res.body)
		}
		if res.callID == "" {
			t.Fatalf("%s: missing %s", tc.path, RequestIDHeader)
		}
		record := h.sink.waitFinal(t, res.callID)
		if record.Status != mcpav1.CallStatus_CALL_STATUS_REJECTED_MODEL_NOT_ALLOWED || record.RequestBytes != 0 {
			t.Fatalf("%s: record %+v", tc.path, record)
		}
	}
	if claude := h.call(http.MethodPost, "/r/"+resourceID+"/v1/messages", key, `{"model":"blocked","messages":[]}`, nil); gjson.GetBytes(claude.body, "type").String() != "error" {
		t.Fatalf("claude error shape %s", claude.body)
	}
	if up.hits.Load() != 0 {
		t.Fatalf("upstream hits %d", up.hits.Load())
	}
	if res := h.call(http.MethodPost, "/r/"+resourceID+"/v1/chat/completions", key, `{"model":"allowed","messages":[]}`, nil); res.resp.StatusCode != http.StatusOK {
		t.Fatalf("allowed model: %d %s", res.resp.StatusCode, res.body)
	}
	listing := h.call(http.MethodGet, "/r/"+resourceID+"/v1/models", key, "", nil)
	if strings.Contains(string(listing.body), "blocked") || !strings.Contains(string(listing.body), "allowed") {
		t.Fatalf("listing %s", listing.body)
	}
}

// modelRecordingUpstream answers chat completions and records the model name
// of every request body it receives.
func modelRecordingUpstream(t *testing.T) (*upstream, func() []string) {
	t.Helper()
	var mu sync.Mutex
	var models []string
	up := newUpstream(t, func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		model := gjson.GetBytes(body, "model").String()
		mu.Lock()
		models = append(models, model)
		mu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, openAIChatResponse(model, "ok"))
	})
	return up, func() []string {
		mu.Lock()
		defer mu.Unlock()
		return append([]string(nil), models...)
	}
}

func TestAllowlistMatchesExactModelStringOnly(t *testing.T) {
	h := newHarness(t)
	up, seen := modelRecordingUpstream(t)
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	h.addCredential(compatCredential(resourceID, up.URL(), "sk", "gpt-4o"))
	h.routing.set(resourceID, "gpt-4o")

	for _, model := range []string{"gpt-4o(extra)", "gpt-4o(high)", "gpt-4o(8192)"} {
		res := h.call(http.MethodPost, "/r/"+resourceID+"/v1/chat/completions", key, `{"model":"`+model+`","messages":[]}`, nil)
		if res.resp.StatusCode != http.StatusForbidden || gjson.GetBytes(res.body, "error.type").String() != "model_not_allowed" {
			t.Fatalf("%s: %d %s", model, res.resp.StatusCode, res.body)
		}
		record := h.sink.waitFinal(t, res.callID)
		if record.Status != mcpav1.CallStatus_CALL_STATUS_REJECTED_MODEL_NOT_ALLOWED {
			t.Fatalf("%s: record %+v", model, record)
		}
	}
	if up.hits.Load() != 0 {
		t.Fatalf("suffixed names reached the provider: %d hits", up.hits.Load())
	}

	res := h.call(http.MethodPost, "/r/"+resourceID+"/v1/chat/completions", key, `{"model":"gpt-4o","messages":[]}`, nil)
	if res.resp.StatusCode != http.StatusOK {
		t.Fatalf("exact model: %d %s", res.resp.StatusCode, res.body)
	}
	if got := seen(); len(got) != 1 || got[0] != "gpt-4o" {
		t.Fatalf("provider models %v", got)
	}

	// An allowlist entry that itself carries a suffix still matches exactly.
	h.routing.set(resourceID, "gpt-4o(high)")
	res = h.call(http.MethodPost, "/r/"+resourceID+"/v1/chat/completions", key, `{"model":"gpt-4o(high)","messages":[]}`, nil)
	if res.resp.StatusCode != http.StatusOK {
		t.Fatalf("suffixed entry: %d %s", res.resp.StatusCode, res.body)
	}
	res = h.call(http.MethodPost, "/r/"+resourceID+"/v1/chat/completions", key, `{"model":"gpt-4o","messages":[]}`, nil)
	if res.resp.StatusCode != http.StatusForbidden {
		t.Fatalf("base name against suffixed entry: %d %s", res.resp.StatusCode, res.body)
	}

	// An empty allowlist stays unrestricted, suffixed names included.
	h.routing.set(resourceID)
	res = h.call(http.MethodPost, "/r/"+resourceID+"/v1/chat/completions", key, `{"model":"gpt-4o(extra)","messages":[]}`, nil)
	if res.resp.StatusCode != http.StatusOK {
		t.Fatalf("empty allowlist: %d %s", res.resp.StatusCode, res.body)
	}
}

func TestHasDuplicateTopLevelKey(t *testing.T) {
	cases := []struct {
		name string
		body string
		want bool
	}{
		{"single key", `{"model":"alpha","messages":[]}`, false},
		{"two top-level keys", `{"model":"alpha","messages":[],"model":"beta"}`, true},
		{"same value twice", `{"model":"alpha","model":"alpha"}`, true},
		{"escaped duplicate", `{"model":"alpha","mo\u0064el":"beta"}`, true},
		{"case-folded duplicate", `{"model":"alpha","Model":"beta"}`, true},
		{"nested key only", `{"model":"alpha","metadata":{"model":"beta"},"messages":[{"model":"gamma"}]}`, false},
		{"nested duplicate only", `{"model":"alpha","metadata":{"model":"a","model":"b"}}`, false},
		{"no model", `{"messages":[]}`, false},
		{"array body", `[{"model":"a"},{"model":"b"}]`, false},
		{"not json", `model=a&model=b`, false},
		{"empty", ``, false},
	}
	for _, tc := range cases {
		if got := hasDuplicateTopLevelKey([]byte(tc.body), "model"); got != tc.want {
			t.Errorf("%s: got %v, want %v", tc.name, got, tc.want)
		}
	}
}

func TestDuplicateModelKeyRejectedBeforeAllowlist(t *testing.T) {
	const duplicate = `{"model":"alpha","messages":[],"model":"beta"}`
	cases := []struct {
		path, typePath string
		header         map[string]string
	}{
		{"/v1/chat/completions", "error.type", nil},
		{"/v1/completions", "error.type", nil},
		{"/v1/responses", "error.type", nil},
		{"/v1/messages", "error.type", map[string]string{"Anthropic-Version": "2023-06-01"}},
		{"/v1/messages/count_tokens", "error.type", map[string]string{"Anthropic-Version": "2023-06-01"}},
	}
	for _, allowlist := range [][]string{{"alpha"}, {"alpha", "beta"}, nil} {
		h := newHarness(t)
		up, seen := modelRecordingUpstream(t)
		resourceID := newResourceID()
		key := h.addKey(resourceID)
		h.addCredential(compatCredential(resourceID, up.URL(), "sk", "alpha", "beta"))
		h.routing.set(resourceID, allowlist...)

		for _, tc := range cases {
			res := h.call(http.MethodPost, "/r/"+resourceID+tc.path, key, duplicate, tc.header)
			if res.resp.StatusCode != http.StatusForbidden || gjson.GetBytes(res.body, tc.typePath).String() != "model_not_allowed" {
				t.Fatalf("allowlist %v %s: %d %s", allowlist, tc.path, res.resp.StatusCode, res.body)
			}
			if strings.Contains(string(res.body), "alpha") || strings.Contains(string(res.body), "beta") {
				t.Fatalf("allowlist %v %s: rejection echoes a model: %s", allowlist, tc.path, res.body)
			}
			record := h.sink.waitFinal(t, res.callID)
			if record.Status != mcpav1.CallStatus_CALL_STATUS_REJECTED_MODEL_NOT_ALLOWED || record.Model != "" {
				t.Fatalf("allowlist %v %s: record %+v", allowlist, tc.path, record)
			}
			// Only the customer-segment response bytes are billed.
			if reqBytes, respBytes := h.sink.sums(res.callID); reqBytes != 0 || respBytes != expectedResponseBytes(res.resp, res.body) {
				t.Fatalf("allowlist %v %s: billed request %d response %d", allowlist, tc.path, reqBytes, respBytes)
			}
		}
		if up.hits.Load() != 0 || len(seen()) != 0 {
			t.Fatalf("allowlist %v: duplicate-key bodies reached the provider", allowlist)
		}
	}
}

func TestSingleAndNestedModelKeysBehaveAsBefore(t *testing.T) {
	h := newHarness(t)
	up, seen := modelRecordingUpstream(t)
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	h.addCredential(compatCredential(resourceID, up.URL(), "sk", "alpha", "beta"))
	h.routing.set(resourceID, "alpha")

	for _, body := range []string{
		`{"model":"alpha","messages":[]}`,
		`{"model":"alpha","metadata":{"model":"beta"},"messages":[]}`,
	} {
		res := h.call(http.MethodPost, "/r/"+resourceID+"/v1/chat/completions", key, body, nil)
		if res.resp.StatusCode != http.StatusOK {
			t.Fatalf("%s: %d %s", body, res.resp.StatusCode, res.body)
		}
	}
	if got := seen(); len(got) != 2 || got[0] != "alpha" || got[1] != "alpha" {
		t.Fatalf("provider models %v", got)
	}
	res := h.call(http.MethodPost, "/r/"+resourceID+"/v1/chat/completions", key, `{"model":"beta","messages":[]}`, nil)
	if res.resp.StatusCode != http.StatusForbidden {
		t.Fatalf("unlisted single key: %d %s", res.resp.StatusCode, res.body)
	}
}

func TestBudgetGateRunsAfterModelAllowlist(t *testing.T) {
	reg := prometheus.NewRegistry()
	metrics, err := NewMetrics(reg, nil)
	if err != nil {
		t.Fatal(err)
	}
	h := newHarness(t, func(o *Options) { o.Metrics = metrics })
	up, seen := modelRecordingUpstream(t)
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	h.addCredential(compatCredential(resourceID, up.URL(), "sk", "a", "b"))
	h.routing.set(resourceID, "a")
	chat := "/r/" + resourceID + "/v1/chat/completions"
	exhausted := &Rejection{Code: RejectionBudgetExhausted, Message: "the resource budget is exhausted", BudgetScope: BudgetScopeResource}
	budgetRejections := func() float64 {
		return testutil.ToFloat64(metrics.budgetRejections.WithLabelValues("resource")) +
			testutil.ToFloat64(metrics.budgetRejections.WithLabelValues("tenant"))
	}
	type result struct {
		status int
		code   string
		call   mcpav1.CallStatus
		gates  []string
	}
	send := func(path, body string) result {
		t.Helper()
		h.authz.reset()
		res := h.call(http.MethodPost, path, key, body, nil)
		record := h.sink.waitFinal(t, res.callID)
		return result{res.resp.StatusCode, gjson.GetBytes(res.body, "error.type").String(), record.Status, h.authz.gates()}
	}
	assertResult := func(name string, got, want result) {
		t.Helper()
		if got.status != want.status || got.code != want.code || got.call != want.call || strings.Join(got.gates, ",") != strings.Join(want.gates, ",") {
			t.Fatalf("%s: got %+v, want %+v", name, got, want)
		}
	}
	disallowed := result{http.StatusForbidden, "model_not_allowed", mcpav1.CallStatus_CALL_STATUS_REJECTED_MODEL_NOT_ALLOWED, []string{"authorize"}}

	// An exhausted budget never masks a disallowed model, and a disallowed
	// model never counts as a budget rejection.
	h.authz.setBudget(exhausted)
	assertResult("disallowed model, exhausted budget", send(chat, `{"model":"b","messages":[]}`), disallowed)
	assertResult("missing model, exhausted budget", send(chat, `{"messages":[]}`), disallowed)
	assertResult("duplicate model key, exhausted budget", send(chat, `{"model":"a","model":"b"}`), disallowed)
	if got := budgetRejections(); got != 0 {
		t.Fatalf("budget rejections after disallowed models: %v", got)
	}

	// An allowed model on the same resource still meets the budget gate.
	assertResult("allowed model, exhausted budget",
		send(chat, `{"model":"a","messages":[]}`),
		result{http.StatusTooManyRequests, "budget_exhausted", mcpav1.CallStatus_CALL_STATUS_REJECTED_BUDGET, []string{"authorize", "budgets"}})
	if got := testutil.ToFloat64(metrics.budgetRejections.WithLabelValues("resource")); got != 1 {
		t.Fatalf("resource budget rejections %v", got)
	}

	// Model listings keep the budget gate.
	h.authz.reset()
	if res := h.call(http.MethodGet, "/r/"+resourceID+"/v1/models", key, "", nil); res.resp.StatusCode != http.StatusTooManyRequests {
		t.Fatalf("listing with exhausted budget: %d %s", res.resp.StatusCode, res.body)
	}

	// An unreadable billing period fails closed only for models that reach the
	// budget gate.
	h.authz.setBudget(Reject(RejectionDependencyUnavailable, "a required service is unavailable; retry later"))
	assertResult("disallowed model, unreadable period", send(chat, `{"model":"b","messages":[]}`), disallowed)
	assertResult("allowed model, unreadable period",
		send(chat, `{"model":"a","messages":[]}`),
		result{http.StatusServiceUnavailable, "dependency_unavailable", mcpav1.CallStatus_CALL_STATUS_REJECTED_DEPENDENCY, []string{"authorize", "budgets"}})
	if up.hits.Load() != 0 {
		t.Fatalf("rejected calls reached the provider: %d hits", up.hits.Load())
	}

	// The authorization gate still runs first, before the body is considered.
	h.authz.setBudget(nil)
	h.authz.set(Reject(RejectionTenantInactive, "tenant is blocked"))
	assertResult("blocked tenant, disallowed model",
		send(chat, `{"model":"b","messages":[]}`),
		result{http.StatusForbidden, "tenant_inactive", mcpav1.CallStatus_CALL_STATUS_REJECTED_TENANT, []string{"authorize"}})
	h.authz.set(nil)

	// Without a budget the call proceeds as before.
	assertResult("allowed model, no budget",
		send(chat, `{"model":"a","messages":[]}`),
		result{http.StatusOK, "", mcpav1.CallStatus_CALL_STATUS_SUCCEEDED, []string{"authorize", "budgets"}})
	if got := seen(); len(got) != 1 || got[0] != "a" {
		t.Fatalf("provider models %v", got)
	}
	// One rejected call and one rejected listing, nothing else.
	if got := budgetRejections(); got != 2 {
		t.Fatalf("budget rejections %v", got)
	}
}

func TestRemoveCredentialTakesEffectImmediately(t *testing.T) {
	h := newHarness(t)
	up := openAIUpstream(t, "ok")
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	one := compatCredential(resourceID, up.URL(), "sk-one", "m1")
	two := compatCredential(resourceID, up.URL(), "sk-two", "m1")
	h.addCredential(one)
	h.addCredential(two)
	call := func() callResult {
		return h.call(http.MethodPost, "/r/"+resourceID+"/v1/chat/completions", key, chatBody, nil)
	}
	call()

	h.creds.remove(resourceID, one.ID)
	if err := h.engine.RemoveCredential(context.Background(), resourceID, one.ID); err != nil {
		t.Fatal(err)
	}
	before := len(up.authsSeen())
	for range 3 {
		if res := call(); res.resp.StatusCode != http.StatusOK {
			t.Fatalf("status %d", res.resp.StatusCode)
		}
	}
	for _, auth := range up.authsSeen()[before:] {
		if auth != "Bearer sk-two" {
			t.Fatalf("removed credential used: %q", auth)
		}
	}

	h.creds.remove(resourceID, two.ID)
	if err := h.engine.RemoveCredential(context.Background(), resourceID, two.ID); err != nil {
		t.Fatal(err)
	}
	hits := up.hits.Load()
	res := call()
	if res.resp.StatusCode != http.StatusBadRequest || up.hits.Load() != hits {
		t.Fatalf("after removing all credentials: %d %s", res.resp.StatusCode, res.body)
	}
	if record := h.sink.waitFinal(t, res.callID); record.Status != mcpav1.CallStatus_CALL_STATUS_NO_CREDENTIAL {
		t.Fatalf("status %s", record.Status)
	}

	h.addCredential(compatCredential(resourceID, up.URL(), "sk-three", "m1"))
	if res := call(); res.resp.StatusCode != http.StatusOK {
		t.Fatalf("re-added credential: %d", res.resp.StatusCode)
	}
	if auths := up.authsSeen(); auths[len(auths)-1] != "Bearer sk-three" {
		t.Fatalf("last auth %q", auths[len(auths)-1])
	}
}

func TestCallKeyLocationsAreAcceptedAndNeverForwarded(t *testing.T) {
	h := newHarness(t)
	up := openAIUpstream(t, "ok")
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	h.addCredential(compatCredential(resourceID, up.URL(), "sk-up", "m1"))
	base := "/r/" + resourceID + "/v1/chat/completions"
	requests := []struct {
		path   string
		header map[string]string
	}{
		{base, map[string]string{"Authorization": "Bearer " + key}},
		{base, map[string]string{"X-Api-Key": key}},
		{base, map[string]string{"X-Goog-Api-Key": key}},
		{base + "?key=" + key, nil},
	}
	for _, r := range requests {
		res := h.call(http.MethodPost, r.path, "", chatBody, r.header)
		if res.resp.StatusCode != http.StatusOK {
			t.Fatalf("%v: %d %s", r.header, res.resp.StatusCode, res.body)
		}
	}
	for i, header := range up.headersSeen() {
		for name, values := range header {
			for _, value := range values {
				if strings.Contains(value, key) {
					t.Fatalf("request %d forwarded the call key in %s", i, name)
				}
			}
		}
	}
	up.mu.Lock()
	defer up.mu.Unlock()
	for _, path := range up.paths {
		if strings.Contains(path, key) {
			t.Fatalf("call key forwarded in path %s", path)
		}
	}
}

func TestRejectionsUseClientFormatShapes(t *testing.T) {
	reg := prometheus.NewRegistry()
	metrics, err := NewMetrics(reg, nil)
	if err != nil {
		t.Fatal(err)
	}
	h := newHarness(t, func(o *Options) { o.Metrics = metrics })
	up := openAIUpstream(t, "ok")
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	h.addCredential(compatCredential(resourceID, up.URL(), "sk", "m1"))
	openai := "/r/" + resourceID + "/v1/chat/completions"

	// Missing key, in each format.
	if res := h.call(http.MethodPost, openai, "", chatBody, nil); res.resp.StatusCode != 401 || gjson.GetBytes(res.body, "error.code").String() != "invalid_call_key" || res.callID == "" {
		t.Fatalf("openai missing key: %d %s", res.resp.StatusCode, res.body)
	}
	if res := h.call(http.MethodPost, "/r/"+resourceID+"/v1/messages", "", `{"model":"m1"}`, nil); res.resp.StatusCode != 401 || gjson.GetBytes(res.body, "error.type").String() != "invalid_call_key" || gjson.GetBytes(res.body, "type").String() != "error" {
		t.Fatalf("claude missing key: %d %s", res.resp.StatusCode, res.body)
	}
	if res := h.call(http.MethodPost, "/r/"+resourceID+"/v1beta/models/m1:generateContent", "", `{}`, nil); res.resp.StatusCode != 401 || gjson.GetBytes(res.body, "error.status").String() != "UNAUTHENTICATED" {
		t.Fatalf("gemini missing key: %d %s", res.resp.StatusCode, res.body)
	}

	checks := []struct {
		err    error
		status int
		code   string
		call   mcpav1.CallStatus
	}{
		{Reject(RejectionTenantInactive, "tenant is blocked"), 403, "tenant_inactive", mcpav1.CallStatus_CALL_STATUS_REJECTED_TENANT},
		{&Rejection{Code: RejectionBudgetExhausted, Message: "budget exhausted", BudgetScope: BudgetScopeTenant}, 429, "budget_exhausted", mcpav1.CallStatus_CALL_STATUS_REJECTED_BUDGET},
		{errors.New("delcore timeout"), 503, "dependency_unavailable", mcpav1.CallStatus_CALL_STATUS_REJECTED_DEPENDENCY},
	}
	for _, c := range checks {
		// Budget rejections come from the budget gate, which runs after the
		// allowlist; the other rejections come from the authorization gate.
		if c.call == mcpav1.CallStatus_CALL_STATUS_REJECTED_BUDGET {
			h.authz.set(nil)
			h.authz.setBudget(c.err)
		} else {
			h.authz.set(c.err)
			h.authz.setBudget(nil)
		}
		res := h.call(http.MethodPost, openai, key, chatBody, nil)
		if res.resp.StatusCode != c.status || gjson.GetBytes(res.body, "error.type").String() != c.code {
			t.Fatalf("%s: %d %s", c.code, res.resp.StatusCode, res.body)
		}
		if strings.Contains(string(res.body), "delcore timeout") {
			t.Fatalf("internal error leaked: %s", res.body)
		}
		if record := h.sink.waitFinal(t, res.callID); record.Status != c.call {
			t.Fatalf("%s: record status %s", c.code, record.Status)
		}
	}
	if up.hits.Load() != 0 {
		t.Fatalf("upstream hits %d", up.hits.Load())
	}
	if got := testutil.ToFloat64(metrics.budgetRejections.WithLabelValues("tenant")); got != 1 {
		t.Fatalf("budget rejections %v", got)
	}
	if got := testutil.ToFloat64(metrics.calls.WithLabelValues("openai", "unspecified", "rejected_tenant")); got != 1 {
		t.Fatalf("calls_total rejected_tenant %v", got)
	}
}

func TestModelListingsArePerResourceAndFormatted(t *testing.T) {
	h := newHarness(t)
	up := openAIUpstream(t, "ok")
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	h.addCredential(compatCredential(resourceID, up.URL(), "sk", "alpha", "beta"))
	base := "/r/" + resourceID

	openai := h.call(http.MethodGet, base+"/v1/models", key, "", nil)
	if gjson.GetBytes(openai.body, "object").String() != "list" || gjson.GetBytes(openai.body, "data.#").Int() != 2 {
		t.Fatalf("openai listing %s", openai.body)
	}
	claude := h.call(http.MethodGet, base+"/v1/models", key, "", map[string]string{"Anthropic-Version": "2023-06-01"})
	if gjson.GetBytes(claude.body, "data.0.type").String() != "model" || gjson.GetBytes(claude.body, "first_id").String() != "alpha" {
		t.Fatalf("claude listing %s", claude.body)
	}
	gemini := h.call(http.MethodGet, base+"/v1beta/models", key, "", nil)
	if gjson.GetBytes(gemini.body, "models.1.name").String() != "models/beta" {
		t.Fatalf("gemini listing %s", gemini.body)
	}
	one := h.call(http.MethodGet, base+"/v1beta/models/alpha", key, "", nil)
	if gjson.GetBytes(one.body, "name").String() != "models/alpha" {
		t.Fatalf("gemini get %s", one.body)
	}
	if missing := h.call(http.MethodGet, base+"/v1beta/models/gamma", key, "", nil); missing.resp.StatusCode != 404 {
		t.Fatalf("missing model %d", missing.resp.StatusCode)
	}
	if up.hits.Load() != 0 {
		t.Fatalf("listing contacted the provider")
	}
	record := h.sink.waitFinal(t, openai.callID)
	if record.Status != mcpav1.CallStatus_CALL_STATUS_SUCCEEDED || record.ResponseBytes != expectedResponseBytes(openai.resp, openai.body) {
		t.Fatalf("listing record %+v", record)
	}
	if res := h.call(http.MethodGet, base+"/v1/responses", key, "", nil); res.resp.StatusCode != http.StatusNotFound {
		t.Fatalf("websocket responses route exposed: %d", res.resp.StatusCode)
	}
}

func TestPeriodicCheckpointsSumToTotals(t *testing.T) {
	h := newHarness(t, func(o *Options) { o.CheckpointInterval = 15 * time.Millisecond })
	up := newUpstream(t, func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.ReadAll(r.Body)
		w.Header().Set("Content-Type", "text/event-stream")
		for range 6 {
			_, _ = io.WriteString(w, openAIChunk("m1", "tick ", false))
			w.(http.Flusher).Flush()
			time.Sleep(40 * time.Millisecond)
		}
		_, _ = io.WriteString(w, openAIChunk("m1", "", true))
		_, _ = io.WriteString(w, "data: [DONE]\n\n")
	})
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	h.addCredential(compatCredential(resourceID, up.URL(), "sk", "m1"))
	res := h.call(http.MethodPost, "/r/"+resourceID+"/v1/chat/completions", key, streamBody("m1"), nil)
	record := h.sink.waitFinal(t, res.callID)

	committed := h.sink.committed(res.callID)
	if len(committed) < 3 {
		t.Fatalf("only %d checkpoints", len(committed))
	}
	finals := 0
	for i, entry := range committed {
		if entry.record.CheckpointSeq != int64(i+1) {
			t.Fatalf("sequence %d at %d", entry.record.CheckpointSeq, i)
		}
		if entry.final {
			finals++
		}
	}
	if finals != 1 || !committed[len(committed)-1].final {
		t.Fatalf("final checkpoints %d", finals)
	}
	req, resp := h.sink.sums(res.callID)
	if req != record.RequestBytes || resp != record.ResponseBytes || resp != expectedResponseBytes(res.resp, res.body) || req != up.read.Load() {
		t.Fatalf("sums %d/%d totals %d/%d", req, resp, record.RequestBytes, record.ResponseBytes)
	}
}

func TestFailedCheckpointIsResentWithSameSequence(t *testing.T) {
	h := newHarness(t)
	up := openAIUpstream(t, "ok")
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	h.addCredential(compatCredential(resourceID, up.URL(), "sk", "m1"))
	h.sink.failNext.Store(1)
	res := h.call(http.MethodPost, "/r/"+resourceID+"/v1/chat/completions", key, chatBody, nil)
	record := h.sink.waitFinal(t, res.callID)

	h.sink.mu.Lock()
	var attempts []checkpointEntry
	for _, entry := range h.sink.entries {
		if entry.callID == res.callID {
			attempts = append(attempts, entry)
		}
	}
	h.sink.mu.Unlock()
	if len(attempts) != 2 || attempts[0].record.CheckpointSeq != attempts[1].record.CheckpointSeq ||
		attempts[0].deltaReq != attempts[1].deltaReq || attempts[0].deltaResp != attempts[1].deltaResp {
		t.Fatalf("attempts %+v", attempts)
	}
	if req, resp := h.sink.sums(res.callID); req != record.RequestBytes || resp != record.ResponseBytes {
		t.Fatalf("sums %d/%d", req, resp)
	}
}
