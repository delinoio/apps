// SPDX-License-Identifier: FSL-1.1-ALv2
package api

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestGatewayCapturesGenerationBeforeOutput(t *testing.T) {
	captured := false
	h := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer fixture" {
			t.Error("missing auth")
		}
		var body map[string]any
		if e := json.NewDecoder(r.Body).Decode(&body); e != nil {
			t.Error(e)
		}
		if body["model"] != ExtractionModel || body["stream"] != true {
			t.Error(body)
		}
		if _, ok := body["models"]; ok {
			t.Error("alternate model fallback")
		}
		w.Header().Set("Content-Type", "text/event-stream")
		fmt.Fprintln(w, `data: {"id":"gen_fixture","choices":[{"delta":{"content":"{}"},"finish_reason":"stop"}]}`)
		fmt.Fprintln(w, `data: {"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":4,"prompt_tokens_details":{"cached_tokens":3}}}`)
		fmt.Fprintln(w, "data: [DONE]")
	}))
	defer h.Close()
	g := &HTTPGateway{Client: h.Client(), BaseURL: h.URL, Token: "fixture", MaxResponseBytes: 10000}
	result, e := g.Execute(context.Background(), ExtractionModel, "fixture", map[string]any{}, func(id string) error { captured = id == "gen_fixture"; return nil })
	if e != nil || !captured || result.Content != "{}" || result.Usage.Input != 7 || result.Usage.CacheRead != 3 {
		t.Fatalf("%+v %v", result, e)
	}
}
func TestGatewayAmbiguityAndReconciliation(t *testing.T) {
	h := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasPrefix(r.URL.Path, "/generation"):
			fmt.Fprint(w, `{"data":{"id":"gen_fixture","model":"openai/gpt-6-luna","native_tokens_prompt":10,"native_tokens_completion":5,"native_tokens_cached":2,"native_tokens_cache_creation":1}}`)
		default:
			w.Header().Set("Content-Type", "text/event-stream")
			fmt.Fprintln(w, `data: {"id":"gen_fixture","choices":[]}`)
		}
	}))
	defer h.Close()
	g := &HTTPGateway{Client: h.Client(), BaseURL: h.URL, MaxResponseBytes: 10000}
	result, e := g.Execute(context.Background(), ExtractionModel, "fixture", nil, func(string) error { return nil })
	if e == nil || !result.Unknown || result.Usage != nil {
		t.Fatal("interrupted stream treated as known", result, e)
	}
	n, e := g.Lookup(context.Background(), "gen_fixture", ExtractionModel)
	if e != nil || n.Input != 7 || n.CacheWrite != 1 {
		t.Fatal(n, e)
	}
}
