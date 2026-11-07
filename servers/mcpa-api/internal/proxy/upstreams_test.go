// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"bytes"
	"compress/gzip"
	"fmt"
	"io"
	"net/http"
	"strings"
	"testing"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/google/uuid"
	"github.com/tidwall/gjson"
)

func claudeMessageResponse(model, text string) string {
	return fmt.Sprintf(`{"id":"msg_1","type":"message","role":"assistant","model":%q,"content":[{"type":"text","text":%q}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":3,"output_tokens":2}}`, model, text)
}

func claudeStreamEvents(model, text string) []string {
	return []string{
		fmt.Sprintf("event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":%q,\"content\":[],\"stop_reason\":null,\"usage\":{\"input_tokens\":3,\"output_tokens\":0}}}\n\n", model),
		"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
		fmt.Sprintf("event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":%q}}\n\n", text),
		"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
		"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":2}}\n\n",
		"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
	}
}

// claudeUpstream serves the Anthropic Messages API.
func claudeUpstream(t *testing.T, text string) *upstream {
	return newUpstream(t, func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		model := gjson.GetBytes(body, "model").String()
		if strings.HasSuffix(r.URL.Path, "/count_tokens") {
			w.Header().Set("Content-Type", "application/json")
			_, _ = io.WriteString(w, `{"input_tokens":7}`)
			return
		}
		if gjson.GetBytes(body, "stream").Bool() {
			w.Header().Set("Content-Type", "text/event-stream")
			for _, event := range claudeStreamEvents(model, text) {
				_, _ = io.WriteString(w, event)
				w.(http.Flusher).Flush()
			}
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, claudeMessageResponse(model, text))
	})
}

func geminiResponse(text string) string {
	return fmt.Sprintf(`{"candidates":[{"content":{"role":"model","parts":[{"text":%q}]},"finishReason":"STOP","index":0}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2,"totalTokenCount":5},"modelVersion":"gemini-test"}`, text)
}

// geminiUpstream serves the Gemini generateContent API.
func geminiUpstream(t *testing.T, text string) *upstream {
	return newUpstream(t, func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.ReadAll(r.Body)
		switch {
		case strings.Contains(r.URL.Path, ":streamGenerateContent"):
			w.Header().Set("Content-Type", "text/event-stream")
			for _, part := range strings.Split(text, " ") {
				_, _ = io.WriteString(w, "data: "+geminiResponse(part)+"\n\n")
				w.(http.Flusher).Flush()
			}
		case strings.Contains(r.URL.Path, ":countTokens"):
			w.Header().Set("Content-Type", "application/json")
			_, _ = io.WriteString(w, `{"totalTokens":7}`)
		default:
			w.Header().Set("Content-Type", "application/json")
			_, _ = io.WriteString(w, geminiResponse(text))
		}
	})
}

func apiKeyCredential(resourceID string, provider mcpav1.Provider, baseURL, secret string, models ...string) Credential {
	return Credential{
		ID:         uuid.Must(uuid.NewV7()).String(),
		ResourceID: resourceID,
		Provider:   provider,
		Kind:       mcpav1.CredentialKind_CREDENTIAL_KIND_API_KEY,
		Secret:     []byte(secret),
		BaseURL:    baseURL,
		Models:     models,
	}
}

// formatCase is one client request shape.
type formatCase struct {
	name   string
	path   string
	body   string
	header map[string]string
	stream bool
}

func clientFormatCases(model string) []formatCase {
	return []formatCase{
		{name: "openai-chat", path: "/v1/chat/completions", body: `{"model":"` + model + `","messages":[{"role":"user","content":"hi"}]}`},
		{name: "openai-chat-stream", path: "/v1/chat/completions", stream: true, body: `{"model":"` + model + `","stream":true,"messages":[{"role":"user","content":"hi"}]}`},
		{name: "openai-responses", path: "/v1/responses", body: `{"model":"` + model + `","input":"hi"}`},
		{name: "openai-responses-stream", path: "/v1/responses", stream: true, body: `{"model":"` + model + `","stream":true,"input":"hi"}`},
		{name: "claude-messages", path: "/v1/messages", header: map[string]string{"Anthropic-Version": "2023-06-01"}, body: `{"model":"` + model + `","max_tokens":16,"messages":[{"role":"user","content":"hi"}]}`},
		{name: "claude-messages-stream", path: "/v1/messages", stream: true, header: map[string]string{"Anthropic-Version": "2023-06-01"}, body: `{"model":"` + model + `","max_tokens":16,"stream":true,"messages":[{"role":"user","content":"hi"}]}`},
		{name: "gemini-generate", path: "/v1beta/models/" + model + ":generateContent", body: `{"contents":[{"role":"user","parts":[{"text":"hi"}]}]}`},
		{name: "gemini-stream", path: "/v1beta/models/" + model + ":streamGenerateContent?alt=sse", stream: true, body: `{"contents":[{"role":"user","parts":[{"text":"hi"}]}]}`},
	}
}

// runFormatCases calls every client format and checks status, content and
// exact byte accounting against the raw bytes the upstream received.
func runFormatCases(t *testing.T, h *harness, up *upstream, resourceID, key, model, wantText string, cases []formatCase) {
	t.Helper()
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			before := up.read.Load()
			res := h.call(http.MethodPost, "/r/"+resourceID+tc.path, key, tc.body, tc.header)
			if res.resp.StatusCode != http.StatusOK {
				t.Fatalf("status %d body %s", res.resp.StatusCode, res.body)
			}
			if !strings.Contains(string(res.body), wantText) {
				t.Fatalf("response does not contain %q: %s", wantText, res.body)
			}
			record := h.sink.waitFinal(t, res.callID)
			if got := up.read.Load() - before; record.RequestBytes != got {
				t.Errorf("request bytes %d, upstream received %d", record.RequestBytes, got)
			}
			if want := expectedResponseBytes(res.resp, res.body); record.ResponseBytes != want {
				t.Errorf("response bytes %d, want %d", record.ResponseBytes, want)
			}
			if record.Status != mcpav1.CallStatus_CALL_STATUS_SUCCEEDED || record.Streaming != tc.stream || record.Model != model {
				t.Errorf("record %+v", record)
			}
		})
	}
}

func TestClientFormatsAgainstOpenAICompatibleUpstream(t *testing.T) {
	h := newHarness(t)
	up := openAIUpstream(t, "hello compat")
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	h.addCredential(compatCredential(resourceID, up.URL(), "sk-compat", "compat-model"))
	runFormatCases(t, h, up, resourceID, key, "compat-model", "compat", clientFormatCases("compat-model"))
}

func TestClaudeAPIKeyUpstream(t *testing.T) {
	h := newHarness(t)
	up := claudeUpstream(t, "hello claude")
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	h.addCredential(apiKeyCredential(resourceID, mcpav1.Provider_PROVIDER_CLAUDE, up.URL(), "sk-ant-test", "claude-test"))
	cases := clientFormatCases("claude-test")
	cases = append(cases, formatCase{name: "claude-count-tokens", path: "/v1/messages/count_tokens", header: map[string]string{"Anthropic-Version": "2023-06-01"}, body: `{"model":"claude-test","messages":[{"role":"user","content":"hi"}]}`})
	// count_tokens returns no text; check it separately.
	runFormatCases(t, h, up, resourceID, key, "claude-test", "claude", cases[:len(cases)-1])
	runFormatCases(t, h, up, resourceID, key, "claude-test", "7", cases[len(cases)-1:])
	for _, auth := range up.authsSeen() {
		if !strings.Contains(auth, "sk-ant-test") {
			t.Fatalf("upstream saw credential %q", auth)
		}
	}
}

func TestGeminiAPIKeyUpstream(t *testing.T) {
	h := newHarness(t)
	up := geminiUpstream(t, "hello gemini")
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	h.addCredential(apiKeyCredential(resourceID, mcpav1.Provider_PROVIDER_GEMINI, up.URL(), "AIza-test", "gemini-test"))
	runFormatCases(t, h, up, resourceID, key, "gemini-test", "gemini", clientFormatCases("gemini-test"))
	for _, auth := range up.authsSeen() {
		if auth != "AIza-test" {
			t.Fatalf("upstream saw credential %q", auth)
		}
	}
}

func TestCompressedUpstreamResponse(t *testing.T) {
	h := newHarness(t)
	up := newUpstream(t, func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		var compressed bytes.Buffer
		zw := gzip.NewWriter(&compressed)
		_, _ = io.WriteString(zw, openAIChatResponse(gjson.GetBytes(body, "model").String(), "hello gzip"))
		_ = zw.Close()
		w.Header().Set("Content-Type", "application/json")
		w.Header().Set("Content-Encoding", "gzip")
		_, _ = w.Write(compressed.Bytes())
	})
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	h.addCredential(compatCredential(resourceID, up.URL(), "sk-gzip", "gz-model"))
	runFormatCases(t, h, up, resourceID, key, "gz-model", "hello gzip", clientFormatCases("gz-model")[:1])
}

// TestClaudeBaseURLTrailingSlashRequestLine checks that the HTTP/1.1 fallback
// request line matches the SDK executor's verbatim base URL concatenation, so
// billed request bytes equal the bytes the upstream received.
func TestClaudeBaseURLTrailingSlashRequestLine(t *testing.T) {
	for _, tc := range []struct{ name, suffix, wantURI string }{
		{"host", "", "/v1/messages?beta=true"},
		{"host trailing slash", "/", "//v1/messages?beta=true"},
		{"path", "/gw", "/gw/v1/messages?beta=true"},
		{"path trailing slash", "/gw/", "/gw//v1/messages?beta=true"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			h := newHarness(t)
			up := claudeUpstream(t, "hello claude")
			resourceID := newResourceID()
			key := h.addKey(resourceID)
			h.addCredential(apiKeyCredential(resourceID, mcpav1.Provider_PROVIDER_CLAUDE, up.URL()+tc.suffix, "sk-ant-test", "claude-test"))
			runFormatCases(t, h, up, resourceID, key, "claude-test", "claude", []formatCase{clientFormatCases("claude-test")[4]})
			paths := up.pathsSeen()
			if len(paths) != 1 || paths[0] != tc.wantURI {
				t.Fatalf("upstream request URIs %q, want %q", paths, tc.wantURI)
			}
		})
	}
}
