// SPDX-License-Identifier: FSL-1.1-ALv2

package mcp

import (
	"google.golang.org/protobuf/reflect/protoreflect"
)

// MessageSchema returns the JSON Schema of the ProtoJSON form of md with
// proto field names, which is the form tools accept and return. Nested
// messages are inlined; a recursive reference becomes an unconstrained
// object.
func MessageSchema(md protoreflect.MessageDescriptor) map[string]any {
	return messageSchema(md, map[protoreflect.FullName]bool{})
}

func messageSchema(md protoreflect.MessageDescriptor, visiting map[protoreflect.FullName]bool) map[string]any {
	if s, ok := wellKnownSchema(md); ok {
		return s
	}
	if visiting[md.FullName()] {
		return map[string]any{"type": "object"}
	}
	visiting[md.FullName()] = true
	defer delete(visiting, md.FullName())

	props := map[string]any{}
	fields := md.Fields()
	for i := range fields.Len() {
		f := fields.Get(i)
		props[string(f.Name())] = fieldSchema(f, visiting)
	}
	return map[string]any{
		"type":                 "object",
		"properties":           props,
		"additionalProperties": false,
	}
}

func fieldSchema(f protoreflect.FieldDescriptor, visiting map[protoreflect.FullName]bool) map[string]any {
	switch {
	case f.IsMap():
		return map[string]any{"type": "object", "additionalProperties": singularSchema(f.MapValue(), visiting)}
	case f.IsList():
		return map[string]any{"type": "array", "items": singularSchema(f, visiting)}
	default:
		return singularSchema(f, visiting)
	}
}

func singularSchema(f protoreflect.FieldDescriptor, visiting map[protoreflect.FullName]bool) map[string]any {
	switch f.Kind() {
	case protoreflect.BoolKind:
		return map[string]any{"type": "boolean"}
	case protoreflect.StringKind:
		return map[string]any{"type": "string"}
	case protoreflect.BytesKind:
		return map[string]any{"type": "string", "contentEncoding": "base64"}
	case protoreflect.Int32Kind, protoreflect.Sint32Kind, protoreflect.Sfixed32Kind,
		protoreflect.Uint32Kind, protoreflect.Fixed32Kind:
		return map[string]any{"type": "integer"}
	case protoreflect.Int64Kind, protoreflect.Sint64Kind, protoreflect.Sfixed64Kind,
		protoreflect.Uint64Kind, protoreflect.Fixed64Kind:
		// ProtoJSON writes 64-bit integers as decimal strings and reads both.
		return map[string]any{"type": []string{"string", "integer"}}
	case protoreflect.FloatKind, protoreflect.DoubleKind:
		return map[string]any{"type": "number"}
	case protoreflect.EnumKind:
		values := f.Enum().Values()
		names := make([]string, 0, values.Len())
		for i := range values.Len() {
			names = append(names, string(values.Get(i).Name()))
		}
		return map[string]any{"type": "string", "enum": names}
	case protoreflect.MessageKind, protoreflect.GroupKind:
		return messageSchema(f.Message(), visiting)
	default:
		return map[string]any{}
	}
}

// wellKnownSchema covers the well-known types that the mcpa protocol uses.
func wellKnownSchema(md protoreflect.MessageDescriptor) (map[string]any, bool) {
	switch md.FullName() {
	case "google.protobuf.Timestamp":
		return map[string]any{"type": "string", "format": "date-time"}, true
	case "google.protobuf.Duration":
		return map[string]any{"type": "string", "pattern": `^-?[0-9]+(\.[0-9]+)?s$`}, true
	default:
		return nil, false
	}
}
