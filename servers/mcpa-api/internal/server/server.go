// SPDX-License-Identifier: FSL-1.1-ALv2

// Package server assembles the HTTP surfaces of mcpa-api.
package server

import (
	"context"
	"crypto/subtle"
	"encoding/json"
	"log/slog"
	"net/http"
	"strings"
	"time"

	"connectrpc.com/connect"
	"github.com/prometheus/client_golang/prometheus/promhttp"
	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	"go.opentelemetry.io/otel/propagation"
	"go.opentelemetry.io/otel/trace"

	"github.com/delinoio/apps/protos/mcpa/v1/mcpav1connect"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/obs"
	"github.com/delinoio/apps/servers/mcpa-api/internal/web"
)

// RequestIDHeader carries the per-request correlation ID on every response.
// Outside the call surface its value is the trace ID; the call surface
// replaces it with the call record ID.
const RequestIDHeader = "X-Mcpa-Request-Id"

// Mount is an HTTP route contributed by a later layer, for example
// "/r/{resource_id}/", "/mcp", "/oauth/" or "/.well-known/". Pattern uses
// net/http ServeMux syntax. Surface labels metrics and spans. The handler is
// wrapped with tracing, the trace ID and metrics, but not with CORS or the
// console Origin policy: each surface owns its own browser policy.
type Mount struct {
	Pattern string
	Handler http.Handler
	Surface obs.Surface
	// SelfMetered marks a handler that counts its own requests in the shared
	// mcpa_http_requests_total counter, such as the data-plane engine, so
	// the wrapper does not count them twice.
	SelfMetered bool
}

// Readiness reports whether the API can serve. It returns nil when ready.
type Readiness func(ctx context.Context) error

// Config configures the handler.
type Config struct {
	Service       mcpav1connect.McpaServiceHandler
	Authenticator *auth.Authenticator
	Web           *web.Handler
	Metrics       *obs.Metrics
	Log           *slog.Logger
	WebOrigin     string
	MetricsToken  string
	Ready         Readiness
	Mounts        []Mount
}

// New builds the root handler.
func New(cfg Config) http.Handler {
	mux := http.NewServeMux()
	s := &server{cfg: cfg}

	path, connectHandler := mcpav1connect.NewMcpaServiceHandler(cfg.Service, append(handlerCodecs(), connect.WithInterceptors(s.interceptor()))...)
	mux.Handle(path, s.wrap(obs.SurfaceManagement, s.browserPolicy(connectHandler, false)))

	cfg.Web.Register(mux, func(h http.Handler) http.Handler {
		return s.wrap(obs.SurfaceAuth, s.browserPolicy(h, true))
	})
	// CORS preflight for the console's POST /auth/refresh and /auth/logout.
	mux.Handle("OPTIONS /auth/", s.wrap(obs.SurfaceAuth, s.browserPolicy(http.NotFoundHandler(), true)))

	mux.Handle("GET /healthz", s.wrap(obs.SurfaceInternal, http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		writeJSON(w, http.StatusOK, map[string]string{"status": "ok"})
	})))
	mux.Handle("GET /readyz", s.wrap(obs.SurfaceInternal, http.HandlerFunc(s.ready)))
	mux.Handle("GET /metrics", s.wrap(obs.SurfaceInternal, s.metricsHandler()))

	for _, m := range cfg.Mounts {
		mux.Handle(m.Pattern, s.wrapMount(m))
	}
	return withRequestID(mux)
}

// withRequestID assigns the trace ID before routing, so that responses the
// ServeMux generates itself (404, 405 and redirects) carry X-Mcpa-Request-Id
// too. Per-route instrumentation reuses this ID. The call surface still
// replaces the header value with the call record ID. The header is set with
// Header.Set, so the last writer wins.
func withRequestID(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		traceID := apperr.NewTraceID()
		w.Header().Set(RequestIDHeader, traceID)
		next.ServeHTTP(w, r.WithContext(apperr.WithTraceID(r.Context(), traceID)))
	})
}

type server struct {
	cfg Config
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

// statusWriter records the response status for metrics and spans.
type statusWriter struct {
	http.ResponseWriter
	status int
}

func (w *statusWriter) WriteHeader(code int) {
	if w.status == 0 {
		w.status = code
	}
	w.ResponseWriter.WriteHeader(code)
}

func (w *statusWriter) Write(b []byte) (int, error) {
	if w.status == 0 {
		w.status = http.StatusOK
	}
	return w.ResponseWriter.Write(b)
}

// Flush supports streaming responses of later layers.
func (w *statusWriter) Flush() {
	if f, ok := w.ResponseWriter.(http.Flusher); ok {
		if w.status == 0 {
			w.status = http.StatusOK
		}
		f.Flush()
	}
}

// Unwrap lets http.ResponseController reach the underlying writer.
func (w *statusWriter) Unwrap() http.ResponseWriter { return w.ResponseWriter }

// wrap adds the trace ID, a server span and request metrics. Spans carry
// only the surface, method and status: paths can contain resource IDs and
// query strings can contain call keys.
func (s *server) wrap(surface obs.Surface, next http.Handler) http.Handler {
	return s.instrument(surface, true, next)
}

func (s *server) wrapMount(m Mount) http.Handler {
	return s.instrument(m.Surface, !m.SelfMetered, m.Handler)
}

func (s *server) instrument(surface obs.Surface, count bool, next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// withRequestID normally assigned the ID before routing; generate one
		// only when a route is invoked without it.
		ctx := r.Context()
		traceID := apperr.TraceID(ctx)
		if traceID == "" {
			traceID = apperr.NewTraceID()
			ctx = apperr.WithTraceID(ctx, traceID)
		}
		ctx = obs.WithSurface(ctx, surface)
		ctx = otel.GetTextMapPropagator().Extract(ctx, propagation.HeaderCarrier(r.Header))
		ctx, span := obs.Tracer().Start(ctx, string(surface)+" "+r.Method,
			trace.WithSpanKind(trace.SpanKindServer),
			trace.WithAttributes(attribute.String("http.request.method", r.Method), attribute.String("mcpa.surface", string(surface)), attribute.String("mcpa.trace_id", traceID)))
		defer span.End()
		if w.Header().Get(RequestIDHeader) == "" {
			w.Header().Set(RequestIDHeader, traceID)
		}
		sw := &statusWriter{ResponseWriter: w}
		next.ServeHTTP(sw, r.WithContext(ctx))
		status := sw.status
		if status == 0 {
			status = http.StatusOK
		}
		span.SetAttributes(attribute.Int("http.response.status_code", status))
		if status >= 500 {
			span.SetStatus(codes.Error, http.StatusText(status))
		}
		if count && surface != obs.SurfaceInternal && s.cfg.Metrics != nil {
			s.cfg.Metrics.HTTPRequests.WithLabelValues(string(surface), obs.CodeClass(status)).Inc()
		}
	})
}

// Connect and CORS header lists, following connectrpc.com/cors.
var (
	corsAllowMethods = "GET, POST, OPTIONS"
	corsAllowHeaders = strings.Join([]string{
		"Content-Type", "Connect-Protocol-Version", "Connect-Timeout-Ms", "Connect-Accept-Encoding", "Connect-Content-Encoding",
		"Grpc-Timeout", "X-Grpc-Web", "X-User-Agent", "Authorization", auth.CSRFHeader,
	}, ", ")
	corsExposeHeaders = strings.Join([]string{
		"Grpc-Status", "Grpc-Message", "Grpc-Status-Details-Bin", "Connect-Content-Encoding", "Connect-Accept-Encoding", RequestIDHeader,
	}, ", ")
)

// browserPolicy applies credentialed CORS for the console origin and rejects
// requests whose present Origin is another origin before the handler runs.
// Top-level GET navigations to /auth/login and /auth/callback carry no
// trusted Origin and are admitted when navigation is true.
func (s *server) browserPolicy(next http.Handler, navigation bool) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		origin := r.Header.Get("Origin")
		w.Header().Add("Vary", "Origin")
		if origin != "" && origin == s.cfg.WebOrigin {
			h := w.Header()
			h.Set("Access-Control-Allow-Origin", origin)
			h.Set("Access-Control-Allow-Credentials", "true")
			h.Set("Access-Control-Expose-Headers", corsExposeHeaders)
			if r.Method == http.MethodOptions && r.Header.Get("Access-Control-Request-Method") != "" {
				h.Set("Access-Control-Allow-Methods", corsAllowMethods)
				h.Set("Access-Control-Allow-Headers", corsAllowHeaders)
				h.Set("Access-Control-Max-Age", "7200")
				w.WriteHeader(http.StatusNoContent)
				return
			}
		} else if origin != "" {
			isNavigation := navigation && r.Method == http.MethodGet &&
				(r.URL.Path == "/auth/login" || r.URL.Path == "/auth/callback")
			if !isNavigation {
				writeJSON(w, http.StatusForbidden, map[string]string{"error": "origin_not_allowed"})
				return
			}
		}
		next.ServeHTTP(w, r)
	})
}

// interceptor authenticates every management call and renders failures
// with a typed ErrorDetail.
func (s *server) interceptor() connect.UnaryInterceptorFunc {
	return func(next connect.UnaryFunc) connect.UnaryFunc {
		return func(ctx context.Context, req connect.AnyRequest) (connect.AnyResponse, error) {
			start := time.Now()
			procedure := req.Spec().Procedure
			var p *auth.Principal
			var err error
			if requestDecodeFailed(req.Any()) {
				// The codecs rejected the body. Fail before authentication, as
				// connect-go did before, with a fixed message and no content.
				err = apperr.InvalidInput(invalidInputMessage)
			} else {
				p, err = s.cfg.Authenticator.Request(ctx, req.Header())
			}
			if err == nil {
				ctx = auth.WithPrincipal(ctx, p)
				var resp connect.AnyResponse
				resp, err = next(ctx, req)
				if err == nil {
					s.cfg.Log.DebugContext(ctx, "management call", "procedure", procedure, "duration_ms", time.Since(start).Milliseconds())
					return resp, nil
				}
			}
			e := apperr.As(err)
			attrs := []any{"procedure", procedure, "code", e.Code.String(), "reason", e.Reason.String(), "duration_ms", time.Since(start).Milliseconds()}
			if p != nil {
				attrs = append(attrs, "actor_kind", p.Kind.String())
			}
			switch e.Code {
			case connect.CodeInternal, connect.CodeUnavailable, connect.CodeUnknown, connect.CodeDataLoss:
				if e.Cause != nil {
					attrs = append(attrs, "error", e.Cause.Error())
				}
				s.cfg.Log.ErrorContext(ctx, "management call failed", attrs...)
			default:
				s.cfg.Log.InfoContext(ctx, "management call rejected", attrs...)
			}
			if span := trace.SpanFromContext(ctx); span.IsRecording() {
				span.SetAttributes(attribute.String("rpc.method", procedure), attribute.String("mcpa.error_reason", e.Reason.String()))
			}
			return nil, apperr.ToConnect(ctx, e)
		}
	}
}

func (s *server) ready(w http.ResponseWriter, r *http.Request) {
	ctx, cancel := context.WithTimeout(r.Context(), 3*time.Second)
	defer cancel()
	if s.cfg.Ready != nil {
		if err := s.cfg.Ready(ctx); err != nil {
			s.cfg.Log.WarnContext(ctx, "readiness check failed", "error", err)
			writeJSON(w, http.StatusServiceUnavailable, map[string]string{"status": "unavailable"})
			return
		}
	}
	writeJSON(w, http.StatusOK, map[string]string{"status": "ready"})
}

func (s *server) metricsHandler() http.Handler {
	h := promhttp.HandlerFor(s.cfg.Metrics.Registry, promhttp.HandlerOpts{})
	if s.cfg.MetricsToken == "" {
		return h
	}
	want := []byte(s.cfg.MetricsToken)
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		token, ok := auth.BearerToken(r.Header.Get("Authorization"))
		if !ok || subtle.ConstantTimeCompare([]byte(token), want) != 1 {
			w.Header().Set("WWW-Authenticate", `Bearer realm="metrics"`)
			writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "unauthenticated"})
			return
		}
		h.ServeHTTP(w, r)
	})
}

// UnauthenticatedJSON renders the shared 401 JSON body. Later layers use it
// for HTTP surfaces outside Connect.
func UnauthenticatedJSON(w http.ResponseWriter) {
	writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "unauthenticated"})
}

// HTTPError renders a typed error as the /auth JSON shape, including
// {"error":"mfa_required","reauthentication_url":...}.
func HTTPError(w http.ResponseWriter, err error) { web.WriteError(w, err) }
