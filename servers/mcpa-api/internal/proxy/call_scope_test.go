// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"bytes"
	"context"
	"encoding/json"
	"log/slog"
	"net/http"
	"strings"
	"sync"
	"testing"
	"time"
	"unicode/utf8"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/testutil"
	"github.com/tidwall/gjson"
)

// logCapture collects the lines of a JSON slog handler.
type logCapture struct {
	mu  sync.Mutex
	buf bytes.Buffer
}

func (c *logCapture) Write(p []byte) (int, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.buf.Write(p)
}

func (c *logCapture) logger() *slog.Logger {
	return slog.New(slog.NewJSONHandler(c, nil))
}

// lines returns the captured JSON lines.
func (c *logCapture) lines() []string {
	c.mu.Lock()
	defer c.mu.Unlock()
	var out []string
	for _, line := range strings.Split(c.buf.String(), "\n") {
		if line != "" {
			out = append(out, line)
		}
	}
	return out
}

// waitCallLog waits for the engine's final log line of a call. The handler
// writes it after the last metering step, so it marks the end of the call.
func (c *logCapture) waitCallLog(t *testing.T, callID string) map[string]any {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		for _, line := range c.lines() {
			var entry map[string]any
			if err := json.Unmarshal([]byte(line), &entry); err != nil {
				t.Fatalf("log line is not JSON: %v", err)
			}
			msg, _ := entry["msg"].(string)
			if entry["call_id"] == callID && strings.HasPrefix(msg, "model call") {
				if len(line) >= 2048 {
					t.Fatalf("log line is %d bytes, want under 2 KiB", len(line))
				}
				return entry
			}
		}
		time.Sleep(5 * time.Millisecond)
	}
	t.Fatalf("no call log line for %s", callID)
	return nil
}

// errRouting fails every allowlist read.
type errRouting struct{ err error }

func (r errRouting) ModelAllowlist(context.Context, string) ([]string, error) { return nil, r.err }

// TestInvalidCallKeyAfterCallStateIsNotRecorded covers the 401 rejections that
// are raised after the call state exists: they are not attributable to a valid
// key, so they leave no call record, bytes or call metric, while the response
// still carries the request ID.
func TestInvalidCallKeyAfterCallStateIsNotRecorded(t *testing.T) {
	cases := []struct {
		name  string
		setup func(h *harness)
		opt   harnessOption
	}{
		{name: "permission denial", setup: func(h *harness) {
			h.authz.set(Reject(RejectionInvalidCallKey, "the call key is not valid"))
		}},
		{name: "resource gone at the allowlist read", opt: func(o *Options) {
			o.Routing = errRouting{err: Reject(RejectionInvalidCallKey, "the call key is not valid")}
		}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			reg := prometheus.NewRegistry()
			metrics, err := NewMetrics(reg, nil)
			if err != nil {
				t.Fatal(err)
			}
			logs := &logCapture{}
			options := []harnessOption{func(o *Options) {
				o.Metrics = metrics
				o.Logger = logs.logger()
			}}
			if tc.opt != nil {
				options = append(options, tc.opt)
			}
			h := newHarness(t, options...)
			up := openAIUpstream(t, "ok")
			resourceID := newResourceID()
			key := h.addKey(resourceID)
			h.addCredential(compatCredential(resourceID, up.URL(), "sk", "m1"))
			if tc.setup != nil {
				tc.setup(h)
			}

			res := h.call(http.MethodPost, "/r/"+resourceID+"/v1/chat/completions", key, `{"model":"m1","messages":[]}`, nil)
			if res.resp.StatusCode != http.StatusUnauthorized || gjson.GetBytes(res.body, "error.type").String() != "invalid_call_key" {
				t.Fatalf("%d %s", res.resp.StatusCode, res.body)
			}
			if res.callID == "" {
				t.Fatalf("missing %s", RequestIDHeader)
			}
			entry := logs.waitCallLog(t, res.callID)
			if entry["error_class"] != string(ErrorClassInvalidCallKey) || entry["http_status"] != float64(http.StatusUnauthorized) {
				t.Fatalf("log line %v", entry)
			}
			if h.sink.count() != 0 {
				t.Fatalf("invalid-key rejection was checkpointed: %+v", h.sink.entries)
			}
			if got := testutil.CollectAndCount(metrics.calls); got != 0 {
				t.Fatalf("mcpa_calls_total has %d samples", got)
			}
			if up.hits.Load() != 0 {
				t.Fatalf("upstream hits %d", up.hits.Load())
			}
		})
	}
}

// TestRejectionsAfterValidKeyAreStillRecorded keeps the recorded and billed
// contract for every rejection that follows a valid key.
func TestRejectionsAfterValidKeyAreStillRecorded(t *testing.T) {
	h := newHarness(t)
	up := openAIUpstream(t, "ok")
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	h.addCredential(compatCredential(resourceID, up.URL(), "sk", "m1"))
	h.authz.set(Reject(RejectionTenantInactive, "tenant is blocked"))

	res := h.call(http.MethodPost, "/r/"+resourceID+"/v1/chat/completions", key, `{"model":"m1","messages":[]}`, nil)
	if res.resp.StatusCode != http.StatusForbidden {
		t.Fatalf("%d %s", res.resp.StatusCode, res.body)
	}
	record := h.sink.waitFinal(t, res.callID)
	if record.Status != mcpav1.CallStatus_CALL_STATUS_REJECTED_TENANT {
		t.Fatalf("record %+v", record)
	}
	if got := h.sink.count(); got != 1 {
		t.Fatalf("checkpoints %d", got)
	}
	if _, respBytes := h.sink.sums(res.callID); respBytes != expectedResponseBytes(res.resp, res.body) {
		t.Fatalf("billed response bytes %d", respBytes)
	}
}

func TestOverlongModelNameIsRejectedBeforeAllowlist(t *testing.T) {
	boundary := strings.Repeat("a", maxModelNameBytes-1) + "é" // 201 bytes, split mid-rune at 200
	cases := []struct {
		name  string
		model string
	}{
		{"one byte over", strings.Repeat("a", maxModelNameBytes+1)},
		{"multi-byte boundary", boundary},
		{"4 MiB", strings.Repeat("a", 4<<20)},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			logs := &logCapture{}
			h := newHarness(t, func(o *Options) { o.Logger = logs.logger() })
			up, seen := modelRecordingUpstream(t)
			resourceID := newResourceID()
			key := h.addKey(resourceID)
			h.addCredential(compatCredential(resourceID, up.URL(), "sk", "alpha"))
			// The allowlist is unreachable for an overlong name: the check runs
			// before the allowlist read, whatever the allowlist holds.
			h.routing.set(resourceID, tc.model)

			res := h.call(http.MethodPost, "/r/"+resourceID+"/v1/chat/completions", key, `{"model":"`+tc.model+`","messages":[]}`, nil)
			if res.resp.StatusCode != http.StatusForbidden || gjson.GetBytes(res.body, "error.type").String() != "model_not_allowed" {
				t.Fatalf("%d %.200s", res.resp.StatusCode, res.body)
			}
			if len(res.body) >= 1024 || strings.Contains(string(res.body), "aaaa") {
				t.Fatalf("response is %d bytes or echoes the model: %.200s", len(res.body), res.body)
			}
			if message := gjson.GetBytes(res.body, "error.message").String(); message != "the model name is too long" {
				t.Fatalf("message %q", message)
			}
			entry := logs.waitCallLog(t, res.callID)
			logged, _ := entry["model"].(string)
			if len(logged) > maxModelNameBytes || !utf8.ValidString(logged) || !strings.HasPrefix(tc.model, logged) {
				t.Fatalf("logged model has %d bytes (valid UTF-8 %v)", len(logged), utf8.ValidString(logged))
			}
			record := h.sink.waitFinal(t, res.callID)
			if record.Status != mcpav1.CallStatus_CALL_STATUS_REJECTED_MODEL_NOT_ALLOWED || len(record.Model) > maxModelNameBytes {
				t.Fatalf("record status %s, model %d bytes", record.Status, len(record.Model))
			}
			if up.hits.Load() != 0 || len(seen()) != 0 {
				t.Fatalf("overlong model reached the provider")
			}
		})
	}
}

func TestModelNameAtLimitBehavesAsBefore(t *testing.T) {
	logs := &logCapture{}
	h := newHarness(t, func(o *Options) { o.Logger = logs.logger() })
	up := openAIUpstream(t, "ok")
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	model := strings.Repeat("m", maxModelNameBytes)
	h.addCredential(compatCredential(resourceID, up.URL(), "sk", model))
	h.routing.set(resourceID, model)

	res := h.call(http.MethodPost, "/r/"+resourceID+"/v1/chat/completions", key, `{"model":"`+model+`","messages":[]}`, nil)
	if res.resp.StatusCode != http.StatusOK {
		t.Fatalf("%d %s", res.resp.StatusCode, res.body)
	}
	entry := logs.waitCallLog(t, res.callID)
	if entry["model"] != model {
		t.Fatalf("logged model %v", entry["model"])
	}
	// One byte over is rejected even though a shorter prefix is allowed.
	res = h.call(http.MethodPost, "/r/"+resourceID+"/v1/chat/completions", key, `{"model":"`+model+`m","messages":[]}`, nil)
	if res.resp.StatusCode != http.StatusForbidden {
		t.Fatalf("%d %s", res.resp.StatusCode, res.body)
	}
}

func TestOverlongGeminiPathModelIsRejected(t *testing.T) {
	logs := &logCapture{}
	h := newHarness(t, func(o *Options) { o.Logger = logs.logger() })
	up := openAIUpstream(t, "ok")
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	h.addCredential(compatCredential(resourceID, up.URL(), "sk", "alpha"))
	model := strings.Repeat("g", 8<<10)

	for _, tc := range []struct{ method, path, body string }{
		{http.MethodPost, "/v1beta/models/" + model + ":generateContent", `{"contents":[]}`},
		{http.MethodGet, "/v1beta/models/" + model, ""},
	} {
		res := h.call(tc.method, "/r/"+resourceID+tc.path, key, tc.body, nil)
		if res.resp.StatusCode != http.StatusForbidden || gjson.GetBytes(res.body, "error.details.0.reason").String() != "model_not_allowed" {
			t.Fatalf("%s: %d %.200s", tc.method, res.resp.StatusCode, res.body)
		}
		if len(res.body) >= 1024 || strings.Contains(string(res.body), "gggg") {
			t.Fatalf("%s: response is %d bytes or echoes the model", tc.method, len(res.body))
		}
		entry := logs.waitCallLog(t, res.callID)
		if logged, _ := entry["model"].(string); len(logged) != maxModelNameBytes {
			t.Fatalf("%s: logged model has %d bytes", tc.method, len(logged))
		}
	}
}

func TestTruncateModelNameKeepsUTF8Boundary(t *testing.T) {
	for _, tc := range []struct{ in, want string }{
		{"short", "short"},
		{strings.Repeat("a", maxModelNameBytes), strings.Repeat("a", maxModelNameBytes)},
		{strings.Repeat("a", maxModelNameBytes+5), strings.Repeat("a", maxModelNameBytes)},
		{strings.Repeat("a", maxModelNameBytes-1) + "é", strings.Repeat("a", maxModelNameBytes-1)},
		{strings.Repeat("é", maxModelNameBytes), strings.Repeat("é", maxModelNameBytes/2)},
	} {
		if got := truncateModelName(tc.in); got != tc.want || !utf8.ValidString(got) {
			t.Fatalf("truncate(%d bytes) = %d bytes", len(tc.in), len(got))
		}
	}
}

// TestPeriodicCheckpointSkipsInvalidCallKeyRejection covers the interleaving
// where a checkpoint tick lands after the 401 response bytes are counted but
// before the handler skips the final checkpoint: the tick must not persist a
// non-final record or billable bytes for the invalid key.
func TestPeriodicCheckpointSkipsInvalidCallKeyRejection(t *testing.T) {
	cases := []struct {
		name      string
		rejection *Rejection
		wantSent  int
	}{
		{"invalid call key", Reject(RejectionInvalidCallKey, "the call key is not valid"), 0},
		{"other rejection after a valid key", Reject(RejectionTenantInactive, "tenant is blocked"), 1},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			sink := newRecordingSink()
			metrics, err := NewMetrics(prometheus.NewRegistry(), nil)
			if err != nil {
				t.Fatal(err)
			}
			cp := &checkpointer{
				sink:          sink,
				metrics:       metrics,
				logger:        slog.New(slog.NewTextHandler(bytes.NewBuffer(nil), nil)),
				interval:      time.Hour,
				timeout:       time.Second,
				finalAttempts: 1,
			}
			call := &callState{id: "c-invalid-key"}
			record := func(bool) CallRecordUpdate { return CallRecordUpdate{CallID: call.id} }
			call.reject(tc.rejection)
			// The rejection is set before the response is written.
			call.addResponseBytes(120)

			if !cp.commit(context.Background(), call, record, false) {
				t.Fatal("periodic commit reported failure")
			}
			if got := sink.count(); got != tc.wantSent {
				t.Fatalf("periodic checkpoints %d, want %d: %+v", got, tc.wantSent, sink.entries)
			}
			if tc.wantSent == 0 && (call.checkpoint.billedResp != 0 || call.checkpoint.seq != 0 || call.checkpoint.pending != nil) {
				t.Fatalf("skipped tick changed checkpoint state: seq %d, billed %d, pending %v", call.checkpoint.seq, call.checkpoint.billedResp, call.checkpoint.pending)
			}
		})
	}
}
