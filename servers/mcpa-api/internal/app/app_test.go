// SPDX-License-Identifier: FSL-1.1-ALv2

package app

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

func listen(t *testing.T) net.Listener {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	return ln
}

func quietApp() *App {
	return &App{Log: slog.New(slog.NewTextHandler(io.Discard, nil))}
}

func TestServeStopsOnContextAndDrainsWorkers(t *testing.T) {
	a := quietApp()
	stopped := make(chan struct{})
	a.AddWorker(Worker{Name: "loop", Run: func(ctx context.Context) error {
		<-ctx.Done()
		close(stopped)
		return ctx.Err()
	}})
	ln := listen(t)
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { done <- a.serve(ctx, ln, http.NotFoundHandler()) }()

	resp, err := http.Get("http://" + ln.Addr().String() + "/")
	if err != nil {
		t.Fatal(err)
	}
	resp.Body.Close()
	if resp.StatusCode != http.StatusNotFound {
		t.Fatalf("status = %d", resp.StatusCode)
	}

	cancel()
	select {
	case err := <-done:
		if err != nil {
			t.Fatalf("serve returned %v after cancellation", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("serve did not return")
	}
	select {
	case <-stopped:
	default:
		t.Fatal("worker was not drained before serve returned")
	}
}

func TestServeReturnsWorkerFailure(t *testing.T) {
	a := quietApp()
	boom := errors.New("boom")
	a.AddWorker(
		Worker{Name: "failing", Run: func(context.Context) error { return boom }},
		Worker{Name: "loop", Run: func(ctx context.Context) error { <-ctx.Done(); return nil }},
	)
	done := make(chan error, 1)
	go func() { done <- a.serve(context.Background(), listen(t), http.NotFoundHandler()) }()
	select {
	case err := <-done:
		if !errors.Is(err, boom) {
			t.Fatalf("serve returned %v, want the worker error", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("a failing worker did not stop serve")
	}
}

func TestServeReturnsListenerFailure(t *testing.T) {
	a := quietApp()
	ln := listen(t)
	ln.Close()
	done := make(chan error, 1)
	go func() { done <- a.serve(context.Background(), ln, http.NotFoundHandler()) }()
	select {
	case err := <-done:
		if err == nil {
			t.Fatal("serve returned nil for a closed listener")
		}
	case <-time.After(5 * time.Second):
		t.Fatal("a failing listener did not stop serve")
	}
}
