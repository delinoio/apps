// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"bytes"
	"context"
	"io"
	"log/slog"
	"net/http"
	"slices"
	"strconv"
	"strings"
	"sync"
	"unicode/utf8"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/gin-gonic/gin"
	"github.com/tidwall/gjson"
)

// ResourcePathPrefix is the path prefix of model-call endpoints.
const ResourcePathPrefix = "/r/"

// maxModelNameBytes bounds a requested model name. It equals the longest name
// an allowlist or credential can hold, so a longer name can never be allowed
// or served. Longer names are rejected before the allowlist check and are
// truncated wherever the call state or a log line carries them.
const maxModelNameBytes = 200

// truncateModelName shortens s to at most maxModelNameBytes bytes without
// splitting a UTF-8 sequence. It does not change shorter names.
func truncateModelName(s string) string {
	if len(s) <= maxModelNameBytes {
		return s
	}
	n := maxModelNameBytes
	for n > 0 && !utf8.RuneStart(s[n]) {
		n--
	}
	return s[:n]
}

type routeKind int

const (
	routeCall routeKind = iota + 1
	routeOpenAIModels
	routeClaudeModels
	routeGeminiModels
	routeGeminiModel
)

// route is a matched model-call endpoint.
type route struct {
	kind      routeKind
	format    mcpav1.EndpointFormat
	streaming bool
	// model is the model named in the path (Gemini only).
	model string
}

func isGeminiCallMethod(method string) bool {
	switch method {
	case "generateContent", "streamGenerateContent", "countTokens":
		return true
	default:
		return false
	}
}

// matchRoute matches the path below /r/{resource_id}. Only the endpoints in
// the server contract are exposed; WebSocket and other SDK routes are not.
func matchRoute(method, path string, header http.Header) (route, bool) {
	switch {
	case method == http.MethodPost && (path == "/v1/chat/completions" || path == "/v1/completions" || path == "/v1/responses"):
		return route{kind: routeCall, format: mcpav1.EndpointFormat_ENDPOINT_FORMAT_OPENAI}, true
	case method == http.MethodPost && (path == "/v1/messages" || path == "/v1/messages/count_tokens"):
		return route{kind: routeCall, format: mcpav1.EndpointFormat_ENDPOINT_FORMAT_CLAUDE}, true
	case method == http.MethodGet && path == "/v1/models":
		// Claude clients send anthropic-version; the SDK's unified handler
		// also routes claude-cli user agents to the Claude listing.
		if header.Get("Anthropic-Version") != "" || strings.HasPrefix(header.Get("User-Agent"), "claude-cli") {
			return route{kind: routeClaudeModels, format: mcpav1.EndpointFormat_ENDPOINT_FORMAT_CLAUDE}, true
		}
		return route{kind: routeOpenAIModels, format: mcpav1.EndpointFormat_ENDPOINT_FORMAT_OPENAI}, true
	case method == http.MethodGet && path == "/v1beta/models":
		return route{kind: routeGeminiModels, format: mcpav1.EndpointFormat_ENDPOINT_FORMAT_GEMINI}, true
	case strings.HasPrefix(path, "/v1beta/models/"):
		action := strings.TrimPrefix(path, "/v1beta/models/")
		if method == http.MethodGet && action != "" && !strings.ContainsAny(action, ":/") {
			return route{kind: routeGeminiModel, format: mcpav1.EndpointFormat_ENDPOINT_FORMAT_GEMINI, model: action}, true
		}
		model, call, ok := strings.Cut(action, ":")
		if method != http.MethodPost || !ok || model == "" || strings.Contains(model, "/") || !isGeminiCallMethod(call) {
			return route{}, false
		}
		return route{
			kind:      routeCall,
			format:    mcpav1.EndpointFormat_ENDPOINT_FORMAT_GEMINI,
			model:     model,
			streaming: call == "streamGenerateContent",
		}, true
	}
	return route{}, false
}

// splitResourcePath splits "/r/{id}/rest" into id and "/rest".
func splitResourcePath(path string) (string, string, bool) {
	if !strings.HasPrefix(path, ResourcePathPrefix) {
		return "", "", false
	}
	id, rest, ok := strings.Cut(strings.TrimPrefix(path, ResourcePathPrefix), "/")
	if !ok || id == "" {
		return "", "", false
	}
	return id, "/" + rest, true
}

// callKeyFrom returns the call key from the accepted credential locations:
// Authorization: Bearer, x-api-key, x-goog-api-key, then the Gemini ?key=
// query parameter.
func callKeyFrom(r *http.Request) string {
	if value := strings.TrimSpace(r.Header.Get("Authorization")); value != "" {
		if scheme, token, ok := strings.Cut(value, " "); ok && strings.EqualFold(scheme, "Bearer") {
			if token = strings.TrimSpace(token); token != "" {
				return token
			}
		}
	}
	for _, name := range []string{"X-Api-Key", "X-Goog-Api-Key"} {
		if value := strings.TrimSpace(r.Header.Get(name)); value != "" {
			return value
		}
	}
	return strings.TrimSpace(r.URL.Query().Get("key"))
}

// modelAllowed applies the resource allowlist. An empty list allows every
// model; otherwise the requested model string must be listed exactly. A
// suffixed name such as "model(high)" is a different name than "model": the
// SDK rewrites it to the base name before the provider call, so matching on
// the base name would let an unlisted name reach a listed model.
func modelAllowed(model string, allowlist []string) bool {
	if len(allowlist) == 0 {
		return true
	}
	return slices.Contains(allowlist, model)
}

type runtimeContextKey struct{}

// ServeHTTP serves /r/{resource_id}/... model calls.
func (e *Engine) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	resourceID, rest, ok := splitResourcePath(r.URL.Path)
	if !ok {
		http.NotFound(w, r)
		return
	}
	rt, ok := matchRoute(r.Method, rest, r.Header)
	callID := e.opts.NewCallID()
	w.Header().Set(RequestIDHeader, callID)
	if !ok {
		writeRejection(w, mcpav1.EndpointFormat_ENDPOINT_FORMAT_OPENAI, Reject(RejectionNotFound, "unknown endpoint"))
		e.opts.Metrics.httpRequest(http.StatusNotFound)
		return
	}
	ctx := r.Context()
	principal, err := e.opts.Keys.AuthenticateCallKey(ctx, resourceID, callKeyFrom(r))
	if err == nil && principal.ResourceID != resourceID {
		err = Reject(RejectionInvalidCallKey, "the call key is not valid for this resource")
	}
	if err != nil {
		// Unauthenticated requests are not attributable to a tenant and are
		// not metered.
		rejection := asRejection(err, RejectionDependencyUnavailable)
		writeRejection(w, rt.format, rejection)
		e.opts.Metrics.httpRequest(rejection.Code.HTTPStatus())
		return
	}

	call := &callState{
		id:         callID,
		resourceID: resourceID,
		principal:  principal,
		format:     rt.format,
		// The path model is unbounded; serveCall rejects an overlong one.
		model:     truncateModelName(rt.model),
		streaming: rt.streaming,
		start:     e.opts.Now(),
	}
	cw := newCountingResponseWriter(w, call)
	var result outcome
	record := func(final bool) CallRecordUpdate {
		call.mu.Lock()
		update := CallRecordUpdate{
			CallID:       call.id,
			ResourceID:   call.resourceID,
			TenantID:     principal.TenantID,
			CallKeyID:    principal.CallKeyID,
			CredentialID: call.credentialID,
			Provider:     call.provider,
			Model:        call.model,
			Format:       call.format,
			Streaming:    call.streaming,
			StartTime:    call.start,
		}
		call.mu.Unlock()
		if final {
			update.Status = result.status
			update.ProviderStatusClass = result.statusClass
			update.ErrorClass = result.errorClass
			update.Latency = e.opts.Now().Sub(call.start)
		}
		return update
	}
	stop := make(chan struct{})
	var wg sync.WaitGroup
	wg.Go(func() { e.cp.run(ctx, call, record, stop) })

	e.serveCall(cw, r, call, rt, rest)

	close(stop)
	wg.Wait()

	// An invalid_call_key rejection raised after the call state exists (a
	// permission denial or a resource that is gone at the allowlist read)
	// means the key is no longer valid, so the request is not attributable to
	// a valid key. Like the rejections before the call state exists, it is
	// not recorded, metered or billed. The 401 response is already written.
	if call.invalidCallKeyRejected() {
		e.opts.Metrics.httpRequest(cw.statusCode())
		e.logger.Info("model call rejected",
			slog.String("call_id", call.id),
			slog.String("resource_id", call.resourceID),
			slog.String("call_key_id", principal.CallKeyID),
			slog.String("format", formatLabel(call.format)),
			slog.String("error_class", string(ErrorClassInvalidCallKey)),
			slog.Int("http_status", cw.statusCode()),
		)
		return
	}
	result = classify(call.outcomeInput(ctx.Err() != nil, cw.statusCode()))
	latency := e.opts.Now().Sub(call.start)
	e.cp.commit(ctx, call, record, true)

	call.mu.Lock()
	provider := call.provider
	call.mu.Unlock()
	e.opts.Metrics.observeCall(formatLabel(call.format), providerLabel(provider), statusLabel(result.status), latency)
	e.opts.Metrics.providerFailure(providerLabel(provider), result.failureClass)
	e.opts.Metrics.httpRequest(cw.statusCode())
	e.logger.Info("model call",
		slog.String("call_id", call.id),
		slog.String("resource_id", call.resourceID),
		slog.String("call_key_id", principal.CallKeyID),
		slog.String("format", formatLabel(call.format)),
		slog.String("provider", providerLabel(provider)),
		slog.String("model", call.model),
		slog.String("status", statusLabel(result.status)),
		slog.String("error_class", string(result.errorClass)),
		slog.Int("http_status", cw.statusCode()),
		slog.Int64("request_bytes", call.requestBytes.Load()),
		slog.Int64("response_bytes", call.responseBytes.Load()),
		slog.Int64("latency_ms", latency.Milliseconds()),
	)
}

// serveCall applies the gates and runs the SDK handler. Every rejection is
// written before any provider contact.
func (e *Engine) serveCall(w *countingResponseWriter, r *http.Request, call *callState, rt route, rest string) {
	ctx := r.Context()
	reject := func(rejection *Rejection) {
		call.reject(rejection)
		if rejection.Code == RejectionBudgetExhausted {
			e.opts.Metrics.budgetRejection(rejection.BudgetScope)
		}
		writeRejection(w, rt.format, rejection)
	}

	if err := e.opts.Authorizer.AuthorizeCall(ctx, call.principal); err != nil {
		reject(asRejection(err, RejectionDependencyUnavailable))
		return
	}

	var body []byte
	if rt.kind == routeCall {
		var err error
		body, err = io.ReadAll(io.LimitReader(r.Body, e.opts.MaxRequestBodyBytes+1))
		if err != nil {
			reject(Reject(RejectionInternal, "the request body could not be read"))
			return
		}
		if int64(len(body)) > e.opts.MaxRequestBodyBytes {
			reject(Reject(RejectionRequestTooLarge, "the request body is too large"))
			return
		}
		if rt.format != mcpav1.EndpointFormat_ENDPOINT_FORMAT_GEMINI {
			// The gate reads the first "model" key but the body is forwarded
			// unchanged, so a decoder that reads another occurrence would
			// serve a model the allowlist never saw. The message must not
			// echo any model value from the body.
			if hasDuplicateTopLevelKey(body, "model") {
				reject(Reject(RejectionModelNotAllowed, "the request body names the model more than once"))
				return
			}
			model := strings.TrimSpace(gjson.GetBytes(body, "model").String())
			call.mu.Lock()
			call.model = truncateModelName(model)
			call.mu.Unlock()
			if len(model) > maxModelNameBytes {
				reject(Reject(RejectionModelNotAllowed, "the model name is too long"))
				return
			}
			call.mu.Lock()
			call.streaming = gjson.GetBytes(body, "stream").Bool()
			call.mu.Unlock()
		}
	}

	// The Gemini path model is not covered by the body limit. The message must
	// not echo the model.
	if len(rt.model) > maxModelNameBytes {
		reject(Reject(RejectionModelNotAllowed, "the model name is too long"))
		return
	}

	allowlist, err := e.opts.Routing.ModelAllowlist(ctx, call.resourceID)
	if err != nil {
		reject(asRejection(err, RejectionDependencyUnavailable))
		return
	}
	if rt.kind == routeCall {
		if call.model == "" {
			reject(Reject(RejectionModelNotAllowed, "the request does not name a model"))
			return
		}
		if !modelAllowed(call.model, allowlist) {
			reject(Reject(RejectionModelNotAllowed, "model "+strconv.Quote(call.model)+" is not allowed for this resource"))
			return
		}
	}

	// Budgets come after the allowlist so that a disallowed model is reported
	// as model_not_allowed and never counts as a budget rejection. Listings
	// keep the budget gate they always had.
	if err := e.opts.Authorizer.CheckBudgets(ctx, call.principal); err != nil {
		reject(asRejection(err, RejectionDependencyUnavailable))
		return
	}

	runtime, err := e.runtime(ctx, call.resourceID)
	if err != nil {
		e.logger.Warn("loading resource runtime failed",
			slog.String("resource_id", call.resourceID),
			slog.String("error", err.Error()))
		reject(Reject(RejectionDependencyUnavailable, "the resource could not be loaded right now"))
		return
	}

	switch rt.kind {
	case routeOpenAIModels, routeClaudeModels, routeGeminiModels, routeGeminiModel:
		writeModelListing(w, rt, runtime.models(), allowlist)
		return
	}
	if !runtime.serves(call.model) {
		reject(Reject(RejectionNoCredential, "no credential of this resource serves model "+strconv.Quote(call.model)))
		return
	}

	// The SDK sees the standard path without the resource prefix and without
	// the caller's call key, so no executor can forward the key upstream.
	sdkReq := r.Clone(context.WithValue(withCall(ctx, call), runtimeContextKey{}, runtime))
	sdkReq.URL.Path = rest
	sdkReq.URL.RawPath = ""
	query := sdkReq.URL.Query()
	query.Del("key")
	sdkReq.URL.RawQuery = query.Encode()
	sdkReq.RequestURI = sdkReq.URL.RequestURI()
	for _, name := range []string{"Authorization", "X-Api-Key", "X-Goog-Api-Key"} {
		sdkReq.Header.Del(name)
	}
	sdkReq.Body = io.NopCloser(bytes.NewReader(body))
	sdkReq.ContentLength = int64(len(body))
	sdkReq.GetBody = func() (io.ReadCloser, error) { return io.NopCloser(bytes.NewReader(body)), nil }
	e.router.ServeHTTP(w, sdkReq)
}

// newRouter mounts the SDK handlers. Routes resolve the resource runtime the
// call middleware placed in the request context.
func (e *Engine) newRouter() *gin.Engine {
	router := gin.New()
	router.Use(func(c *gin.Context) {
		defer func() {
			if recovered := recover(); recovered != nil {
				e.logger.Error("sdk handler panic", slog.Any("panic", recovered))
				if !c.Writer.Written() {
					c.AbortWithStatusJSON(http.StatusInternalServerError, gin.H{"error": gin.H{"message": "internal error", "type": "internal_error"}})
				}
			}
		}()
		c.Next()
	})
	runtimeOf := func(c *gin.Context) *resourceRuntime {
		rt, _ := c.Request.Context().Value(runtimeContextKey{}).(*resourceRuntime)
		return rt
	}
	handle := func(serve func(*resourceRuntime, *gin.Context)) gin.HandlerFunc {
		return func(c *gin.Context) {
			rt := runtimeOf(c)
			if rt == nil {
				c.AbortWithStatus(http.StatusNotFound)
				return
			}
			serve(rt, c)
		}
	}
	router.POST("/v1/chat/completions", handle(func(rt *resourceRuntime, c *gin.Context) { rt.openai.ChatCompletions(c) }))
	router.POST("/v1/completions", handle(func(rt *resourceRuntime, c *gin.Context) { rt.openai.Completions(c) }))
	router.POST("/v1/responses", handle(func(rt *resourceRuntime, c *gin.Context) { rt.responses.Responses(c) }))
	router.POST("/v1/messages", handle(func(rt *resourceRuntime, c *gin.Context) { rt.claude.ClaudeMessages(c) }))
	router.POST("/v1/messages/count_tokens", handle(func(rt *resourceRuntime, c *gin.Context) { rt.claude.ClaudeCountTokens(c) }))
	router.POST("/v1beta/models/*action", handle(func(rt *resourceRuntime, c *gin.Context) { rt.gemini.GeminiHandler(c) }))
	return router
}
