// SPDX-License-Identifier: Apache-2.0

package mcp

import (
	"encoding/json"
	"fmt"
	"regexp"
	"strings"
	"unicode"

	lanev1 "github.com/delinoio/apps/protos/lane/v1"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/reflect/protoreflect"
	"google.golang.org/protobuf/types/dynamicpb"
)

type schema map[string]any

type Tool struct {
	Name        string `json:"name"`
	Description string `json:"description"`
	InputSchema schema `json:"inputSchema"`
	method      string
	input       protoreflect.MessageDescriptor
}

func object(properties map[string]any) schema {
	return schema{"type": "object", "properties": properties, "additionalProperties": false}
}

func messageSchema(m protoreflect.MessageDescriptor) schema {
	if m.FullName() == "google.protobuf.Timestamp" {
		return schema{"type": "string", "format": "date-time"}
	}
	props := map[string]any{}
	for i := 0; i < m.Fields().Len(); i++ {
		f := m.Fields().Get(i)
		if f.Name() == "origin" || (f.Message() != nil && f.Message().FullName() == "lane.v1.Origin") {
			continue
		}
		if m.FullName() == "lane.v1.MutationMeta" && f.Name() != "request_id" {
			continue
		}
		s := fieldSchema(f)
		if f.IsList() {
			s = schema{"type": "array", "items": s}
		}
		props[f.JSONName()] = s
	}
	s := object(props)
	// Protobuf permits an absent oneof, but never two present alternatives.
	var constraints []any
	for i := 0; i < m.Oneofs().Len(); i++ {
		o := m.Oneofs().Get(i)
		if o.IsSynthetic() {
			continue
		}
		for a := 0; a < o.Fields().Len(); a++ {
			for b := a + 1; b < o.Fields().Len(); b++ {
				constraints = append(constraints, schema{"not": schema{"required": []string{o.Fields().Get(a).JSONName(), o.Fields().Get(b).JSONName()}}})
			}
		}
	}
	if len(constraints) > 0 {
		s["allOf"] = constraints
	}
	return s
}

func fieldSchema(f protoreflect.FieldDescriptor) schema {
	switch f.Kind() {
	case protoreflect.MessageKind:
		return messageSchema(f.Message())
	case protoreflect.EnumKind:
		values := []string{}
		for i := 0; i < f.Enum().Values().Len(); i++ {
			values = append(values, string(f.Enum().Values().Get(i).Name()))
		}
		return schema{"type": "string", "enum": values}
	case protoreflect.BoolKind:
		return schema{"type": "boolean"}
	case protoreflect.Int64Kind, protoreflect.Sint64Kind, protoreflect.Sfixed64Kind:
		return schema{"type": "string", "pattern": "^-?(0|[1-9][0-9]*)$"}
	case protoreflect.Uint64Kind, protoreflect.Fixed64Kind:
		return schema{"type": "string", "pattern": "^(0|[1-9][0-9]*)$"}
	case protoreflect.Int32Kind, protoreflect.Sint32Kind, protoreflect.Sfixed32Kind:
		return schema{"type": "integer", "minimum": float64(-2147483648), "maximum": float64(2147483647)}
	case protoreflect.Uint32Kind, protoreflect.Fixed32Kind:
		return schema{"type": "integer", "minimum": float64(0), "maximum": float64(4294967295)}
	case protoreflect.FloatKind, protoreflect.DoubleKind:
		return schema{"type": "number"}
	case protoreflect.BytesKind:
		return schema{"type": "string", "contentEncoding": "base64"}
	default:
		return schema{"type": "string"}
	}
}

func snake(s string) string {
	var b strings.Builder
	for i, r := range s {
		if unicode.IsUpper(r) && i > 0 {
			b.WriteByte('_')
		}
		b.WriteRune(unicode.ToLower(r))
	}
	return b.String()
}

// Tools returns the adapter surface. The injected dispatcher owns availability,
// local/remote selection, authorization, and all business validation.
func Tools() []Tool {
	var result []Tool
	services := lanev1.File_lane_v1_lane_proto.Services()
	for _, name := range []protoreflect.Name{"LaneService", "AccountService", "WebhookService"} {
		service := services.ByName(name)
		for i := 0; i < service.Methods().Len(); i++ {
			m := service.Methods().Get(i)
			method := string(m.Name())
			description := method + " using the selected Lane target."
			if name != "LaneService" {
				description += " Online-only; requires explicit remote selection and approval."
			}
			if method == "GetEvent" || method == "ListEvents" {
				description = method + "; local selection reads retained cached events only."
			}
			if strings.HasPrefix(method, "Delete") {
				description += " Permanently deletes data; invocation confirms task, repository, receiver, or event deletion."
			}
			if method == "DeleteAccountData" || method == "BeginDataDeletion" {
				description += " Account deletion requires human account confirmation and browser reauthentication; fabricated proof is forbidden."
			}
			result = append(result, Tool{Name: "lane_" + snake(method), Description: description, InputSchema: messageSchema(m.Input()), method: method, input: m.Input()})
		}
	}
	locals := []struct {
		name, description string
		s                 schema
	}{
		{"GetLocalStatus", "Read selected local space and device synchronization status.", object(map[string]any{})},
		{"ConnectSync", "Start explicit browser approval for synchronization; accountId is optional for first approval.", object(map[string]any{"accountId": schema{"type": "string"}})},
		{"DisconnectSync", "Disconnect synchronization with an explicit retained-local-data choice.", object(map[string]any{"retention": schema{"type": "string", "enum": []string{"LOCAL_RETENTION_KEEP_LOCAL", "LOCAL_RETENTION_REMOVE_LOCAL"}}})},
		{"RemoveDeviceData", "Permanently remove the selected local space after coordinated handle shutdown; invocation confirms removal.", object(map[string]any{})},
		{"GetNotificationPreferences", "Read this device's notification preferences and permission.", object(map[string]any{})},
		{"SetNotificationPreferences", "Set this device's notification preferences; enabling requests OS permission.", object(map[string]any{"enabled": schema{"type": "boolean"}, "content": schema{"type": "string", "enum": []string{"NOTIFICATION_CONTENT_DETAIL", "NOTIFICATION_CONTENT_GENERIC"}}})},
	}
	for _, l := range locals {
		if l.name == "DisconnectSync" {
			l.s["required"] = []string{"retention"}
		}
		if l.name == "SetNotificationPreferences" {
			l.s["required"] = []string{"enabled"}
		}
		result = append(result, Tool{Name: "lane_" + snake(l.name), Description: l.description, InputSchema: l.s, method: l.name})
	}
	return result
}

func validate(s schema, v any) error {
	bad := fmt.Errorf("arguments do not match the tool input schema")
	switch s["type"] {
	case "object":
		o, ok := v.(map[string]any)
		if !ok {
			return bad
		}
		props := s["properties"].(map[string]any)
		for k, value := range o {
			p, ok := props[k]
			if !ok {
				return fmt.Errorf("unknown or forbidden argument field %q", k)
			}
			if err := validate(p.(schema), value); err != nil {
				return err
			}
		}
		if req, ok := s["required"].([]string); ok {
			for _, k := range req {
				if _, ok := o[k]; !ok {
					return bad
				}
			}
		}
		if all, ok := s["allOf"].([]any); ok {
			for _, c := range all {
				r := c.(schema)["not"].(schema)["required"].([]string)
				_, a := o[r[0]]
				_, b := o[r[1]]
				if a && b {
					return fmt.Errorf("oneof alternatives are mutually exclusive")
				}
			}
		}
	case "array":
		a, ok := v.([]any)
		if !ok {
			return bad
		}
		for _, x := range a {
			if err := validate(s["items"].(schema), x); err != nil {
				return err
			}
		}
	case "string":
		str, ok := v.(string)
		if !ok {
			return bad
		}
		if values, ok := s["enum"].([]string); ok {
			found := false
			for _, x := range values {
				found = found || x == str
			}
			if !found {
				return bad
			}
		}
		if p, ok := s["pattern"].(string); ok && !regexp.MustCompile(p).MatchString(str) {
			return bad
		}
	case "boolean":
		if _, ok := v.(bool); !ok {
			return bad
		}
	case "integer", "number":
		n, ok := v.(json.Number)
		if !ok {
			return bad
		}
		f, err := n.Float64()
		if err != nil {
			return bad
		}
		if s["type"] == "integer" {
			if _, err := n.Int64(); err != nil {
				return bad
			}
		}
		if min, ok := s["minimum"].(float64); ok && f < min {
			return bad
		}
		if max, ok := s["maximum"].(float64); ok && f > max {
			return bad
		}
	}
	return nil
}

func (t Tool) check(raw json.RawMessage) error {
	value, err := decodeJSON(raw)
	if err != nil {
		return err
	}
	if err = validate(t.InputSchema, value); err != nil {
		return err
	}
	if t.input != nil {
		return protojson.Unmarshal(raw, dynamicpb.NewMessage(t.input))
	}
	return nil
}
