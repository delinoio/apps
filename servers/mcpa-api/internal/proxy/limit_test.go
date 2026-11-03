// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"fmt"
	"io"
	"net/http"
	"runtime"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/tidwall/gjson"
)

// testResponseLimit is the lowered Options.MaxProviderResponseBytes of these
// tests; the default is 128 MiB.
const testResponseLimit = 1 << 20

// endlessUpstream writes filler without end and without Content-Length until
// the client goes away.
func endlessUpstream(t *testing.T, contentType string) *upstream {
	return newUpstream(t, func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.Copy(io.Discard, r.Body)
		w.Header().Set("Content-Type", contentType)
		chunk := []byte(strings.Repeat("x", 32<<10))
		for {
			if _, err := w.Write(chunk); err != nil {
				return
			}
			select {
			case <-r.Context().Done():
				return
			default:
			}
		}
	})
}

// peakHeap samples the live heap until stop is called and returns the peak
// growth over the heap at the start.
func peakHeap() (stop func() uint64) {
	runtime.GC()
	var stats runtime.MemStats
	runtime.ReadMemStats(&stats)
	base := stats.HeapAlloc
	var peak atomic.Uint64
	done := make(chan struct{})
	var wg sync.WaitGroup
	wg.Go(func() {
		for {
			var s runtime.MemStats
			runtime.ReadMemStats(&s)
			for {
				old := peak.Load()
				if s.HeapAlloc <= old || peak.CompareAndSwap(old, s.HeapAlloc) {
					break
				}
			}
			select {
			case <-done:
				return
			case <-time.After(2 * time.Millisecond):
			}
		}
	})
	return func() uint64 {
		close(done)
		wg.Wait()
		if peak.Load() < base {
			return 0
		}
		return peak.Load() - base
	}
}

type limitCase struct {
	name     string
	provider mcpav1.Provider
	path     string
	body     string
	header   map[string]string
	format   mcpav1.EndpointFormat
	// errorType reads the error type of the customer response.
	errorType func(body []byte) string
	baseURL   func(up *upstream) string
}

func limitCases() []limitCase {
	same := func(up *upstream) string { return up.URL() }
	return []limitCase{
		{
			name: "openai-compatible", provider: mcpav1.Provider_PROVIDER_OPENAI_COMPATIBLE,
			baseURL: func(up *upstream) string { return up.URL() + "/v1" },
			path:    "/v1/chat/completions",
			body:    `{"model":"m","messages":[{"role":"user","content":"hi"}]}`,
			format:  mcpav1.EndpointFormat_ENDPOINT_FORMAT_OPENAI,
			errorType: func(b []byte) string {
				return gjson.GetBytes(b, "error.type").String()
			},
		},
		{
			name: "claude via relay", provider: mcpav1.Provider_PROVIDER_CLAUDE, baseURL: same,
			path:   "/v1/messages",
			body:   `{"model":"m","max_tokens":16,"messages":[{"role":"user","content":"hi"}]}`,
			header: map[string]string{"Anthropic-Version": "2023-06-01"},
			format: mcpav1.EndpointFormat_ENDPOINT_FORMAT_CLAUDE,
			errorType: func(b []byte) string {
				return gjson.GetBytes(b, "error.type").String()
			},
		},
		{
			name: "gemini", provider: mcpav1.Provider_PROVIDER_GEMINI, baseURL: same,
			path:   "/v1beta/models/m:generateContent",
			body:   `{"contents":[{"role":"user","parts":[{"text":"hi"}]}]}`,
			format: mcpav1.EndpointFormat_ENDPOINT_FORMAT_GEMINI,
			errorType: func(b []byte) string {
				return gjson.GetBytes(b, "error.details.0.reason").String()
			},
		},
	}
}

// An endless provider body must be cut off at the limit: the customer gets a
// 502 provider_error in the format of the endpoint, the call is recorded as a
// provider error of the new class, and the heap stays bounded.
func TestEndlessProviderBodyIsCutOff(t *testing.T) {
	for _, tc := range limitCases() {
		t.Run(tc.name, func(t *testing.T) {
			up := endlessUpstream(t, "application/json")
			h := newHarness(t, func(o *Options) { o.MaxProviderResponseBytes = testResponseLimit })
			resourceID := newResourceID()
			key := h.addKey(resourceID)
			h.addCredential(apiKeyCredential(resourceID, tc.provider, tc.baseURL(up), "sk-test", "m"))

			stop := peakHeap()
			res := h.call(http.MethodPost, "/r/"+resourceID+tc.path, key, tc.body, tc.header)
			growth := stop()
			t.Logf("status %d, peak heap growth %d bytes with a %d byte limit", res.resp.StatusCode, growth, testResponseLimit)

			if res.resp.StatusCode != http.StatusBadGateway {
				t.Fatalf("status %d body %s", res.resp.StatusCode, res.body)
			}
			if got := tc.errorType(res.body); got != "provider_error" {
				t.Fatalf("error type %q in %s", got, res.body)
			}
			if growth > 20<<20 {
				t.Fatalf("heap grew by %d bytes with a %d byte limit", growth, testResponseLimit)
			}
			record := h.sink.waitFinal(t, res.callID)
			if record.Status != mcpav1.CallStatus_CALL_STATUS_PROVIDER_ERROR || record.ErrorClass != ErrorClassProviderResponseTooLarge {
				t.Fatalf("record status %v class %q", record.Status, record.ErrorClass)
			}
			// The customer segment billed is the replacement response.
			if _, resp := h.sink.sums(res.callID); resp != expectedResponseBytes(res.resp, res.body) {
				t.Fatalf("billed %d response bytes, sent %d", resp, expectedResponseBytes(res.resp, res.body))
			}
			if record.CredentialID == "" || record.Provider != tc.provider {
				t.Fatalf("record %+v", record)
			}
		})
	}
}

// Event streams are exempt: a stream larger than the limit still completes.
func TestLargeEventStreamIsNotCutOff(t *testing.T) {
	const events = 96 // about 3 MiB of content
	filler := strings.Repeat("y", 32<<10)

	t.Run("openai-compatible", func(t *testing.T) {
		up := newUpstream(t, func(w http.ResponseWriter, r *http.Request) {
			_, _ = io.Copy(io.Discard, r.Body)
			// A media type parameter must not defeat the exemption.
			w.Header().Set("Content-Type", "Text/Event-Stream; charset=utf-8")
			for range events {
				_, _ = io.WriteString(w, openAIChunk("m", filler, false))
				w.(http.Flusher).Flush()
			}
			_, _ = io.WriteString(w, openAIChunk("m", "", true))
			_, _ = io.WriteString(w, "data: [DONE]\n\n")
		})
		h := newHarness(t, func(o *Options) { o.MaxProviderResponseBytes = testResponseLimit })
		resourceID := newResourceID()
		key := h.addKey(resourceID)
		h.addCredential(compatCredential(resourceID, up.URL(), "sk-test", "m"))
		res := h.call(http.MethodPost, "/r/"+resourceID+"/v1/chat/completions", key,
			`{"model":"m","stream":true,"messages":[{"role":"user","content":"hi"}]}`, nil)
		if res.resp.StatusCode != http.StatusOK || strings.Count(string(res.body), "data: ") < events {
			t.Fatalf("status %d, %d bytes, %d events", res.resp.StatusCode, len(res.body), strings.Count(string(res.body), "data: "))
		}
		if len(res.body) <= testResponseLimit {
			t.Fatalf("stream of %d bytes does not exceed the limit", len(res.body))
		}
		record := h.sink.waitFinal(t, res.callID)
		if record.Status != mcpav1.CallStatus_CALL_STATUS_SUCCEEDED {
			t.Fatalf("record %+v", record)
		}
	})

	t.Run("claude via relay", func(t *testing.T) {
		up := newUpstream(t, func(w http.ResponseWriter, r *http.Request) {
			_, _ = io.Copy(io.Discard, r.Body)
			w.Header().Set("Content-Type", "text/event-stream")
			for _, event := range claudeStreamEvents("m", "") {
				_, _ = io.WriteString(w, event)
			}
			for range events {
				_, _ = io.WriteString(w, fmt.Sprintf("event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":%q}}\n\n", filler))
				w.(http.Flusher).Flush()
			}
		})
		h := newHarness(t, func(o *Options) { o.MaxProviderResponseBytes = testResponseLimit })
		resourceID := newResourceID()
		key := h.addKey(resourceID)
		h.addCredential(apiKeyCredential(resourceID, mcpav1.Provider_PROVIDER_CLAUDE, up.URL(), "sk-ant-test", "m"))
		res := h.call(http.MethodPost, "/r/"+resourceID+"/v1/messages", key,
			`{"model":"m","max_tokens":16,"stream":true,"messages":[{"role":"user","content":"hi"}]}`,
			map[string]string{"Anthropic-Version": "2023-06-01"})
		if res.resp.StatusCode != http.StatusOK || len(res.body) <= testResponseLimit {
			t.Fatalf("status %d, %d bytes", res.resp.StatusCode, len(res.body))
		}
		record := h.sink.waitFinal(t, res.callID)
		if record.ErrorClass == ErrorClassProviderResponseTooLarge {
			t.Fatalf("record %+v", record)
		}
	})
}

// A normal response below the limit is unaffected.
func TestProviderResponseBelowLimitIsServed(t *testing.T) {
	up := openAIUpstream(t, "hello")
	h := newHarness(t, func(o *Options) { o.MaxProviderResponseBytes = testResponseLimit })
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	h.addCredential(compatCredential(resourceID, up.URL(), "sk-test", "m"))
	res := h.call(http.MethodPost, "/r/"+resourceID+"/v1/chat/completions", key,
		`{"model":"m","messages":[{"role":"user","content":"hi"}]}`, nil)
	if res.resp.StatusCode != http.StatusOK || !strings.Contains(string(res.body), "hello") {
		t.Fatalf("status %d body %s", res.resp.StatusCode, res.body)
	}
	if record := h.sink.waitFinal(t, res.callID); record.Status != mcpav1.CallStatus_CALL_STATUS_SUCCEEDED {
		t.Fatalf("record %+v", record)
	}
}

func TestDefaultProviderResponseLimit(t *testing.T) {
	if defaultMaxProviderResponseBytes != 128<<20 {
		t.Fatalf("default limit %d", defaultMaxProviderResponseBytes)
	}
	h := newHarness(t)
	if h.engine.opts.MaxProviderResponseBytes != defaultMaxProviderResponseBytes || h.engine.transport.maxResponseBytes != defaultMaxProviderResponseBytes {
		t.Fatal("the engine does not default the provider response limit")
	}
}

func TestLimitedBodyBoundaries(t *testing.T) {
	read := func(body io.ReadCloser, declared, limit int64) (int, error, bool) {
		call := &callState{}
		data, err := io.ReadAll(&limitedBody{ReadCloser: body, call: call, limit: limit, declared: declared})
		return len(data), err, call.responseTooLarge.Load()
	}
	t.Run("exactly the limit passes", func(t *testing.T) {
		n, err, flagged := read(io.NopCloser(strings.NewReader(strings.Repeat("a", 100))), -1, 100)
		if n != 100 || err != nil || flagged {
			t.Fatalf("read %d err %v flagged %v", n, err, flagged)
		}
	})
	t.Run("one byte over fails", func(t *testing.T) {
		n, err, flagged := read(io.NopCloser(strings.NewReader(strings.Repeat("a", 101))), -1, 100)
		if n != 100 || err != errProviderResponseTooLarge || !flagged {
			t.Fatalf("read %d err %v flagged %v", n, err, flagged)
		}
	})
	t.Run("a declared length over the limit fails before any read", func(t *testing.T) {
		src := &countingSource{}
		n, err, flagged := read(src, 1000, 100)
		if n != 0 || err != errProviderResponseTooLarge || !flagged || src.reads != 0 || !src.closed {
			t.Fatalf("read %d err %v flagged %v reads %d closed %v", n, err, flagged, src.reads, src.closed)
		}
	})
}

type countingSource struct {
	reads  int
	closed bool
}

func (s *countingSource) Read(p []byte) (int, error) { s.reads++; return len(p), nil }
func (s *countingSource) Close() error               { s.closed = true; return nil }

func TestIsEventStream(t *testing.T) {
	for contentType, want := range map[string]bool{
		"text/event-stream":                true,
		"Text/Event-Stream":                true,
		"text/event-stream; charset=utf-8": true,
		" text/event-stream ;q=1":          true,
		"application/json":                 false,
		"":                                 false,
		"text/event-streams":               false,
	} {
		if got := isEventStream(contentType); got != want {
			t.Errorf("isEventStream(%q) = %v", contentType, got)
		}
	}
}

func TestClassifyResponseTooLarge(t *testing.T) {
	out := classify(outcomeInput{executorInvoked: true, providerStatus: http.StatusOK, networkError: true, responseTooLarge: true, customerStatus: http.StatusBadGateway})
	if out.status != mcpav1.CallStatus_CALL_STATUS_PROVIDER_ERROR || out.errorClass != ErrorClassProviderResponseTooLarge {
		t.Fatalf("outcome %+v", out)
	}
	if out.failureClass != "5xx" {
		t.Fatalf("failure class %q", out.failureClass)
	}
	// The caller going away wins over the provider result.
	out = classify(outcomeInput{executorInvoked: true, responseTooLarge: true, clientCancelled: true})
	if out.status != mcpav1.CallStatus_CALL_STATUS_CLIENT_CANCELLED {
		t.Fatalf("outcome %+v", out)
	}
}
