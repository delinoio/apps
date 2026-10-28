// SPDX-License-Identifier: Apache-2.0
package lanev1_test

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	lanev1 "github.com/delinoio/apps/protos/lane/v1"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protoreflect"
	"google.golang.org/protobuf/types/dynamicpb"
)

func TestWireContract(t *testing.T) {
	file := lanev1.File_lane_v1_lane_proto
	for name, fields := range map[string][]string{
		"Origin":               {"kind", "device_id", "device_name", "account_id"},
		"Task":                 {"repository_id", "title", "description", "prompt", "progress", "proposal"},
		"CompletedFilter":      {"selection", "cutoff"},
		"TerminalEntry":        {"kind", "entered_at"},
		"ListTasksRequest":     {"page", "repository_id", "progress", "proposal", "search", "filter", "sort_key", "sort_direction", "completed"},
		"MutationMeta":         {"request_id", "origin"},
		"SyncMutation":         {"meta", "version", "expected_head_ids", "kind"},
		"ErrorDetail":          {"reason", "correlation_id", "entity_id", "current_head_ids", "retryable"},
		"WebhookEvent":         {"id", "receiver_id", "repository_id", "sender_event_id", "event_type", "occurred_at", "payload", "state", "revision"},
		"RegisterPushRequest":  {"meta", "device_id", "provider", "endpoint", "public_key", "auth_secret", "enabled", "content"},
		"CompleteEventRequest": {"meta", "id", "lease_token", "tasks"},
	} {
		desc := file.Messages().ByName(protoreflect.Name(name))
		if desc == nil {
			t.Fatalf("missing message %s", name)
		}
		for i, name := range fields {
			field := desc.Fields().ByName(protoreflect.Name(name))
			if field == nil || field.Number() != protoreflect.FieldNumber(i+1) {
				t.Errorf("%s.%s must have number %d", desc.Name(), name, i+1)
			}
		}
	}
	envelope := file.Messages().ByName("EntityVersion")
	for name, number := range map[string]int{"format_version": 1, "entity_id": 2, "kind": 3, "version_id": 4, "parent_version_ids": 5, "created_at": 6, "origin": 7, "deleted": 8, "prompt_restoration": 9, "repository": 10, "task": 11} {
		if field := envelope.Fields().ByName(protoreflect.Name(name)); field == nil || int(field.Number()) != number {
			t.Errorf("EntityVersion.%s field number changed", name)
		}
	}
	for name, count := range map[string]int{"LaneService": 15, "SyncService": 4, "AccountService": 7, "WebhookService": 13} {
		svc := file.Services().ByName(protoreflect.Name(name))
		if svc == nil || svc.Methods().Len() != count {
			t.Fatalf("unexpected methods for %s", name)
		}
		for i := 0; i < svc.Methods().Len(); i++ {
			method := svc.Methods().Get(i)
			if method.IsStreamingClient() || method.IsStreamingServer() != (method.Name() == "WatchChanges") {
				t.Errorf("unexpected streaming mode: %s", method.FullName())
			}
		}
	}
	if lanev1.ErrorReason_ERROR_REASON_LEASE_INVALID != 18 || lanev1.Progress_PROGRESS_CANCELED != 5 || lanev1.Proposal_PROPOSAL_REJECTED != 4 || lanev1.CompletedSelection_COMPLETED_SELECTION_ALL != 5 || lanev1.TerminalEntryKind_TERMINAL_ENTRY_KIND_UNKNOWN != 2 {
		t.Fatal("enum identities changed")
	}
}

func TestPromptPresenceRoundTrip(t *testing.T) {
	for _, input := range []string{`{"title":"Title"}`, `{"title":"Title","prompt":""}`, `{"title":"Title","prompt":"one"}`} {
		var task lanev1.Task
		if err := protojson.Unmarshal([]byte(input), &task); err != nil {
			t.Fatal(err)
		}
		present := strings.Contains(input, `"prompt"`)
		if (task.Prompt != nil) != present {
			t.Fatal("ProtoJSON lost optional prompt presence")
		}
		wire, err := proto.Marshal(&task)
		if err != nil {
			t.Fatal(err)
		}
		var decoded lanev1.Task
		if err := proto.Unmarshal(wire, &decoded); err != nil {
			t.Fatal(err)
		}
		if !proto.Equal(&task, &decoded) || (decoded.Prompt != nil) != present {
			t.Fatal("binary round trip lost presence")
		}
	}
}

type fixture struct {
	ID       string                     `json:"id"`
	Input    map[string]json.RawMessage `json:"input"`
	Expected map[string]json.RawMessage `json:"expected"`
}

// These checks verify fixture integrity and wire compatibility. Runtime adapters
// must separately execute the domain operations and compare expected outcomes.
func TestFixtureWireInputs(t *testing.T) {
	paths, err := filepath.Glob("fixtures/*.json")
	if err != nil {
		t.Fatal(err)
	}
	if len(paths) < 5 {
		t.Fatal("missing conformance fixture families")
	}
	for _, path := range paths {
		// The completed-list suite has entry, list and cursor cases with a dedicated evaluator.
		if filepath.Base(path) == "completed-list.json" {
			continue
		}
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		var suite struct {
			FormatVersion int       `json:"formatVersion"`
			Cases         []fixture `json:"cases"`
		}
		if err := json.Unmarshal(data, &suite); err != nil {
			t.Fatal(err)
		}
		if suite.FormatVersion != 1 || len(suite.Cases) == 0 {
			t.Fatalf("invalid suite %s", path)
		}
		seen := map[string]bool{}
		for _, c := range suite.Cases {
			if c.ID == "" || seen[c.ID] || len(c.Expected) == 0 {
				t.Fatalf("invalid/duplicate fixture %s in %s", c.ID, path)
			}
			seen[c.ID] = true
			t.Run(filepath.Base(path)+"/"+c.ID, func(t *testing.T) {
				var method string
				_ = json.Unmarshal(c.Input["method"], &method)
				if request := c.Input["request"]; request != nil && method != "" {
					decodeRequest(t, method, request)
				}
				if requests := c.Input["requests"]; requests != nil && method != "" {
					var items []json.RawMessage
					if err := json.Unmarshal(requests, &items); err != nil {
						t.Fatal(err)
					}
					for _, item := range items {
						decodeRequest(t, method, item)
					}
				}
				if mutations := c.Input["mutations"]; mutations != nil {
					var items []json.RawMessage
					if err := json.Unmarshal(mutations, &items); err != nil {
						t.Fatal(err)
					}
					for _, item := range items {
						if err := protojson.Unmarshal(item, new(lanev1.SyncMutation)); err != nil {
							t.Fatal(err)
						}
					}
				}
			})
		}
	}
}
func decodeRequest(t *testing.T, method string, raw []byte) {
	t.Helper()
	desc := lanev1.File_lane_v1_lane_proto.Messages().ByName(protoreflect.Name(method + "Request"))
	if desc == nil {
		t.Fatalf("unknown request %s", method)
	}
	if err := protojson.Unmarshal(raw, dynamicpb.NewMessage(desc)); err != nil {
		t.Fatal(err)
	}
}

func TestSignatureKnownAnswers(t *testing.T) {
	data, err := os.ReadFile("fixtures/webhooks.json")
	if err != nil {
		t.Fatal(err)
	}
	var suite struct {
		Cases []struct {
			ID    string `json:"id"`
			Input struct {
				Operation string `json:"operation"`
				Timestamp string `json:"timestamp"`
				Secret    string `json:"secretBase64url"`
				Body      string `json:"rawBody"`
				Signature string `json:"signature"`
			} `json:"input"`
		}
	}
	if err := json.Unmarshal(data, &suite); err != nil {
		t.Fatal(err)
	}
	for _, c := range suite.Cases {
		if c.Input.Operation != "verifySignature" {
			continue
		}
		t.Run(c.ID, func(t *testing.T) {
			key, err := base64.RawURLEncoding.DecodeString(c.Input.Secret)
			if err != nil || len(key) != 32 {
				t.Fatal("invalid synthetic key")
			}
			mac := hmac.New(sha256.New, key)
			mac.Write([]byte(c.Input.Timestamp + "." + c.Input.Body))
			valid := c.Input.Signature == "v1="+hex.EncodeToString(mac.Sum(nil))
			want := c.ID != "changed-body" && c.ID != "rotated-old-key"
			if valid != want {
				t.Fatal("known-answer signature does not match fixture")
			}
		})
	}
}

func TestCanonicalFingerprintKnownAnswers(t *testing.T) {
	data, err := os.ReadFile("fixtures/fingerprints.json")
	if err != nil {
		t.Fatal(err)
	}
	var suite struct {
		Cases []struct {
			ID    string `json:"id"`
			Input struct {
				Request json.RawMessage `json:"normalizedRequest"`
			} `json:"input"`
			Expected struct {
				Canonical string `json:"canonicalJson"`
				SHA256    string `json:"sha256"`
			} `json:"expected"`
		}
	}
	if err := json.Unmarshal(data, &suite); err != nil {
		t.Fatal(err)
	}
	seen := map[string]bool{}
	for _, c := range suite.Cases {
		t.Run(c.ID, func(t *testing.T) {
			decodeRequest(t, "CreateTask", c.Input.Request)
			sum := sha256.Sum256([]byte(c.Expected.Canonical))
			digest := hex.EncodeToString(sum[:])
			if digest != c.Expected.SHA256 || seen[digest] {
				t.Fatal("invalid or colliding known-answer fingerprint")
			}
			seen[digest] = true
			var input, canonical any
			if err := json.Unmarshal(c.Input.Request, &input); err != nil {
				t.Fatal(err)
			}
			if err := json.Unmarshal([]byte(c.Expected.Canonical), &canonical); err != nil {
				t.Fatal(err)
			}
			// This corpus contains only strings and objects, so encoding/json's sorted
			// keys agree with RFC 8785. Runtime fingerprints also need RFC number rules.
			encoded, err := json.Marshal(input)
			if err != nil {
				t.Fatal(err)
			}
			if string(encoded) != c.Expected.Canonical {
				t.Fatal("canonical text differs from normalized request")
			}
		})
	}
}

func TestRestorationMarkerSyncRoundTrip(t *testing.T) {
	for _, marked := range []bool{false, true} {
		version := &lanev1.EntityVersion{FormatVersion: 1, Kind: lanev1.EntityKind_ENTITY_KIND_TASK, PromptRestoration: marked, Body: &lanev1.EntityVersion_Task{Task: &lanev1.Task{Prompt: proto.String("same")}}}
		mutation := &lanev1.SyncMutation{Kind: lanev1.MutationKind_MUTATION_KIND_SAVE, Version: version}
		wire, err := proto.Marshal(mutation)
		if err != nil {
			t.Fatal(err)
		}
		decoded := new(lanev1.SyncMutation)
		if err := proto.Unmarshal(wire, decoded); err != nil {
			t.Fatal(err)
		}
		if !proto.Equal(mutation, decoded) {
			t.Fatal("sync binary lost restoration marker")
		}
		data, err := protojson.Marshal(decoded)
		if err != nil {
			t.Fatal(err)
		}
		restored := new(lanev1.SyncMutation)
		if err := protojson.Unmarshal(data, restored); err != nil {
			t.Fatal(err)
		}
		if restored.Version.PromptRestoration != marked {
			t.Fatal("sync JSON lost restoration marker")
		}
	}
	old := new(lanev1.EntityVersion)
	if err := protojson.Unmarshal([]byte(`{"formatVersion":1,"task":{"prompt":"same"}}`), old); err != nil {
		t.Fatal(err)
	}
	if old.PromptRestoration {
		t.Fatal("missing marker must default to false")
	}
}

// TestTaskMetadataWireContract pins the field numbers and enum identities added for task
// metadata, relationships and repository planning. Numbers never change once released.
func TestTaskMetadataWireContract(t *testing.T) {
	file := lanev1.File_lane_v1_lane_proto
	for name, fields := range map[string]map[string]int{
		"Task": {"priority": 8, "type": 9, "label_ids": 10, "start_date": 11, "due_date": 12, "story_points": 13, "milestone_id": 14,
			"cycle_id": 15, "parent_task_id": 16, "depends_on_task_ids": 17, "related_task_ids": 18, "duplicate_of_task_id": 19},
		"Label":         {"repository_id": 1, "name": 2, "color": 3, "description": 4},
		"Milestone":     {"repository_id": 1, "name": 2, "description": 3, "target_date": 4, "completed": 5},
		"Cycle":         {"repository_id": 1, "name": 2, "description": 3, "start_date": 4, "end_date": 5},
		"EntityVersion": {"label": 13, "milestone": 14, "cycle": 15},
		"ErrorDetail":   {"violation": 6},
		"DateRange":     {"from": 1, "to": 2, "include_unassigned": 3},
		"PointRange":    {"min": 1, "max": 2, "include_unassigned": 3},
		"TaskFilter": {"priorities": 1, "include_unassigned_priority": 2, "types": 3, "include_unassigned_type": 4, "label_ids": 5, "include_unlabeled": 6,
			"start_date": 7, "due_date": 8, "story_points": 9, "milestone_ids": 10, "include_no_milestone": 11, "cycle_ids": 12, "include_no_cycle": 13,
			"relationships": 14, "include_no_relationships": 15},
		"ListTasksRequest": {"page": 1, "repository_id": 2, "progress": 3, "proposal": 4, "search": 5, "filter": 6, "sort_key": 7, "sort_direction": 8},
		"CreateTaskRequest": {"priority": 9, "type": 10, "label_ids": 11, "start_date": 12, "due_date": 13, "story_points": 14, "milestone_id": 15,
			"cycle_id": 16, "parent_task_id": 17, "depends_on_task_ids": 18, "related_task_ids": 19, "duplicate_of_task_id": 20, "relationship_edits": 21},
		"RelationshipEdits":        {"add_subtask_ids": 1, "remove_subtask_ids": 2, "add_blocked_task_ids": 3, "remove_blocked_task_ids": 4},
		"SaveTaskRequest":          {"relationship_edits": 5},
		"SaveTaskResponse":         {"affected": 2},
		"CreateTaskResponse":       {"affected": 3},
		"PlanningBody":             {"label": 1, "milestone": 2, "cycle": 3},
		"ListPlanningItemsRequest": {"page": 1, "repository_id": 2, "kind": 3},
	} {
		desc := file.Messages().ByName(protoreflect.Name(name))
		if desc == nil {
			t.Fatalf("missing message %s", name)
		}
		for field, number := range fields {
			got := desc.Fields().ByName(protoreflect.Name(field))
			if got == nil || got.Number() != protoreflect.FieldNumber(number) {
				t.Errorf("%s.%s must have number %d", name, field, number)
			}
		}
	}
	for name, ok := range map[string]bool{
		"optional start_date":   new(lanev1.Task).ProtoReflect().Descriptor().Fields().ByName("start_date").HasPresence(),
		"optional story_points": new(lanev1.Task).ProtoReflect().Descriptor().Fields().ByName("story_points").HasPresence(),
	} {
		if !ok {
			t.Errorf("%s must track presence", name)
		}
	}
	if lanev1.EntityKind_ENTITY_KIND_CYCLE != 6 || lanev1.ErrorReason_ERROR_REASON_CONSTRAINT_VIOLATION != 19 ||
		lanev1.ConstraintViolation_CONSTRAINT_VIOLATION_FIELD_RANGE != 10 || lanev1.RelationshipKind_RELATIONSHIP_KIND_DUPLICATE_OF != 6 ||
		lanev1.TaskSortKey_TASK_SORT_KEY_CYCLE_START_DATE != 7 || lanev1.SortDirection_SORT_DIRECTION_DESCENDING != 2 ||
		lanev1.TaskPriority_TASK_PRIORITY_LOW != 4 || lanev1.TaskType_TASK_TYPE_MAINTENANCE != 5 {
		t.Fatal("enum identities changed")
	}
	service := file.Services().ByName("PlanningService")
	if service == nil || service.Methods().Len() != 5 {
		t.Fatal("PlanningService must expose the five planning RPCs")
	}
	// Absent story points differ from zero on the wire and in ProtoJSON.
	for _, input := range []string{`{"storyPoints":0}`, `{}`} {
		var task lanev1.Task
		if err := protojson.Unmarshal([]byte(input), &task); err != nil {
			t.Fatal(err)
		}
		if (task.StoryPoints != nil) != strings.Contains(input, "storyPoints") {
			t.Fatal("story points lost presence")
		}
	}
}
