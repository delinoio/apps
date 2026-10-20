// SPDX-License-Identifier: FSL-1.1-ALv2
package api

import (
	"context"
	"encoding/json"
	"fmt"
	core "github.com/delinoio/apps/protos/delcore/v1"
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

func TestPriceSettlementGrammar(t *testing.T) {
	for _, value := range []string{"0", "1", "0.1", "1.00", strings.Repeat("9", 64)} {
		if !decimal(value) {
			t.Errorf("rejected valid price %q", value)
		}
	}
	for _, value := range []string{"", ".1", "01", "1.", "-0", "+1", "1e2", "1/2", " 1", "1\n", strings.Repeat("9", 65)} {
		for _, price := range []Price{
			{Input: value, Output: "0"}, {Input: "0", Output: value},
			{Input: "0", Output: "0", CacheRead: value, CacheWrite: value},
			{Input: "0", Output: "0", InputTiers: []Tier{{Cost: value}}},
			{Input: "0", Output: "0", OutputTiers: []Tier{{Cost: value}}},
			{Input: "0", Output: "0", CacheReadTiers: []Tier{{Cost: value}}},
			{Input: "0", Output: "0", CacheWriteTiers: []Tier{{Cost: value}}},
		} {
			// Empty optional flat cache prices mean unsupported, not a numeric value.
			if value == "" && price.Input == "0" && price.Output == "0" && len(price.InputTiers)+len(price.OutputTiers)+len(price.CacheReadTiers)+len(price.CacheWriteTiers) == 0 {
				continue
			}
			if price.Valid() {
				t.Errorf("accepted invalid price %q: %+v", value, price)
			}
		}
	}
}

func TestGatewayCacheCreationUsage(t *testing.T) {
	for _, tc := range []struct {
		name, fields string
		conflict     bool
	}{
		{"root", `"cache_creation_input_tokens":4,"prompt_tokens_details":{"cached_tokens":3}`, false},
		{"details", `"prompt_tokens_details":{"cached_tokens":3,"cache_creation_input_tokens":4}`, false},
		{"both", `"cache_creation_input_tokens":4,"prompt_tokens_details":{"cached_tokens":3,"cache_creation_input_tokens":4}`, false},
		{"conflict", `"cache_creation_input_tokens":0,"prompt_tokens_details":{"cached_tokens":3,"cache_creation_input_tokens":4}`, true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			h := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				fmt.Fprintln(w, `data: {"id":"gen_cache","choices":[{"delta":{"content":"{}"},"finish_reason":"stop"}],"usage":{"prompt_tokens":12,"completion_tokens":2,`+tc.fields+`}}`)
				fmt.Fprintln(w, "data: [DONE]")
			}))
			defer h.Close()
			g := &HTTPGateway{Client: h.Client(), BaseURL: h.URL, MaxResponseBytes: 10000}
			result, err := g.Execute(context.Background(), ExtractionModel, "fixture", nil, func(string) error { return nil })
			if tc.conflict {
				if err == nil || !result.Unknown || result.Usage != nil {
					t.Fatal("conflicting counts accepted", result, err)
				}
				return
			}
			if err != nil || result.Usage == nil || result.Usage.Input != 5 || result.Usage.CacheRead != 3 || result.Usage.CacheWrite != 4 {
				t.Fatal(result, err)
			}
			price := Price{Input: "1", Output: "2", CacheRead: "3", CacheWrite: "4", CacheWriteTiers: []Tier{{Cost: "5"}}}
			tokens, err := price.Tokens(*result.Usage)
			if err != nil {
				t.Fatal(err)
			}
			found := false
			for _, token := range tokens {
				if token.Kind == core.AiTokenKind_AI_TOKEN_KIND_CACHE_WRITE {
					found = true
					if token.Tokens != 4 || token.UsdPerToken != "5" {
						t.Fatal(token)
					}
				}
			}
			if !found {
				t.Fatal("missing cache write billing")
			}
		})
	}
}
