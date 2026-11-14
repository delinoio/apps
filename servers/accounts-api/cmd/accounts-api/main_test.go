// SPDX-License-Identifier: FSL-1.1-ALv2

package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

// failingListener fails every Accept call, which makes http.Server.Serve return
// the error immediately.
type failingListener struct {
	net.Listener
	err error
}

func (l failingListener) Accept() (net.Conn, error) { return nil, l.err }

func newTestListener(t *testing.T) net.Listener {
	t.Helper()
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	t.Cleanup(func() { _ = listener.Close() })
	return listener
}

func newTestLogger() *slog.Logger {
	return slog.New(slog.NewJSONHandler(io.Discard, nil))
}

func TestAwaitServeReturnsServeErrorWhenContextIsAlreadyCancelled(t *testing.T) {
	acceptErr := errors.New("accept failed")
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	// Both select cases are ready; the serve error must not depend on which wins.
	for range 100 {
		serveErr := make(chan error, 1)
		serveErr <- acceptErr
		err := awaitServe(ctx, &http.Server{}, serveErr, newTestLogger(), time.Second)
		if !errors.Is(err, acceptErr) {
			t.Fatalf("awaitServe error = %v, want %v", err, acceptErr)
		}
	}
}

func TestAwaitServeIgnoresErrServerClosed(t *testing.T) {
	serveErr := make(chan error, 1)
	serveErr <- http.ErrServerClosed

	if err := awaitServe(context.Background(), &http.Server{}, serveErr, newTestLogger(), time.Second); err != nil {
		t.Fatalf("awaitServe error = %v, want nil", err)
	}
}

func TestAwaitServeIgnoresErrServerClosedAfterShutdown(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	serveErr := make(chan error, 1)
	serveErr <- http.ErrServerClosed

	if err := awaitServe(ctx, &http.Server{}, serveErr, newTestLogger(), time.Second); err != nil {
		t.Fatalf("awaitServe error = %v, want nil", err)
	}
}

func TestServeReturnsServeErrorBeforeShutdown(t *testing.T) {
	acceptErr := errors.New("accept failed")

	err := serve(context.Background(), &http.Server{}, failingListener{Listener: newTestListener(t), err: acceptErr}, newTestLogger(), time.Second)
	if !errors.Is(err, acceptErr) {
		t.Fatalf("serve error = %v, want %v", err, acceptErr)
	}
}

func TestServeIgnoresErrServerClosed(t *testing.T) {
	srv := &http.Server{}
	// Closing before Serve makes Serve return http.ErrServerClosed.
	if err := srv.Close(); err != nil {
		t.Fatalf("close: %v", err)
	}

	if err := serve(context.Background(), srv, newTestListener(t), newTestLogger(), time.Second); err != nil {
		t.Fatalf("serve error = %v, want nil", err)
	}
}

func TestServeCleanShutdownReturnsNil(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	done := make(chan error, 1)
	go func() {
		done <- serve(ctx, &http.Server{}, newTestListener(t), newTestLogger(), time.Second)
	}()

	cancel()
	select {
	case err := <-done:
		if err != nil {
			t.Fatalf("serve error = %v, want nil", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("serve did not return after the context was cancelled")
	}
}

// startServe runs serve on a real listener and returns the listener address and
// a channel that receives serve's result.
func startServe(ctx context.Context, t *testing.T, handler http.Handler, timeout time.Duration) (string, <-chan error) {
	t.Helper()
	listener := newTestListener(t)
	done := make(chan error, 1)
	go func() {
		done <- serve(ctx, &http.Server{Handler: handler}, listener, newTestLogger(), timeout)
	}()
	return listener.Addr().String(), done
}

func TestServeDrainsInFlightRequestWithoutCancellingItsContext(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	started := make(chan struct{})
	release := make(chan struct{})
	handlerCtx := make(chan context.Context, 1)
	handlerCtxErr := make(chan error, 1)
	addr, done := startServe(ctx, t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		handlerCtx <- r.Context()
		close(started)
		<-release
		handlerCtxErr <- r.Context().Err()
		_, _ = io.WriteString(w, "completed")
	}), 5*time.Second)

	type response struct {
		status int
		body   string
		err    error
	}
	responded := make(chan response, 1)
	go func() {
		resp, err := http.Get("http://" + addr + "/")
		if err != nil {
			responded <- response{err: err}
			return
		}
		defer resp.Body.Close()
		body, err := io.ReadAll(resp.Body)
		responded <- response{status: resp.StatusCode, body: string(body), err: err}
	}()

	<-started
	cancel() // the shutdown trigger

	// Shutdown must wait for the in-flight request instead of returning.
	select {
	case err := <-done:
		t.Fatalf("serve returned %v while a request was in flight", err)
	case <-time.After(200 * time.Millisecond):
	}

	close(release)
	if err := <-handlerCtxErr; err != nil {
		t.Fatalf("request context error during drain = %v, want nil", err)
	}
	if got := <-responded; got.err != nil || got.status != http.StatusOK || got.body != "completed" {
		t.Fatalf("response = %+v, want 200 completed", got)
	}
	select {
	case err := <-done:
		if err != nil {
			t.Fatalf("serve error = %v, want nil", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("serve did not return after the request completed")
	}

	// The server-lifetime context ends with the drain.
	if err := (<-handlerCtx).Err(); err == nil {
		t.Fatal("request context was not cancelled after serve returned")
	}
}

func TestServeCancelsRequestContextOnlyAfterShutdownDeadline(t *testing.T) {
	const timeout = 200 * time.Millisecond
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	started := make(chan struct{})
	cancelledAt := make(chan time.Time, 1)
	addr, done := startServe(ctx, t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		close(started)
		<-r.Context().Done()
		cancelledAt <- time.Now()
	}), timeout)

	go func() {
		resp, err := http.Get("http://" + addr + "/")
		if err == nil {
			_ = resp.Body.Close()
		}
	}()

	<-started
	shutdownStartedAt := time.Now()
	cancel()

	select {
	case err := <-done:
		if !errors.Is(err, context.DeadlineExceeded) {
			t.Fatalf("serve error = %v, want %v", err, context.DeadlineExceeded)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("serve did not return after the shutdown deadline")
	}
	select {
	case at := <-cancelledAt:
		if elapsed := at.Sub(shutdownStartedAt); elapsed < timeout {
			t.Fatalf("request context cancelled %v after shutdown started, want at least %v", elapsed, timeout)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("request context was not cancelled after the shutdown deadline")
	}
}

// serveThroughNewServer sends one GET to handler on a real HTTP server built by
// newServer, with the logger writing JSON to a buffer. It returns the client
// error (nil when a response arrived), the client-observed status (0 on error)
// and the decoded log records. Close waits for in-flight handlers, so every
// record is complete when it returns.
func serveThroughNewServer(t *testing.T, handler http.HandlerFunc) (clientErr error, clientStatus int, records []map[string]any) {
	t.Helper()
	var logs bytes.Buffer
	logger := slog.New(slog.NewJSONHandler(&logs, nil))
	ts := httptest.NewUnstartedServer(nil)
	ts.Config = newServer(handler, logger)
	ts.Start()
	defer ts.Close()

	resp, err := http.Get(ts.URL + "/probe")
	if err == nil {
		clientStatus = resp.StatusCode
		resp.Body.Close()
	}
	ts.Close()

	for _, line := range strings.Split(strings.TrimSpace(logs.String()), "\n") {
		if line == "" {
			continue
		}
		var record map[string]any
		if err := json.Unmarshal([]byte(line), &record); err != nil {
			t.Fatalf("log line is not JSON: %q: %v", line, err)
		}
		records = append(records, record)
	}
	return err, clientStatus, records
}

func TestNewServerSetsErrorLog(t *testing.T) {
	srv := newServer(http.NotFoundHandler(), newTestLogger())
	if srv.ErrorLog == nil {
		t.Fatal("ErrorLog is nil; net/http would write plain text to stderr")
	}
	if srv.ReadHeaderTimeout != 10*time.Second {
		t.Errorf("ReadHeaderTimeout = %v, want 10s", srv.ReadHeaderTimeout)
	}
}

func TestNewServerLogsHandlerPanicAsStructuredError(t *testing.T) {
	// The panic value must not be http.ErrAbortHandler, which net/http does not log.
	clientErr, _, records := serveThroughNewServer(t, func(http.ResponseWriter, *http.Request) {
		panic("boom")
	})
	if clientErr == nil {
		t.Fatal("client got a response, want the aborted connection")
	}
	if len(records) != 1 {
		t.Fatalf("got %d log records, want 1: %v", len(records), records)
	}
	record := records[0]
	if record["level"] != "ERROR" {
		t.Errorf("level = %v, want ERROR", record["level"])
	}
	msg, _ := record["msg"].(string)
	if !strings.Contains(msg, "http: panic serving") || !strings.Contains(msg, "boom") {
		t.Errorf("msg = %q, want the net/http panic report for boom", msg)
	}
}

func TestNewServerLogsSuperfluousWriteHeaderAsStructuredError(t *testing.T) {
	clientErr, status, records := serveThroughNewServer(t, func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusOK)
		w.WriteHeader(http.StatusTeapot)
	})
	if clientErr != nil {
		t.Fatalf("client error = %v, want a response", clientErr)
	}
	if status != http.StatusOK {
		t.Errorf("client status = %d, want the first committed status 200", status)
	}
	if len(records) != 1 {
		t.Fatalf("got %d log records, want 1: %v", len(records), records)
	}
	if records[0]["level"] != "ERROR" {
		t.Errorf("level = %v, want ERROR", records[0]["level"])
	}
	msg, _ := records[0]["msg"].(string)
	if !strings.Contains(msg, "superfluous response.WriteHeader") {
		t.Errorf("msg = %q, want the superfluous WriteHeader report", msg)
	}
}

func TestNewServerKeepsErrAbortHandlerSilent(t *testing.T) {
	clientErr, _, records := serveThroughNewServer(t, func(http.ResponseWriter, *http.Request) {
		panic(http.ErrAbortHandler)
	})
	if clientErr == nil {
		t.Fatal("client got a response, want the aborted connection")
	}
	if len(records) != 0 {
		t.Errorf("got log records %v, want none for http.ErrAbortHandler", records)
	}
}
