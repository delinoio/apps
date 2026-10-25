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
	for name, number := range map[string]int{"format_version": 1, "entity_id": 2, "kind": 3, "version_id": 4, "parent_version_ids": 5, "created_at": 6, "origin": 7, "deleted": 8, "repository": 10, "task": 11} {
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
	if lanev1.ErrorReason_ERROR_REASON_LEASE_INVALID != 18 || lanev1.Progress_PROGRESS_CANCELED != 5 || lanev1.Proposal_PROPOSAL_REJECTED != 4 {
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
