// SPDX-License-Identifier: FSL-1.1-ALv2

// Command delino-accounts-api serves the Delino account and billing API.
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

	"github.com/delinoio/mono/servers/delino-accounts-api/internal/config"
	"github.com/delinoio/mono/servers/delino-accounts-api/internal/server"
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

	serveErr := make(chan error, 1)
	go func() {
		logger.Info("server_started", slog.String("addr", listener.Addr().String()), slog.String("log_level", string(cfg.LogLevel)))
		serveErr <- srv.Serve(listener)
	}()

	select {
	case err := <-serveErr:
		if errors.Is(err, http.ErrServerClosed) {
			return nil
		}
		return err
	case <-ctx.Done():
	}

	logger.Info("server_shutdown_started")
	shutdownCtx, cancel := context.WithTimeout(context.Background(), shutdownTimeout)
	defer cancel()
	if err := srv.Shutdown(shutdownCtx); err != nil {
		logger.Error("server_shutdown_failed", slog.Any("error", err))
		return err
	}
	logger.Info("server_shutdown_completed")
	return nil
}
