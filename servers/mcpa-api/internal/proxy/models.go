// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"encoding/json"
	"net/http"
	"strings"
	"time"

	"github.com/router-for-me/CLIProxyAPI/v6/sdk/cliproxy"
	"github.com/tidwall/gjson"
)

// writeModelListing answers model listings from the resource's own
// credentials. The SDK listing handlers read the process-wide model registry
// and would expose models registered by other resources.
func writeModelListing(w http.ResponseWriter, rt route, models []*cliproxy.ModelInfo, allowlist []string) {
	visible := make([]*cliproxy.ModelInfo, 0, len(models))
	for _, model := range models {
		if modelAllowed(model.ID, allowlist) {
			visible = append(visible, model)
		}
	}
	var body any
	status := http.StatusOK
	switch rt.kind {
	case routeClaudeModels:
		data := make([]map[string]any, 0, len(visible))
		for _, model := range visible {
			data = append(data, map[string]any{
				"type":         "model",
				"id":           model.ID,
				"display_name": displayName(model),
				"created_at":   time.Unix(model.Created, 0).UTC().Format(time.RFC3339),
			})
		}
		listing := map[string]any{"data": data, "has_more": false, "first_id": nil, "last_id": nil}
		if len(visible) > 0 {
			listing["first_id"] = visible[0].ID
			listing["last_id"] = visible[len(visible)-1].ID
		}
		body = listing
	case routeGeminiModels:
		data := make([]map[string]any, 0, len(visible))
		for _, model := range visible {
			data = append(data, geminiModel(model))
		}
		body = map[string]any{"models": data}
	case routeGeminiModel:
		name := strings.TrimPrefix(rt.model, "models/")
		for _, model := range visible {
			if model.ID == name {
				body = geminiModel(model)
				break
			}
		}
		if body == nil {
			status = http.StatusNotFound
			body = map[string]any{"error": map[string]any{
				"code":    http.StatusNotFound,
				"message": "model not found",
				"status":  "NOT_FOUND",
			}}
		}
	default:
		data := make([]map[string]any, 0, len(visible))
		for _, model := range visible {
			ownedBy := model.OwnedBy
			if ownedBy == "" {
				ownedBy = model.Type
			}
			data = append(data, map[string]any{
				"id":       model.ID,
				"object":   "model",
				"created":  model.Created,
				"owned_by": ownedBy,
			})
		}
		body = map[string]any{"object": "list", "data": data}
	}
	encoded, err := json.Marshal(body)
	if err != nil {
		status = http.StatusInternalServerError
		encoded = []byte(`{"error":{"message":"internal error","type":"internal_error"}}`)
	}
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_, _ = w.Write(encoded)
}

func displayName(model *cliproxy.ModelInfo) string {
	if model.DisplayName != "" {
		return model.DisplayName
	}
	return model.ID
}

func geminiModel(model *cliproxy.ModelInfo) map[string]any {
	methods := model.SupportedGenerationMethods
	if len(methods) == 0 {
		methods = []string{"generateContent", "streamGenerateContent", "countTokens"}
	}
	out := map[string]any{
		"name":                       "models/" + model.ID,
		"displayName":                displayName(model),
		"supportedGenerationMethods": methods,
	}
	if model.Description != "" {
		out["description"] = model.Description
	}
	if model.Version != "" {
		out["version"] = model.Version
	}
	if model.InputTokenLimit > 0 {
		out["inputTokenLimit"] = model.InputTokenLimit
	}
	if model.OutputTokenLimit > 0 {
		out["outputTokenLimit"] = model.OutputTokenLimit
	}
	return out
}

// hasDuplicateTopLevelKey reports whether the top-level JSON object of body
// holds name more than once. Keys are compared after JSON unescaping and
// without case, because decoders in the request path differ: gjson reads the
// first occurrence and is case-sensitive, while a last-wins or case-folding
// decoder (such as encoding/json) reads another one. Nested objects are not
// inspected. Bodies that are not JSON objects hold no duplicates.
func hasDuplicateTopLevelKey(body []byte, name string) bool {
	root := gjson.ParseBytes(body)
	if !root.IsObject() {
		return false
	}
	seen := 0
	root.ForEach(func(key, _ gjson.Result) bool {
		if strings.EqualFold(key.String(), name) {
			seen++
		}
		return seen < 2
	})
	return seen > 1
}
