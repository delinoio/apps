// SPDX-License-Identifier: FSL-1.1-ALv2

// Command accounts-api serves the Delino account and billing API.
package main

import (
	"context"
	"errors"
	"log/slog"
	"net"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/delinoio/mono/servers/accounts-api/internal/config"
	"github.com/delinoio/mono/servers/accounts-api/internal/server"
)

const shutdownTimeout = 10 * time.Second

func main() {
	// Keep startup failures in the same JSON format as the server logs.
	slog.SetDefault(slog.New(slog.NewJSONHandler(os.Stdout, nil)))
	if err := run(); err != nil {
		slog.Error("server_exited", slog.Any("error", err))
		os.Exit(1)
	}
}

func run() error {
	cfg, err := config.Load(os.LookupEnv)
	if err != nil {
		return err
	}

	logger := slog.New(slog.NewJSONHandler(os.Stdout, &slog.HandlerOptions{Level: cfg.LogLevel.SlogLevel()}))
	slog.SetDefault(logger)

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	listener, err := net.Listen("tcp", cfg.Addr)
	if err != nil {
		return err
	}

	srv := &http.Server{
		Handler:           server.NewHandler(logger),
		ReadHeaderTimeout: 10 * time.Second,
		BaseContext:       func(net.Listener) context.Context { return ctx },
	}

	logger.Info("server_started", slog.String("addr", listener.Addr().String()), slog.String("log_level", string(cfg.LogLevel)))
	return serve(ctx, srv, listener, logger, shutdownTimeout)
}

// serve runs srv on listener until Serve fails or ctx is done, then shuts the
// server down within timeout. It takes the shutdown trigger as a context so
// tests can drive it without OS signals.
func serve(ctx context.Context, srv *http.Server, listener net.Listener, logger *slog.Logger, timeout time.Duration) error {
	serveErr := make(chan error, 1)
	go func() {
		serveErr <- srv.Serve(listener)
	}()
	return awaitServe(ctx, srv, serveErr, logger, timeout)
}

// awaitServe waits for the Serve result on serveErr or for ctx to be done. It
// takes the result channel so tests can inject a Serve outcome deterministically.
func awaitServe(ctx context.Context, srv *http.Server, serveErr <-chan error, logger *slog.Logger, timeout time.Duration) error {
	select {
	case err := <-serveErr:
		if errors.Is(err, http.ErrServerClosed) {
			return nil
		}
		return err
	case <-ctx.Done():
	}

	logger.Info("server_shutdown_started")
	shutdownCtx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	shutdownErr := srv.Shutdown(shutdownCtx)
	if shutdownErr != nil {
		logger.Error("server_shutdown_failed", slog.Any("error", shutdownErr))
	}

	// ctx can win the select above while Serve has already failed, which would
	// otherwise discard the serve error. Shutdown makes Serve return, so this
	// receive does not block.
	var drainedErr error
	if err := <-serveErr; !errors.Is(err, http.ErrServerClosed) {
		drainedErr = err
	}
	if err := errors.Join(shutdownErr, drainedErr); err != nil {
		return err
	}
	logger.Info("server_shutdown_completed")
	return nil
}
