// SPDX-License-Identifier: FSL-1.1-ALv2

// Package server builds the accounts-api HTTP handler.
package server

import (
	"encoding/json"
	"io"
	"log/slog"
	"net/http"
	"time"
)

// maxOptionsBodyBytes bounds the body read for a server-wide "OPTIONS *"
// request, matching net/http's built-in handler.
const maxOptionsBodyBytes = 4 << 10

// NewHandler returns the root HTTP handler with request logging applied.
//
// The handler answers a server-wide "OPTIONS *" request itself, inside the
// request logger. net/http answers that request before it consults the
// configured Handler unless http.Server.DisableGeneralOptionsHandler is set, so
// the production server must set it (see newServer in cmd/accounts-api);
// otherwise the request is neither routed here nor logged.
func NewHandler(logger *slog.Logger) http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /healthz", handleHealthz(logger))
	return logRequests(logger, withGeneralOptions(mux))
}

// withGeneralOptions answers "OPTIONS *" and passes every other request to
// next. It reproduces net/http's built-in handler: 200 with Content-Length 0,
// reading at most maxOptionsBodyBytes of request body.
func withGeneralOptions(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodOptions || r.RequestURI != "*" {
			next.ServeHTTP(w, r)
			return
		}
		w.Header().Set("Content-Length", "0")
		if r.ContentLength != 0 {
			// http.MaxBytesReader tells the server to close the connection
			// after an oversized body through an unexported method of the
			// ResponseWriter that net/http created. Wrappers such as
			// statusRecorder only embed the http.ResponseWriter interface and
			// do not promote that method, so the reader must be given the
			// unwrapped writer or the connection would be kept alive.
			body := http.MaxBytesReader(unwrapResponseWriter(w), r.Body, maxOptionsBodyBytes)
			_, _ = io.Copy(io.Discard, body)
		}
	})
}

// unwrapResponseWriter returns the innermost writer reachable through Unwrap
// methods, which is the one net/http created for the request.
func unwrapResponseWriter(w http.ResponseWriter) http.ResponseWriter {
	for {
		u, ok := w.(interface{ Unwrap() http.ResponseWriter })
		if !ok {
			return w
		}
		w = u.Unwrap()
	}
}

type healthResponse struct {
	Status string `json:"status"`
}

func handleHealthz(logger *slog.Logger) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Header().Set("Cache-Control", "no-store")
		if err := json.NewEncoder(w).Encode(healthResponse{Status: "ok"}); err != nil {
			logger.WarnContext(r.Context(), "health_response_write_failed", slog.Any("error", err))
		}
	}
}

// statusRecorder captures the status code and body size for request logs.
// status is the first committed status, matching what net/http sends to the
// client; wroteHeader reports whether a header has been committed, either by
// the first WriteHeader call or by the first implicit Write.
type statusRecorder struct {
	http.ResponseWriter
	status      int
	bytes       int
	wroteHeader bool
}

func (r *statusRecorder) WriteHeader(status int) {
	// net/http sends informational 1xx headers (except 101 Switching Protocols)
	// without committing the final status, so they must not be recorded.
	if !r.wroteHeader && (status < http.StatusContinue || status >= http.StatusOK || status == http.StatusSwitchingProtocols) {
		r.status = status
		r.wroteHeader = true
	}
	r.ResponseWriter.WriteHeader(status)
}

func (r *statusRecorder) Write(body []byte) (int, error) {
	// An implicit WriteHeader(200) commits the default status.
	r.wroteHeader = true
	n, err := r.ResponseWriter.Write(body)
	r.bytes += n
	return n, err
}

// Unwrap lets http.ResponseController reach the underlying writer.
func (r *statusRecorder) Unwrap() http.ResponseWriter {
	return r.ResponseWriter
}

func logRequests(logger *slog.Logger, next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		start := time.Now()
		recorder := &statusRecorder{ResponseWriter: w, status: http.StatusOK}
		// The record is written from a defer so a panicking handler is logged
		// too. The panic is deliberately not recovered here: net/http keeps its
		// connection-level recovery (including http.ErrAbortHandler handling)
		// and the client sees the aborted connection.
		returned := false
		defer func() {
			status := recorder.status
			if !returned && !recorder.wroteHeader {
				// The handler did not return and nothing was committed, so
				// net/http will not send a response; record it as a failure.
				status = http.StatusInternalServerError
			}
			logger.InfoContext(r.Context(), "http_request",
				slog.String("method", r.Method),
				slog.String("path", r.URL.Path),
				slog.Int("status", status),
				slog.Int64("duration_ms", time.Since(start).Milliseconds()),
				slog.Int("bytes", recorder.bytes),
			)
		}()
		next.ServeHTTP(recorder, r)
		returned = true
	})
}
