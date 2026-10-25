// SPDX-License-Identifier: Apache-2.0

package mcp

import (
	"encoding/json"
	"strings"
	"testing"

	lanev1 "github.com/delinoio/apps/protos/lane/v1"
	"google.golang.org/protobuf/reflect/protoreflect"
)

func TestCatalogMatchesServices(t *testing.T) {
	tools := Tools()
	byName := map[string]Tool{}
	for _, tool := range tools {
		if _, exists := byName[tool.Name]; exists {
			t.Fatalf("duplicate %s", tool.Name)
		}
		byName[tool.Name] = tool
		raw, _ := json.Marshal(tool.InputSchema)
		if strings.Contains(string(raw), `"origin"`) || strings.Contains(string(raw), `"deviceName"`) {
			t.Fatalf("origin exposed: %s", raw)
		}
	}
	count := 6
	for _, name := range []protoreflect.Name{"LaneService", "AccountService", "WebhookService"} {
		s := lanev1.File_lane_v1_lane_proto.Services().ByName(name)
		count += s.Methods().Len()
		for i := 0; i < s.Methods().Len(); i++ {
			m := s.Methods().Get(i)
			tool, ok := byName["lane_"+snake(string(m.Name()))]
			if !ok || tool.method != string(m.Name()) {
				t.Fatal(m.FullName())
			}
		}
	}
	if len(tools) != count {
		t.Fatalf("got %d want %d", len(tools), count)
	}
	for _, name := range []string{"lane_get_local_status", "lane_connect_sync", "lane_disconnect_sync", "lane_remove_device_data", "lane_get_notification_preferences", "lane_set_notification_preferences"} {
		if _, ok := byName[name]; !ok {
			t.Fatal(name)
		}
	}
}

func TestStrictInputSchemas(t *testing.T) {
	byName := map[string]Tool{}
	for _, tool := range Tools() {
		byName[tool.Name] = tool
	}
	cases := []struct {
		tool, raw string
		valid     bool
	}{
		{"lane_get_task", `{"id":"x"}`, true},
		{"lane_get_task", `{"id":"x","extra":1}`, false},
		{"lane_get_task", `{"id":null}`, false},
		{"lane_get_task", `{"id":"a","id":"b"}`, false},
		{"lane_create_task", `{"meta":{"requestId":"r"},"repository":{"url":"https://host/repo"},"title":"T","progress":"PROGRESS_TODO","proposal":"PROPOSAL_PENDING"}`, true},
		{"lane_create_task", `{"meta":{"origin":{}}}`, false},
		{"lane_create_task", `{"meta":{"request_id":"r"}}`, false},
		{"lane_create_task", `{"progress":1}`, false},
		{"lane_create_task", `{"progress":"TODO"}`, false},
		{"lane_create_task", `{"repository":{"id":"x","url":"u"}}`, false},
		{"lane_complete_event", `{"tasks":[{"meta":{"origin":{"kind":"ORIGIN_KIND_CLI"}}}]}`, false},
		{"lane_complete_event", `{"tasks":[{"meta":{"requestId":"r"},"title":"T"}]}`, true},
		{"lane_resolve_conflict", `{"resolved":{"origin":{}}}`, false},
		{"lane_resolve_conflict", `{"resolved":{"task":{},"repository":{}}}`, false},
		{"lane_resolve_conflict", `{"resolved":{"createdAt":"2026-10-25T00:00:00Z","task":{"prompt":""}}}`, true},
		{"lane_resolve_conflict", `{"resolved":{"createdAt":"yesterday"}}`, false},
		{"lane_save_receiver", `{"expectedRevision":"18446744073709551615"}`, true},
		{"lane_save_receiver", `{"expectedRevision":"18446744073709551616"}`, false},
		{"lane_save_receiver", `{"expectedRevision":2}`, false},
		{"lane_save_receiver", `{"expectedRevision":"-1"}`, false},
		{"lane_list_tasks", `{"page":{"size":4294967296}}`, false},
		{"lane_list_tasks", `{"progress":["PROGRESS_DONE"],"page":{"size":10}}`, true},
		{"lane_register_push", `{"publicKey":"aGVsbG8=","enabled":true}`, true},
		{"lane_register_push", `{"publicKey":"!"}`, false},
		{"lane_connect_sync", `{"accountId":"account"}`, true},
		{"lane_disconnect_sync", `{}`, false},
		{"lane_disconnect_sync", `{"retention":"LOCAL_RETENTION_KEEP_LOCAL"}`, true},
		{"lane_set_notification_preferences", `{"enabled":false}`, true},
		{"lane_set_notification_preferences", `{"enabled":true,"content":"DETAIL"}`, false},
	}
	for _, tc := range cases {
		t.Run(tc.tool+tc.raw, func(t *testing.T) {
			err := byName[tc.tool].check(json.RawMessage(tc.raw))
			if (err == nil) != tc.valid {
				t.Fatalf("valid=%v err=%v", tc.valid, err)
			}
		})
	}
}
