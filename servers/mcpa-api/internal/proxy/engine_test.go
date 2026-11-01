// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"net/http"
	"strings"
	"testing"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
)

func TestOpenAICompatibleChatMetersExactBytes(t *testing.T) {
	h := newHarness(t)
	up := openAIUpstream(t, "hello there")
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	h.addCredential(compatCredential(resourceID, up.URL(), "sk-upstream", "gpt-test"))

	for _, stream := range []bool{false, true} {
		before := up.read.Load()
		body := `{"model":"gpt-test","messages":[{"role":"user","content":"hi"}]}`
		if stream {
			body = `{"model":"gpt-test","stream":true,"messages":[{"role":"user","content":"hi"}]}`
		}
		res := h.call(http.MethodPost, "/r/"+resourceID+"/v1/chat/completions", key, body, nil)
		if res.resp.StatusCode != http.StatusOK {
			t.Fatalf("stream=%v: status %d body %s", stream, res.resp.StatusCode, res.body)
		}
		if !strings.Contains(string(res.body), "hello") {
			t.Fatalf("stream=%v: unexpected body %s", stream, res.body)
		}
		record := h.sink.waitFinal(t, res.callID)
		providerBytes := up.read.Load() - before
		if record.RequestBytes != providerBytes {
			t.Errorf("stream=%v: request bytes %d, upstream received %d", stream, record.RequestBytes, providerBytes)
		}
		if want := expectedResponseBytes(res.resp, res.body); record.ResponseBytes != want {
			t.Errorf("stream=%v: response bytes %d, want %d", stream, record.ResponseBytes, want)
		}
		req, resp := h.sink.sums(res.callID)
		if req != record.RequestBytes || resp != record.ResponseBytes {
			t.Errorf("stream=%v: checkpoint deltas %d/%d do not sum to totals %d/%d", stream, req, resp, record.RequestBytes, record.ResponseBytes)
		}
		if record.Status != mcpav1.CallStatus_CALL_STATUS_SUCCEEDED {
			t.Errorf("stream=%v: status %s", stream, record.Status)
		}
		if record.Provider != mcpav1.Provider_PROVIDER_OPENAI_COMPATIBLE || record.Model != "gpt-test" || record.Streaming != stream {
			t.Errorf("stream=%v: record %+v", stream, record)
		}
		if record.ProviderStatusClass != mcpav1.StatusClass_STATUS_CLASS_2XX {
			t.Errorf("stream=%v: status class %s", stream, record.ProviderStatusClass)
		}
	}
	if got := up.authsSeen(); len(got) != 2 || got[0] != "Bearer sk-upstream" {
		t.Fatalf("upstream auth headers %v", got)
	}
}
