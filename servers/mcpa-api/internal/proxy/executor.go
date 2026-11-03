// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"context"
	"fmt"
	"log/slog"
	"net/http"
	"strings"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	coreauth "github.com/router-for-me/CLIProxyAPI/v6/sdk/cliproxy/auth"
	cliproxyexecutor "github.com/router-for-me/CLIProxyAPI/v6/sdk/cliproxy/executor"
)

const (
	claudeDefaultBaseURL = "https://api.anthropic.com"
	// kimiClaudeBaseURL is the endpoint the SDK Kimi executor forces for
	// Claude-format requests (internal/auth/kimi.KimiAPIBaseURL).
	kimiClaudeBaseURL       = "https://api.kimi.com/coding"
	claudeMessagesURI       = "/v1/messages?beta=true"
	claudeCountTokensURI    = "/v1/messages/count_tokens?beta=true"
	sourceFormatClaudeLabel = "claude"
)

// providerFromSDK maps an SDK executor key to the mcpa provider enum.
func providerFromSDK(key string) mcpav1.Provider {
	switch key {
	case sdkProviderGemini:
		return mcpav1.Provider_PROVIDER_GEMINI
	case sdkProviderVertex:
		return mcpav1.Provider_PROVIDER_VERTEX
	case sdkProviderGeminiCLI:
		return mcpav1.Provider_PROVIDER_GEMINI_CLI
	case sdkProviderAntigravity:
		return mcpav1.Provider_PROVIDER_ANTIGRAVITY
	case sdkProviderClaude:
		return mcpav1.Provider_PROVIDER_CLAUDE
	case sdkProviderCodex:
		return mcpav1.Provider_PROVIDER_CODEX
	case sdkProviderKimi:
		return mcpav1.Provider_PROVIDER_KIMI
	case sdkProviderOpenAICompat:
		return mcpav1.Provider_PROVIDER_OPENAI_COMPATIBLE
	default:
		return mcpav1.Provider_PROVIDER_UNSPECIFIED
	}
}

// isolationError is returned when an auth of another resource reaches an
// executor. It cannot happen with per-resource managers; the check is a
// fail-closed guard.
type isolationError struct{}

func (isolationError) Error() string   { return "credential does not belong to the called resource" }
func (isolationError) StatusCode() int { return http.StatusInternalServerError }

// resourceExecutor wraps a harvested SDK executor for one resource's auth
// manager. It enforces that the selected auth belongs to the resource and to
// the current call, records the selected credential and the executor result,
// and installs trace metering for executors that bypass the metering
// transport.
type resourceExecutor struct {
	inner       coreauth.ProviderExecutor
	resourceID  string
	sdkProvider string
	// relay performs the dials of the SDK Claude client (see
	// upstreamRelay).
	relay  *upstreamRelay
	logger *slog.Logger
}

var (
	_ coreauth.ProviderExecutor       = (*resourceExecutor)(nil)
	_ coreauth.RequestPreparer        = (*resourceExecutor)(nil)
	_ coreauth.ExecutionSessionCloser = (*resourceExecutor)(nil)
)

func (e *resourceExecutor) Identifier() string { return e.inner.Identifier() }

func (e *resourceExecutor) PrepareRequest(req *http.Request, auth *coreauth.Auth) error {
	if err := e.checkAuth(auth); err != nil {
		return err
	}
	if preparer, ok := e.inner.(coreauth.RequestPreparer); ok {
		return preparer.PrepareRequest(req, auth)
	}
	return nil
}

func (e *resourceExecutor) CloseExecutionSession(sessionID string) {
	if closer, ok := e.inner.(coreauth.ExecutionSessionCloser); ok {
		closer.CloseExecutionSession(sessionID)
	}
}

func (e *resourceExecutor) checkAuth(auth *coreauth.Auth) error {
	if auth == nil || auth.Attributes[attrResourceID] != e.resourceID {
		e.logger.Error("credential isolation check failed", slog.String("resource_id", e.resourceID))
		return isolationError{}
	}
	return nil
}

// begin validates the execution and returns the context for the inner
// executor.
func (e *resourceExecutor) begin(ctx context.Context, auth *coreauth.Auth, opts cliproxyexecutor.Options, claudeURI string) (context.Context, *callState, error) {
	if err := e.checkAuth(auth); err != nil {
		return ctx, nil, err
	}
	call := callFromContext(ctx)
	if call == nil {
		return ctx, nil, nil
	}
	if call.resourceID != e.resourceID {
		e.logger.Error("call isolation check failed",
			slog.String("resource_id", e.resourceID),
			slog.String("call_id", call.id))
		return ctx, nil, isolationError{}
	}
	call.selected(auth.Attributes[attrCredentialID], providerFromSDK(e.sdkProvider))
	if e.sdkProvider == sdkProviderKimi {
		call.setKimiDeviceID(kimiDeviceIDFor(auth))
	}
	if baseURL, ok := e.traceMeteringBase(auth, opts); ok {
		ctx = withTraceMetering(ctx, call, requestURIFor(baseURL, claudeURI))
	}
	return ctx, call, nil
}

// guardedAuth returns the auth to pass to the inner executor. For the SDK
// Claude executor it is a copy whose ProxyURL is the upstream relay, so every
// dial of that executor's private client goes through Options.DialUpstream.
// The SDK reads ProxyURL in helps.NewUtlsHTTPClient; mcpa never sets it
// otherwise, and the copy keeps the registered auth unchanged.
//
// The relay URL carries the call ID as SOCKS username, so the relay can close
// the call's tunnels when the call ends (Engine.ServeHTTP). Executions
// without a call use the shared relay identity and rely on the relay idle
// timeout.
func (e *resourceExecutor) guardedAuth(ctx context.Context, auth *coreauth.Auth) *coreauth.Auth {
	if e.sdkProvider != sdkProviderClaude || auth == nil {
		return auth
	}
	out := auth.Clone()
	if call := callFromContext(ctx); call != nil {
		out.ProxyURL = e.relay.URLFor(call.id)
	} else {
		out.ProxyURL = e.relay.URL()
	}
	return out
}

// traceMeteringBase reports whether the inner executor sends its requests
// through the SDK Claude client, which ignores the metering transport, and
// returns the base URL that client uses.
func (e *resourceExecutor) traceMeteringBase(auth *coreauth.Auth, opts cliproxyexecutor.Options) (string, bool) {
	switch e.sdkProvider {
	case sdkProviderClaude:
		if base := strings.TrimSpace(auth.Attributes["base_url"]); base != "" {
			return base, true
		}
		return claudeDefaultBaseURL, true
	case sdkProviderKimi:
		if opts.SourceFormat.String() == sourceFormatClaudeLabel {
			return kimiClaudeBaseURL, true
		}
	}
	return "", false
}

func (e *resourceExecutor) Execute(ctx context.Context, auth *coreauth.Auth, req cliproxyexecutor.Request, opts cliproxyexecutor.Options) (cliproxyexecutor.Response, error) {
	ctx, call, err := e.begin(ctx, auth, opts, claudeMessagesURI)
	if err != nil {
		return cliproxyexecutor.Response{}, err
	}
	resp, err := e.inner.Execute(ctx, e.guardedAuth(ctx, auth), req, opts)
	if call != nil {
		call.observeExecutorResult(err)
	}
	return resp, err
}

func (e *resourceExecutor) CountTokens(ctx context.Context, auth *coreauth.Auth, req cliproxyexecutor.Request, opts cliproxyexecutor.Options) (cliproxyexecutor.Response, error) {
	ctx, call, err := e.begin(ctx, auth, opts, claudeCountTokensURI)
	if err != nil {
		return cliproxyexecutor.Response{}, err
	}
	resp, err := e.inner.CountTokens(ctx, e.guardedAuth(ctx, auth), req, opts)
	if call != nil {
		call.observeExecutorResult(err)
	}
	return resp, err
}

func (e *resourceExecutor) ExecuteStream(ctx context.Context, auth *coreauth.Auth, req cliproxyexecutor.Request, opts cliproxyexecutor.Options) (*cliproxyexecutor.StreamResult, error) {
	ctx, call, err := e.begin(ctx, auth, opts, claudeMessagesURI)
	if err != nil {
		return nil, err
	}
	result, err := e.inner.ExecuteStream(ctx, e.guardedAuth(ctx, auth), req, opts)
	if call == nil {
		return result, err
	}
	call.observeExecutorResult(err)
	if err != nil || result == nil || result.Chunks == nil {
		return result, err
	}
	out := make(chan cliproxyexecutor.StreamChunk)
	go func(in <-chan cliproxyexecutor.StreamChunk) {
		defer close(out)
		for chunk := range in {
			call.observeStreamChunk(chunk.Payload, chunk.Err)
			select {
			case out <- chunk:
			case <-ctx.Done():
				go drainStream(in)
				return
			}
		}
	}(result.Chunks)
	return &cliproxyexecutor.StreamResult{Headers: result.Headers, Chunks: out}, nil
}

func (e *resourceExecutor) Refresh(ctx context.Context, auth *coreauth.Auth) (*coreauth.Auth, error) {
	if err := e.checkAuth(auth); err != nil {
		return nil, err
	}
	return e.inner.Refresh(ctx, auth)
}

func (e *resourceExecutor) HttpRequest(ctx context.Context, auth *coreauth.Auth, req *http.Request) (*http.Response, error) {
	if err := e.checkAuth(auth); err != nil {
		return nil, err
	}
	if call := callFromContext(ctx); call != nil && call.resourceID != e.resourceID {
		return nil, isolationError{}
	}
	return e.inner.HttpRequest(ctx, e.guardedAuth(ctx, auth), req)
}

func drainStream(in <-chan cliproxyexecutor.StreamChunk) {
	for range in {
	}
}

// String is used in log output only.
func (e *resourceExecutor) String() string {
	return fmt.Sprintf("resourceExecutor(%s)", e.sdkProvider)
}
