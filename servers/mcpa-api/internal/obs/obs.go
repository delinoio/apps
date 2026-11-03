// SPDX-License-Identifier: FSL-1.1-ALv2

// Package obs owns logging, Prometheus metrics and OpenTelemetry tracing.
package obs

import (
	"context"
	"io"
	"log/slog"
	"time"

	"github.com/lmittmann/tint"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/collectors"
	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/exporters/otlp/otlptrace/otlptracehttp"
	"go.opentelemetry.io/otel/propagation"
	"go.opentelemetry.io/otel/sdk/resource"
	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/trace"

	"github.com/delinoio/apps/servers/mcpa-api/internal/config"
)

// ServiceName is the OpenTelemetry and log service name.
const ServiceName = "mcpa-api"

// NewLogger builds the process logger. Text logs use ANSI color unless
// NO_COLOR is set; JSON logs never use color. Lines written with a *Context
// method and a request context carry trace_id and surface.
func NewLogger(w io.Writer, l config.Logging) *slog.Logger {
	if l.Format == config.LogFormatJSON {
		return slog.New(newContextHandler(slog.NewJSONHandler(w, &slog.HandlerOptions{Level: slog.LevelInfo})))
	}
	return slog.New(newContextHandler(tint.NewTextHandler(w, &tint.Options{
		Level:      slog.LevelInfo,
		TimeFormat: time.RFC3339,
		NoColor:    l.NoColor,
	})))
}

// Surface labels HTTP request metrics.
type Surface string

// Surfaces with bounded metric label values.
const (
	SurfaceManagement Surface = "management"
	SurfaceProxy      Surface = "proxy"
	SurfaceMCP        Surface = "mcp"
	SurfaceAuth       Surface = "auth"
	SurfaceOAuth      Surface = "oauth"
	// SurfaceInternal covers health, readiness and metrics. It is not counted.
	SurfaceInternal Surface = "internal"
)

// DelcoreFailureKind labels Delcore failures.
type DelcoreFailureKind string

// Delcore failure kinds.
const (
	DelcoreUnavailable DelcoreFailureKind = "unavailable"
	DelcoreDenied      DelcoreFailureKind = "denied"
	DelcoreError       DelcoreFailureKind = "error"
)

// Metrics owns the Prometheus registry. Later layers register their own
// collectors on Registry.
type Metrics struct {
	Registry        *prometheus.Registry
	HTTPRequests    *prometheus.CounterVec
	DelcoreFailures *prometheus.CounterVec
}

// NewMetrics creates the registry with process, Go and core collectors.
func NewMetrics() *Metrics {
	reg := prometheus.NewRegistry()
	reg.MustRegister(collectors.NewGoCollector(), collectors.NewProcessCollector(collectors.ProcessCollectorOpts{}))
	m := &Metrics{
		Registry: reg,
		HTTPRequests: prometheus.NewCounterVec(prometheus.CounterOpts{
			Name: "mcpa_http_requests_total",
			Help: "HTTP requests by surface and status code class.",
		}, []string{"surface", "code_class"}),
		DelcoreFailures: prometheus.NewCounterVec(prometheus.CounterOpts{
			Name: "mcpa_delcore_failures_total",
			Help: "Failed or denied Delcore calls by RPC and kind.",
		}, []string{"rpc", "kind"}),
	}
	reg.MustRegister(m.HTTPRequests, m.DelcoreFailures)
	return m
}

// RecordDelcoreFailure counts one failed Delcore call. A nil receiver is a
// no-op so tests can omit metrics.
func (m *Metrics) RecordDelcoreFailure(rpc string, kind DelcoreFailureKind) {
	if m == nil {
		return
	}
	m.DelcoreFailures.WithLabelValues(rpc, string(kind)).Inc()
}

// CodeClass returns the bounded status class label of an HTTP status.
func CodeClass(status int) string {
	switch {
	case status >= 500:
		return "5xx"
	case status >= 400:
		return "4xx"
	case status >= 300:
		return "3xx"
	case status >= 200:
		return "2xx"
	default:
		return "1xx"
	}
}

// Tracer returns the mcpa tracer from the global provider.
func Tracer() trace.Tracer { return otel.Tracer(ServiceName) }

// SetupTracing installs the OTLP HTTP exporter when enabled. The exporter
// reads the standard OTEL_* variables. The returned function flushes spans.
func SetupTracing(ctx context.Context, enabled bool) (func(context.Context) error, error) {
	otel.SetTextMapPropagator(propagation.NewCompositeTextMapPropagator(propagation.TraceContext{}, propagation.Baggage{}))
	if !enabled {
		return func(context.Context) error { return nil }, nil
	}
	exporter, err := otlptracehttp.New(ctx)
	if err != nil {
		return nil, err
	}
	// Explicit attributes come first so OTEL_SERVICE_NAME and
	// OTEL_RESOURCE_ATTRIBUTES can override them.
	res, err := resource.New(ctx,
		resource.WithTelemetrySDK(),
		resource.WithAttributes(attribute.String("service.name", ServiceName)),
		resource.WithFromEnv(),
	)
	if err != nil {
		return nil, err
	}
	tp := sdktrace.NewTracerProvider(sdktrace.WithBatcher(exporter), sdktrace.WithResource(res))
	otel.SetTracerProvider(tp)
	return tp.Shutdown, nil
}
