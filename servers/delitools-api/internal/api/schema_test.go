// SPDX-License-Identifier: FSL-1.1-ALv2
package api

import (
	"encoding/json"
	pb "github.com/delinoio/apps/protos/delitools/v1"
	"google.golang.org/protobuf/proto"
	"strings"
	"testing"
)

const personSchema = `{"type":"object","properties":{"name":{"type":"string"},"age":{"type":"integer"}},"required":["name","age"],"additionalProperties":false}`

func TestSchemaSubset(t *testing.T) {
	s, e := ParseSchema(personSchema)
	if e != nil {
		t.Fatal(e)
	}
	for _, tc := range []struct {
		data string
		want bool
	}{{`{"name":"Ada","age":37}`, true}, {`{"name":"Ada","age":37.5}`, false}, {`{"name":"Ada"}`, false}, {`{"name":"Ada","age":37,"extra":true}`, false}, {`{"name":"Ada","age":"37"}`, false}} {
		v, e := DecodeJSON([]byte(tc.data))
		if e != nil || ValidateOutput(s, v) != tc.want {
			t.Fatal(tc)
		}
	}
	for _, raw := range []string{`{"type":"object","$ref":"#"}`, `{"type":"object","properties":{"x":{"type":"string"}},"required":[],"additionalProperties":false}`, `{"type":"object","properties":{},"required":[],"additionalProperties":true}`, `{"type":"object","properties":{"x":{"type":"string","pattern":".*"}},"required":["x"],"additionalProperties":false}`, personSchema + `{}`} {
		if _, e = ParseSchema(raw); e == nil {
			t.Fatal("unsupported schema accepted", raw)
		}
	}
}
func testSettings() *pb.Settings {
	return &pb.Settings{MaxInputBytes: 100000, MaxTranslationItems: 20, ExecutionTimeoutSeconds: 5, TenantRequestsPerMinute: 100, TenantConcurrency: 3, MaxAiRetries: proto.Uint32(1), PriceSyncSeconds: 30, DeliveryRetrySeconds: 1, MaxBackoffSeconds: 10, AlertBacklog: 10, AlertExpirySeconds: 3600, TranslationModels: []string{ExtractionModel}}
}
func TestMandatorySettings(t *testing.T) {
	s := testSettings()
	if e := ValidateSettings(s); e != nil {
		t.Fatal(e)
	}
	for _, field := range []string{"max_input_bytes", "max_translation_items", "execution_timeout_seconds", "tenant_requests_per_minute", "tenant_concurrency", "max_ai_retries", "price_sync_seconds", "delivery_retry_seconds", "max_backoff_seconds", "alert_backlog", "alert_expiry_seconds"} {
		c := proto.Clone(s).(*pb.Settings)
		m := c.ProtoReflect()
		fd := m.Descriptor().Fields().ByTextName(field)
		m.Clear(fd)
		if ValidateSettings(c) == nil {
			t.Fatal("missing setting accepted", field)
		}
	}
	s.MaxAiRetries = proto.Uint32(0)
	if e := ValidateSettings(s); e != nil {
		t.Fatal("zero retries must be valid", e)
	}
}
func TestCatalogCategories(t *testing.T) {
	p := Price{Input: "0.001", Output: "0.002", CacheRead: "0.0001", CacheWrite: "0.003", InputTiers: []Tier{{Min: 0, Max: ptr(100), Cost: "0.001"}, {Min: 100, Cost: "0.002"}}}
	tokens, e := p.Tokens(Counts{Input: 80, Output: 2, CacheRead: 20})
	if e != nil || tokens[0].UsdPerToken != "0.002" || tokens[2].Tokens != 20 {
		t.Fatal(tokens, e)
	}
	p.CacheRead = ""
	if _, e = p.Tokens(Counts{CacheRead: 1}); e == nil {
		t.Fatal("missing cached-token price accepted")
	}
}
func ptr(n uint64) *uint64 { return &n }

func TestNumericEnumUsesExactEquality(t *testing.T) {
	schema, err := ParseSchema(`{"type":"object","properties":{"x":{"type":"number","enum":[1,9007199254740993]}},"required":["x"],"additionalProperties":false}`)
	if err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		value string
		want  bool
	}{{"1", true}, {"1.0", true}, {"1e0", true}, {"9007199254740993.0", true}, {"9007199254740992", false}, {"1.01", false}, {`"1"`, false}} {
		data, err := DecodeJSON([]byte(`{"x":` + tc.value + `}`))
		if err != nil || ValidateOutput(schema, data) != tc.want {
			t.Fatal(tc, err)
		}
	}
}

func TestCachedTokenPriceTiers(t *testing.T) {
	p := Price{Input: "1", Output: "1", CacheRead: "2", CacheWrite: "3", CacheReadTiers: []Tier{{Min: 0, Max: ptr(100), Cost: "2"}, {Min: 100, Cost: "4"}}, CacheWriteTiers: []Tier{{Min: 0, Max: ptr(100), Cost: "3"}, {Min: 100, Cost: "6"}}}
	tokens, err := p.Tokens(Counts{Input: 70, CacheRead: 20, CacheWrite: 10})
	if err != nil || tokens[2].UsdPerToken != "4" || tokens[3].UsdPerToken != "6" {
		t.Fatal(tokens, err)
	}
	tokens, err = p.Tokens(Counts{Input: 69, CacheRead: 20, CacheWrite: 10})
	if err != nil || tokens[2].UsdPerToken != "2" || tokens[3].UsdPerToken != "3" {
		t.Fatal(tokens, err)
	}
}

func TestNumericEnumCharacterBudget(t *testing.T) {
	values := make([]any, 1000)
	for i := range values {
		values[i] = json.Number(strings.Repeat("1", 121))
	}
	schema := map[string]any{"enum": values}
	if schemaBudget(schema) {
		t.Fatal("oversized numeric enums accepted")
	}
	for i := range values {
		values[i] = json.Number("12345678901234567890")
	}
	if !schemaBudget(schema) {
		t.Fatal("numeric enums incorrectly use the string-only limit")
	}
}
