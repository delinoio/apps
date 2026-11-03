// SPDX-License-Identifier: FSL-1.1-ALv2

package obs_test

import (
	"bytes"
	"context"
	"encoding/json"
	"log/slog"
	"strings"
	"testing"

	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/config"
	"github.com/delinoio/apps/servers/mcpa-api/internal/obs"
)

func decodeLines(t *testing.T, buf *bytes.Buffer) []map[string]any {
	t.Helper()
	var out []map[string]any
	for _, line := range strings.Split(strings.TrimSpace(buf.String()), "\n") {
		if line == "" {
			continue
		}
		var m map[string]any
		if err := json.Unmarshal([]byte(line), &m); err != nil {
			t.Fatalf("log line is not JSON: %q: %v", line, err)
		}
		out = append(out, m)
	}
	return out
}

func requestContext(surface obs.Surface) context.Context {
	return obs.WithSurface(apperr.WithTraceID(context.Background(), "trace-1"), surface)
}

func TestLoggerAddsRequestIdentityFromContext(t *testing.T) {
	var buf bytes.Buffer
	log := obs.NewLogger(&buf, config.Logging{Format: config.LogFormatJSON})

	log.InfoContext(requestContext(obs.SurfaceOAuth), "in request", "k", "v")
	// Derived loggers keep the decoration.
	log.With("component", "x").WithGroup("g").WarnContext(requestContext(obs.SurfaceMCP), "derived", "k", "v")
	// Logs without a request context, and non-Context calls, are unchanged.
	log.Info("startup", "k", "v")
	log.InfoContext(context.Background(), "background")
	// A trace ID alone (no surface) is still attached.
	log.InfoContext(apperr.WithTraceID(context.Background(), "trace-2"), "trace only")

	lines := decodeLines(t, &buf)
	if len(lines) != 5 {
		t.Fatalf("got %d lines: %s", len(lines), buf.String())
	}
	if lines[0]["trace_id"] != "trace-1" || lines[0]["surface"] != "oauth" || lines[0]["k"] != "v" {
		t.Errorf("in-request line = %v", lines[0])
	}
	g, _ := lines[1]["g"].(map[string]any)
	if lines[1]["component"] != "x" || g["trace_id"] != "trace-1" || g["surface"] != "mcp" {
		// WithGroup nests later attributes, including the context-derived ones.
		t.Errorf("derived line = %v", lines[1])
	}
	for _, i := range []int{2, 3} {
		if _, ok := lines[i]["trace_id"]; ok {
			t.Errorf("line %d gained trace_id: %v", i, lines[i])
		}
		if _, ok := lines[i]["surface"]; ok {
			t.Errorf("line %d gained surface: %v", i, lines[i])
		}
	}
	if lines[4]["trace_id"] != "trace-2" {
		t.Errorf("trace-only line = %v", lines[4])
	}
	if _, ok := lines[4]["surface"]; ok {
		t.Errorf("trace-only line gained surface: %v", lines[4])
	}
}

func TestLoggerOmitsTraceIDOnCallSurface(t *testing.T) {
	var buf bytes.Buffer
	log := obs.NewLogger(&buf, config.Logging{Format: config.LogFormatJSON})
	log.InfoContext(requestContext(obs.SurfaceProxy), "model call")
	lines := decodeLines(t, &buf)
	if len(lines) != 1 {
		t.Fatalf("got %d lines", len(lines))
	}
	// The call surface answers with the call record ID, not the trace ID.
	if _, ok := lines[0]["trace_id"]; ok {
		t.Errorf("call surface line carries trace_id: %v", lines[0])
	}
	if lines[0]["surface"] != "proxy" {
		t.Errorf("surface = %v", lines[0]["surface"])
	}
}

func TestLoggerRespectsLevel(t *testing.T) {
	var buf bytes.Buffer
	log := obs.NewLogger(&buf, config.Logging{Format: config.LogFormatJSON})
	if log.Enabled(context.Background(), slog.LevelDebug) {
		t.Fatal("debug must stay disabled")
	}
}
