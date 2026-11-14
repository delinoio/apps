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

// HTTP server timeouts. They are fixed constants rather than configuration.
//
// readHeaderTimeout bounds reading the request headers. net/http clears that
// deadline once the headers are read, so without readTimeout a client could
// declare a body and then stall forever. readTimeout bounds the whole request
// read (headers and body) and must not be below readHeaderTimeout. writeTimeout
// bounds writing the response. idleTimeout bounds a keep-alive connection that
// is waiting for its next request; without it the connection stays open until
// the client closes it.
//
// writeTimeout also caps the lifetime of any single response, so it must be
// revisited (or lifted per handler through http.ResponseController) before a
// long-lived streaming RPC is added.
const (
	readHeaderTimeout = 10 * time.Second
	readTimeout       = 30 * time.Second
	writeTimeout      = 30 * time.Second
	idleTimeout       = 120 * time.Second
)

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

	ctx, stop := shutdownContext()
	defer stop()

	listener, err := net.Listen("tcp", cfg.Addr)
	if err != nil {
		return err
	}

	srv := newServer(server.NewHandler(logger), logger)

	logger.Info("server_started", slog.String("addr", listener.Addr().String()), slog.String("log_level", string(cfg.LogLevel)))
	return serve(ctx, stop, srv, listener, logger, shutdownTimeout)
}

// shutdownContext returns a context that is cancelled by the first SIGINT or
// SIGTERM, and the stop function that releases the signal registration.
//
// The registration stays installed until stop runs. While it is installed a
// repeated signal is swallowed, so serve calls stop as soon as it observes the
// first signal; a second signal during the drain then reaches the default
// disposition and terminates the process instead of waiting for the drain.
func shutdownContext() (context.Context, context.CancelFunc) {
	return signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
}

// newServer is the single production construction point of the http.Server.
//
// net/http reports its own internal errors (a recovered handler panic with its
// stack trace, a superfluous WriteHeader call, TLS handshake and malformed
// request errors) through Server.ErrorLog. When ErrorLog is nil it falls back
// to the standard library's default logger. After slog.SetDefault that logger
// forwards to the slog default handler at INFO level, so these failures would
// be recorded as INFO and dropped when LOG_LEVEL is warn or error; without
// slog.SetDefault they would be plain text on stderr. Setting ErrorLog routes
// them through logger's handler as ERROR records independent of the global
// default, so every server log stays JSON on standard output.
//
// DisableGeneralOptionsHandler stops net/http from answering a server-wide
// "OPTIONS *" request itself, which would bypass Handler and the request log.
// server.NewHandler answers and logs that request instead; a Handler passed to
// newServer must do the same if it needs to serve "OPTIONS *".
//
// net/http rejects a request with an unsupported Expect header with 417 before
// it calls Handler, so the request logger inside Handler never sees it. serve
// wraps the listener with server.ObserveListener, which logs that rejection,
// and ConnContext here pairs each connection with its observer.
// MaxHeaderBytes is set explicitly because the observer derives its own head
// limit from the same constant.
func newServer(handler http.Handler, logger *slog.Logger) *http.Server {
	return &http.Server{
		Handler:                      handler,
		ConnContext:                  server.ConnContext,
		MaxHeaderBytes:               server.MaxHeaderBytes,
		DisableGeneralOptionsHandler: true,
		ReadHeaderTimeout:            readHeaderTimeout,
		ReadTimeout:                  readTimeout,
		WriteTimeout:                 writeTimeout,
		IdleTimeout:                  idleTimeout,
		ErrorLog:                     slog.NewLogLogger(logger.Handler(), slog.LevelError),
	}
}

// serve runs srv on listener until Serve fails or ctx is done, then shuts the
// server down within timeout. It takes the shutdown trigger as a context so
// tests can drive it without OS signals.
//
// stop releases the signal registration behind ctx. serve calls it, before it
// logs server_shutdown_started and starts the drain, when ctx is done first, so
// a second termination signal during the drain terminates the process. It is
// not called when Serve fails first: the caller's deferred stop covers that
// path. stop must be safe to call more than once.
//
// Request contexts derive from a server-lifetime context that is deliberately
// not ctx: cancelling ctx only starts the drain, so in-flight handlers that
// honour r.Context() can finish within timeout. The server-lifetime context is
// cancelled once the drain has ended, which cuts off handlers that outlived
// the timeout. serve sets srv.BaseContext and must not be called twice for the
// same srv.
func serve(ctx context.Context, stop func(), srv *http.Server, listener net.Listener, logger *slog.Logger, timeout time.Duration) error {
	baseCtx, cancelBase := context.WithCancel(context.Background())
	defer cancelBase()
	srv.BaseContext = func(net.Listener) context.Context { return baseCtx }

	// Log requests that net/http rejects before the handler runs (417 for an
	// unsupported Expect header); see newServer.
	observed := server.ObserveListener(listener, logger)
	serveErr := make(chan error, 1)
	go func() {
		serveErr <- srv.Serve(observed)
	}()
	return awaitServe(ctx, stop, srv, serveErr, logger, timeout)
}

// awaitServe waits for the Serve result on serveErr or for ctx to be done. It
// takes the result channel so tests can inject a Serve outcome deterministically.
// When ctx is done first it calls stop before anything else; see serve.
func awaitServe(ctx context.Context, stop func(), srv *http.Server, serveErr <-chan error, logger *slog.Logger, timeout time.Duration) error {
	select {
	case err := <-serveErr:
		if errors.Is(err, http.ErrServerClosed) {
			return nil
		}
		return err
	case <-ctx.Done():
	}

	// Call stop explicitly, and before the drain, rather than through
	// context.AfterFunc: AfterFunc runs asynchronously, so a second signal could
	// still arrive before the registration is released.
	stop()
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
