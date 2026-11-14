// SPDX-License-Identifier: FSL-1.1-ALv2

package main

import (
	"context"
	"errors"
	"io"
	"log/slog"
	"net"
	"net/http"
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
