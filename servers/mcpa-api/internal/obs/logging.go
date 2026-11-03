// SPDX-License-Identifier: FSL-1.1-ALv2

package obs

import (
	"context"
	"log/slog"

	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
)

// Log attribute keys added from the request context.
const (
	LogKeyTraceID = "trace_id"
	LogKeySurface = "surface"
)

type surfaceKey struct{}

// WithSurface stores the HTTP surface that serves the request so that log
// lines written with a *Context method carry it.
func WithSurface(ctx context.Context, s Surface) context.Context {
	return context.WithValue(ctx, surfaceKey{}, s)
}

// SurfaceFrom returns the surface stored by WithSurface, or an empty string.
func SurfaceFrom(ctx context.Context) Surface {
	s, _ := ctx.Value(surfaceKey{}).(Surface)
	return s
}

// contextHandler decorates every record logged with a context that carries a
// request identity. It appends trace_id and surface, so call sites must not
// add those attributes themselves. Records logged without a request context,
// such as startup and background logs, pass through unchanged.
type contextHandler struct {
	next slog.Handler
}

func newContextHandler(next slog.Handler) slog.Handler { return &contextHandler{next: next} }

func (h *contextHandler) Enabled(ctx context.Context, level slog.Level) bool {
	return h.next.Enabled(ctx, level)
}

func (h *contextHandler) Handle(ctx context.Context, r slog.Record) error {
	surface := SurfaceFrom(ctx)
	// The call surface replaces X-Mcpa-Request-Id with the call record ID, so
	// the request trace ID would not match the ID the caller sees. Its own log
	// lines carry the call ID instead.
	if id := apperr.TraceID(ctx); id != "" && surface != SurfaceProxy {
		r.AddAttrs(slog.String(LogKeyTraceID, id))
	}
	if surface != "" {
		r.AddAttrs(slog.String(LogKeySurface, string(surface)))
	}
	return h.next.Handle(ctx, r)
}

// WithAttrs and WithGroup keep the wrapper so derived loggers still decorate.
func (h *contextHandler) WithAttrs(attrs []slog.Attr) slog.Handler {
	return &contextHandler{next: h.next.WithAttrs(attrs)}
}

func (h *contextHandler) WithGroup(name string) slog.Handler {
	return &contextHandler{next: h.next.WithGroup(name)}
}
