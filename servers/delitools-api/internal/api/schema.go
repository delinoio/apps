// SPDX-License-Identifier: FSL-1.1-ALv2
package api

import (
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"math/big"
	"reflect"
	"slices"
	"unicode/utf8"
)

var errSchema = errors.New("unsupported JSON schema")

// DecodeJSON rejects trailing JSON and keeps numeric lexemes exact.
func DecodeJSON(raw []byte) (any, error) {
	d := json.NewDecoder(bytes.NewReader(raw))
	d.UseNumber()
	var value any
	if err := d.Decode(&value); err != nil {
		return nil, err
	}
	var extra any
	if err := d.Decode(&extra); err != io.EOF {
		return nil, errors.New("trailing JSON")
	}
	return value, nil
}
func ParseSchema(raw string) (map[string]any, error) {
	v, err := DecodeJSON([]byte(raw))
	if err != nil {
		return nil, errSchema
	}
	s, ok := v.(map[string]any)
	if !ok || s["type"] != "object" {
		return nil, errSchema
	}
	if err = checkSchema(s, 0); err != nil {
		return nil, err
	}
	if !schemaBudget(s) {
		return nil, errSchema
	}
	return s, nil
}
func checkSchema(s map[string]any, depth int) error {
	// This is the provider's supported nesting boundary, not a product size quota.
	if depth > 7 {
		return errSchema
	}
	for k := range s {
		if !slices.Contains([]string{"type", "description", "properties", "required", "additionalProperties", "items", "enum"}, k) {
			return errSchema
		}
	}
	if d, ok := s["description"]; ok {
		if _, ok = d.(string); !ok {
			return errSchema
		}
	}
	typ, ok := s["type"].(string)
	if !ok {
		return errSchema
	}
	switch typ {
	case "object":
		props, ok := s["properties"].(map[string]any)
		if !ok || s["additionalProperties"] != false {
			return errSchema
		}
		required, ok := s["required"].([]any)
		if !ok || len(required) != len(props) {
			return errSchema
		}
		seen := map[string]bool{}
		for _, r := range required {
			k, ok := r.(string)
			if !ok || seen[k] {
				return errSchema
			}
			if _, ok = props[k]; !ok {
				return errSchema
			}
			seen[k] = true
		}
		for _, v := range props {
			child, ok := v.(map[string]any)
			if !ok {
				return errSchema
			}
			if err := checkSchema(child, depth+1); err != nil {
				return err
			}
		}
	case "array":
		child, ok := s["items"].(map[string]any)
		if !ok {
			return errSchema
		}
		if err := checkSchema(child, depth+1); err != nil {
			return err
		}
	case "string", "number", "integer", "boolean", "null":
	default:
		return errSchema
	}
	for _, k := range []string{"properties", "required", "additionalProperties"} {
		if _, ok := s[k]; ok && typ != "object" {
			return errSchema
		}
	}
	if _, ok := s["items"]; ok && typ != "array" {
		return errSchema
	}
	if enum, ok := s["enum"]; ok {
		values, ok := enum.([]any)
		if !ok || len(values) == 0 || typ == "object" || typ == "array" {
			return errSchema
		}
		for _, v := range values {
			if !matchesType(typ, v) {
				return errSchema
			}
		}
	}
	return nil
}
func matchesType(typ string, v any) bool {
	switch typ {
	case "object":
		_, ok := v.(map[string]any)
		return ok
	case "array":
		_, ok := v.([]any)
		return ok
	case "string":
		_, ok := v.(string)
		return ok
	case "boolean":
		_, ok := v.(bool)
		return ok
	case "null":
		return v == nil
	case "number", "integer":
		n, ok := v.(json.Number)
		if !ok {
			return false
		}
		r, ok := new(big.Rat).SetString(string(n))
		return ok && (typ == "number" || r.IsInt())
	}
	return false
}
func ValidateOutput(s map[string]any, v any) bool {
	typ, _ := s["type"].(string)
	if !matchesType(typ, v) {
		return false
	}
	if values, ok := s["enum"].([]any); ok {
		found := false
		for _, x := range values {
			if enumEqual(x, v) {
				found = true
			}
		}
		if !found {
			return false
		}
	}
	switch typ {
	case "object":
		props := s["properties"].(map[string]any)
		o := v.(map[string]any)
		if len(o) != len(props) {
			return false
		}
		for k, c := range props {
			x, ok := o[k]
			if !ok || !ValidateOutput(c.(map[string]any), x) {
				return false
			}
		}
	case "array":
		for _, x := range v.([]any) {
			if !ValidateOutput(s["items"].(map[string]any), x) {
				return false
			}
		}
	}
	return true
}

// Reserve two nesting levels and one property for the extraction envelope.
// These are provider schema limits, validated before any paid execution.
func schemaBudget(root map[string]any) bool {
	properties, enums, chars := 1, 0, 5
	var visit func(map[string]any) bool
	visit = func(s map[string]any) bool {
		if props, ok := s["properties"].(map[string]any); ok {
			properties += len(props)
			for k, v := range props {
				chars += utf8.RuneCountInString(k)
				if !visit(v.(map[string]any)) {
					return false
				}
			}
		}
		if child, ok := s["items"].(map[string]any); ok {
			if !visit(child) {
				return false
			}
		}
		if values, ok := s["enum"].([]any); ok {
			enums += len(values)
			size := 0
			for _, v := range values {
				if text, ok := v.(string); ok {
					size += utf8.RuneCountInString(text)
				}
			}
			chars += size
			if len(values) > 250 && size > 15000 {
				return false
			}
		}
		return properties <= 5000 && enums <= 1000 && chars <= 120000
	}
	return visit(root)
}

func enumEqual(a, b any) bool {
	x, xok := a.(json.Number)
	y, yok := b.(json.Number)
	if xok || yok {
		if !xok || !yok {
			return false
		}
		xr, ok := new(big.Rat).SetString(string(x))
		if !ok {
			return false
		}
		yr, ok := new(big.Rat).SetString(string(y))
		return ok && xr.Cmp(yr) == 0
	}
	return reflect.DeepEqual(a, b)
}
