// SPDX-License-Identifier: FSL-1.1-ALv2

// Package server builds the delino-accounts-api HTTP handler.
package server

import (
	"encoding/json"
	"log/slog"
	"net/http"
	"time"
)

// NewHandler returns the root HTTP handler with request logging applied.
func NewHandler(logger *slog.Logger) http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /healthz", handleHealthz(logger))
	return logRequests(logger, mux)
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
type statusRecorder struct {
	http.ResponseWriter
	status int
	bytes  int
}

func (r *statusRecorder) WriteHeader(status int) {
	r.status = status
	r.ResponseWriter.WriteHeader(status)
}

func (r *statusRecorder) Write(body []byte) (int, error) {
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
		next.ServeHTTP(recorder, r)
		logger.InfoContext(r.Context(), "http_request",
			slog.String("method", r.Method),
			slog.String("path", r.URL.Path),
			slog.Int("status", recorder.status),
			slog.Int64("duration_ms", time.Since(start).Milliseconds()),
			slog.Int("bytes", recorder.bytes),
		)
	})
}
