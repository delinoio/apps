// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"encoding/json"
	"net/http"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
)

// RequestIDHeader carries the call record ID on every model-call response.
const RequestIDHeader = "X-Mcpa-Request-Id"

const errorDomain = "mcpa.api.delino.io"

// rejectionBody renders a rejection in the error shape of the client format.
func rejectionBody(format mcpav1.EndpointFormat, rejection *Rejection) []byte {
	code := rejection.Code.String()
	message := rejection.Message
	if message == "" {
		message = code
	}
	var body any
	switch format {
	case mcpav1.EndpointFormat_ENDPOINT_FORMAT_CLAUDE:
		body = map[string]any{
			"type": "error",
			"error": map[string]any{
				"type":    code,
				"message": message,
			},
		}
	case mcpav1.EndpointFormat_ENDPOINT_FORMAT_GEMINI:
		status := rejection.Code.HTTPStatus()
		body = map[string]any{
			"error": map[string]any{
				"code":    status,
				"message": message,
				"status":  googleRPCStatus(status),
				"details": []any{map[string]any{
					"@type":  "type.googleapis.com/google.rpc.ErrorInfo",
					"reason": code,
					"domain": errorDomain,
				}},
			},
		}
	default:
		body = map[string]any{
			"error": map[string]any{
				"message": message,
				"type":    code,
				"code":    code,
			},
		}
	}
	encoded, err := json.Marshal(body)
	if err != nil {
		return []byte(`{"error":{"message":"internal error","type":"internal_error","code":"internal_error"}}`)
	}
	return encoded
}

// googleRPCStatus maps an HTTP status to the canonical google.rpc code name.
func googleRPCStatus(status int) string {
	switch status {
	case http.StatusBadRequest:
		return "INVALID_ARGUMENT"
	case http.StatusUnauthorized:
		return "UNAUTHENTICATED"
	case http.StatusPaymentRequired, http.StatusTooManyRequests:
		return "RESOURCE_EXHAUSTED"
	case http.StatusForbidden:
		return "PERMISSION_DENIED"
	case http.StatusNotFound:
		return "NOT_FOUND"
	case http.StatusRequestEntityTooLarge:
		return "INVALID_ARGUMENT"
	case http.StatusServiceUnavailable:
		return "UNAVAILABLE"
	default:
		return "INTERNAL"
	}
}

// writeRejection writes a rejection response. The request ID header must
// already be set.
func writeRejection(w http.ResponseWriter, format mcpav1.EndpointFormat, rejection *Rejection) {
	body := rejectionBody(format, rejection)
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(rejection.Code.HTTPStatus())
	_, _ = w.Write(body)
}
