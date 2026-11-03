// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
)

// ErrorClass is the bounded, secret-free error classifier stored with call
// records.
type ErrorClass string

const (
	ErrorClassNone                  ErrorClass = ""
	ErrorClassRateLimited           ErrorClass = "rate_limited"
	ErrorClassProviderAuth          ErrorClass = "provider_auth"
	ErrorClassProvider4xx           ErrorClass = "provider_4xx"
	ErrorClassProvider5xx           ErrorClass = "provider_5xx"
	ErrorClassNetwork               ErrorClass = "network"
	ErrorClassClientCancelled       ErrorClass = "client_cancelled"
	ErrorClassStreamInterrupted     ErrorClass = "stream_interrupted"
	ErrorClassModelNotAllowed       ErrorClass = "model_not_allowed"
	ErrorClassBudgetExhausted       ErrorClass = "budget_exhausted"
	ErrorClassTenantInactive        ErrorClass = "tenant_inactive"
	ErrorClassDependencyUnavailable ErrorClass = "dependency_unavailable"
	ErrorClassInvalidCallKey        ErrorClass = "invalid_call_key"
	ErrorClassNoCredential          ErrorClass = "no_credential"
	ErrorClassRequestTooLarge       ErrorClass = "request_too_large"
	ErrorClassInternal              ErrorClass = "internal"
)

// statusLabel returns the bounded metric label for a call status.
func statusLabel(status mcpav1.CallStatus) string {
	switch status {
	case mcpav1.CallStatus_CALL_STATUS_SUCCEEDED:
		return "succeeded"
	case mcpav1.CallStatus_CALL_STATUS_PROVIDER_ERROR:
		return "provider_error"
	case mcpav1.CallStatus_CALL_STATUS_PROVIDER_AUTH_ERROR:
		return "provider_auth_error"
	case mcpav1.CallStatus_CALL_STATUS_CLIENT_CANCELLED:
		return "client_cancelled"
	case mcpav1.CallStatus_CALL_STATUS_STREAM_INTERRUPTED:
		return "stream_interrupted"
	case mcpav1.CallStatus_CALL_STATUS_REJECTED_MODEL_NOT_ALLOWED:
		return "rejected_model_not_allowed"
	case mcpav1.CallStatus_CALL_STATUS_REJECTED_BUDGET:
		return "rejected_budget"
	case mcpav1.CallStatus_CALL_STATUS_REJECTED_TENANT:
		return "rejected_tenant"
	case mcpav1.CallStatus_CALL_STATUS_REJECTED_DEPENDENCY:
		return "rejected_dependency"
	case mcpav1.CallStatus_CALL_STATUS_NO_CREDENTIAL:
		return "no_credential"
	case mcpav1.CallStatus_CALL_STATUS_INTERNAL_ERROR:
		return "internal_error"
	default:
		return "unspecified"
	}
}

// formatLabel returns the bounded metric label for an endpoint format.
func formatLabel(format mcpav1.EndpointFormat) string {
	switch format {
	case mcpav1.EndpointFormat_ENDPOINT_FORMAT_OPENAI:
		return "openai"
	case mcpav1.EndpointFormat_ENDPOINT_FORMAT_CLAUDE:
		return "claude"
	case mcpav1.EndpointFormat_ENDPOINT_FORMAT_GEMINI:
		return "gemini"
	default:
		return "unspecified"
	}
}

// statusClassOf maps a provider HTTP status to a StatusClass. Zero means no
// provider response was received.
func statusClassOf(status int) mcpav1.StatusClass {
	switch {
	case status == 0:
		return mcpav1.StatusClass_STATUS_CLASS_NONE
	case status == 429:
		return mcpav1.StatusClass_STATUS_CLASS_429
	case status >= 200 && status < 300:
		return mcpav1.StatusClass_STATUS_CLASS_2XX
	case status >= 300 && status < 400:
		return mcpav1.StatusClass_STATUS_CLASS_3XX
	case status >= 400 && status < 500:
		return mcpav1.StatusClass_STATUS_CLASS_4XX
	case status >= 500:
		return mcpav1.StatusClass_STATUS_CLASS_5XX
	default:
		return mcpav1.StatusClass_STATUS_CLASS_UNSPECIFIED
	}
}

// rejectionOutcome maps a rejection to the recorded status and error class.
// Engine.ServeHTTP never records a RejectionInvalidCallKey call, so the first
// case only keeps the mapping total; no call record carries it.
func rejectionOutcome(code RejectionCode) (mcpav1.CallStatus, ErrorClass) {
	switch code {
	case RejectionInvalidCallKey:
		return mcpav1.CallStatus_CALL_STATUS_UNSPECIFIED, ErrorClassInvalidCallKey
	case RejectionModelNotAllowed:
		return mcpav1.CallStatus_CALL_STATUS_REJECTED_MODEL_NOT_ALLOWED, ErrorClassModelNotAllowed
	case RejectionBudgetExhausted:
		return mcpav1.CallStatus_CALL_STATUS_REJECTED_BUDGET, ErrorClassBudgetExhausted
	case RejectionTenantInactive:
		return mcpav1.CallStatus_CALL_STATUS_REJECTED_TENANT, ErrorClassTenantInactive
	case RejectionDependencyUnavailable:
		return mcpav1.CallStatus_CALL_STATUS_REJECTED_DEPENDENCY, ErrorClassDependencyUnavailable
	case RejectionNoCredential:
		return mcpav1.CallStatus_CALL_STATUS_NO_CREDENTIAL, ErrorClassNoCredential
	case RejectionRequestTooLarge:
		return mcpav1.CallStatus_CALL_STATUS_INTERNAL_ERROR, ErrorClassRequestTooLarge
	default:
		return mcpav1.CallStatus_CALL_STATUS_INTERNAL_ERROR, ErrorClassInternal
	}
}

// outcomeInput is the evidence gathered during a call.
type outcomeInput struct {
	rejection       *Rejection
	executorInvoked bool
	providerStatus  int
	networkError    bool
	streamFailed    bool
	sentStreamData  bool
	clientCancelled bool
	customerStatus  int
}

// outcome is the classified result of a call.
type outcome struct {
	status      mcpav1.CallStatus
	statusClass mcpav1.StatusClass
	errorClass  ErrorClass
	// failureClass is the provider failure metric label, empty when the
	// provider did not fail.
	failureClass string
}

// classify derives the call outcome. Client cancellation wins over provider
// results because the caller stopped receiving bytes; a stream failure after
// payload bytes reached the caller is an interruption.
func classify(in outcomeInput) outcome {
	if in.rejection != nil {
		status, class := rejectionOutcome(in.rejection.Code)
		return outcome{status: status, statusClass: mcpav1.StatusClass_STATUS_CLASS_NONE, errorClass: class}
	}
	out := outcome{statusClass: statusClassOf(in.providerStatus)}
	if !in.executorInvoked {
		out.statusClass = mcpav1.StatusClass_STATUS_CLASS_NONE
		switch {
		case in.clientCancelled:
			out.status, out.errorClass = mcpav1.CallStatus_CALL_STATUS_CLIENT_CANCELLED, ErrorClassClientCancelled
		case in.customerStatus >= 400:
			// The SDK refused before selecting a credential, for example when
			// the resource has no credential for the model.
			out.status, out.errorClass = mcpav1.CallStatus_CALL_STATUS_NO_CREDENTIAL, ErrorClassNoCredential
		default:
			out.status = mcpav1.CallStatus_CALL_STATUS_SUCCEEDED
		}
		return out
	}
	switch {
	case in.networkError:
		out.failureClass = "network"
	case in.providerStatus == 401 || in.providerStatus == 403:
		out.failureClass = "auth"
	case in.providerStatus == 429:
		out.failureClass = "429"
	case in.providerStatus >= 500:
		out.failureClass = "5xx"
	case in.providerStatus >= 400:
		out.failureClass = "4xx"
	}
	switch {
	case in.clientCancelled:
		out.status, out.errorClass = mcpav1.CallStatus_CALL_STATUS_CLIENT_CANCELLED, ErrorClassClientCancelled
	case in.streamFailed && in.sentStreamData:
		out.status, out.errorClass = mcpav1.CallStatus_CALL_STATUS_STREAM_INTERRUPTED, ErrorClassStreamInterrupted
		if out.failureClass == "" {
			out.failureClass = "network"
		}
	case in.networkError:
		out.status, out.errorClass = mcpav1.CallStatus_CALL_STATUS_PROVIDER_ERROR, ErrorClassNetwork
	case in.providerStatus == 401 || in.providerStatus == 403:
		out.status, out.errorClass = mcpav1.CallStatus_CALL_STATUS_PROVIDER_AUTH_ERROR, ErrorClassProviderAuth
	case in.providerStatus == 429:
		out.status, out.errorClass = mcpav1.CallStatus_CALL_STATUS_PROVIDER_ERROR, ErrorClassRateLimited
	case in.providerStatus >= 500:
		out.status, out.errorClass = mcpav1.CallStatus_CALL_STATUS_PROVIDER_ERROR, ErrorClassProvider5xx
	case in.providerStatus >= 400:
		out.status, out.errorClass = mcpav1.CallStatus_CALL_STATUS_PROVIDER_ERROR, ErrorClassProvider4xx
	case in.streamFailed:
		out.status, out.errorClass = mcpav1.CallStatus_CALL_STATUS_PROVIDER_ERROR, ErrorClassNetwork
		out.failureClass = "network"
	case in.customerStatus >= 500:
		out.status, out.errorClass = mcpav1.CallStatus_CALL_STATUS_INTERNAL_ERROR, ErrorClassInternal
	default:
		out.status = mcpav1.CallStatus_CALL_STATUS_SUCCEEDED
	}
	return out
}
