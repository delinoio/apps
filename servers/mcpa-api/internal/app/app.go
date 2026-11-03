// SPDX-License-Identifier: FSL-1.1-ALv2

// Package app wires the mcpa-api components for `mcpa-api serve` and runs
// the HTTP server together with the in-process background workers.
package app

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"net"
	"net/http"
	"sync"
	"time"

	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/authz"
	"github.com/delinoio/apps/servers/mcpa-api/internal/config"
	"github.com/delinoio/apps/servers/mcpa-api/internal/crypto"
	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
	"github.com/delinoio/apps/servers/mcpa-api/internal/obs"
	"github.com/delinoio/apps/servers/mcpa-api/internal/server"
	"github.com/delinoio/apps/servers/mcpa-api/internal/service"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
	"github.com/delinoio/apps/servers/mcpa-api/internal/web"
)

// ShutdownTimeout bounds the graceful drain of in-flight requests and
// workers after the serve context ends.
const ShutdownTimeout = 30 * time.Second

// Worker is a background loop that runs until ctx ends. A worker returns
// nil on cancellation; any other error stops the process.
type Worker struct {
	Name string
	Run  func(ctx context.Context) error
}

// App holds the wired components. Later layers read them to build their
// surfaces and register mounts, hooks and workers before Run.
type App struct {
	Config        *config.Serve
	Log           *slog.Logger
	Metrics       *obs.Metrics
	Store         *store.Store
	Keys          *crypto.Keyring
	Delcore       *delcore.Client
	Logto         *auth.Logto
	Authenticator *auth.Authenticator
	Authorizer    *authz.Authorizer
	Service       *service.Service
	Web           *web.Handler

	mounts  []server.Mount
	workers []Worker
}

// New connects to PostgreSQL, refuses a schema that differs from the
// binary's migrations and builds every component. It never migrates. It does
// not contact Logto or Delcore; those are reached lazily per request.
func New(ctx context.Context, cfg *config.Serve, log *slog.Logger, metrics *obs.Metrics) (*App, error) {
	keys, err := crypto.NewKeyring(cfg.MasterKeys, cfg.ActiveKeyID)
	if err != nil {
		return nil, fmt.Errorf("master keys: %w", err)
	}
	db, err := store.Open(ctx, cfg.URL, cfg.DependencyTimeout, store.ModeServe)
	if err != nil {
		return nil, err
	}
	if err := db.CheckSchema(ctx); err != nil {
		db.Close()
		return nil, err
	}
	dc, err := delcore.New(delcore.Config{
		BaseURL:      cfg.DelcoreURL,
		TokenURL:     cfg.LogtoTokenURL,
		ClientID:     cfg.ClientID,
		ClientSecret: cfg.ClientSecret,
		Resource:     cfg.DelcoreResource,
		ServiceID:    cfg.ServiceID,
		Timeout:      cfg.DependencyTimeout,
		CacheTTL:     cfg.AuthorizeCacheTTL,
		Metrics:      metrics,
	})
	if err != nil {
		db.Close()
		return nil, err
	}
	logto := auth.NewLogto(cfg.LogtoIssuer, cfg.ConsoleClientID, cfg.ConsoleClientSecret, &http.Client{Timeout: cfg.DependencyTimeout})
	sessions := auth.NewSessions(auth.SessionsConfig{
		Store:           db,
		Keys:            keys,
		Logto:           logto,
		APIResource:     cfg.APIResource,
		DelcoreResource: cfg.DelcoreResource,
		MFAClaim:        cfg.MFAClaim,
		ReauthURL:       cfg.MFAReauthURL,
		Log:             log,
	})
	authenticator := auth.NewAuthenticator(auth.AuthenticatorConfig{
		Sessions:    sessions,
		Logto:       logto,
		Keys:        dc,
		WebOrigin:   cfg.WebOrigin,
		MCPResource: cfg.MCPResource,
		MFAClaim:    cfg.MFAClaim,
		ReauthURL:   cfg.MFAReauthURL,
	})
	// Group snapshots refresh at most once per authorization cache TTL, so
	// group changes reach call keys on the same cadence as Delcore results.
	authorizer := authz.New(db, dc, cfg.AuthorizeCacheTTL, log).WithRefreshTimeout(cfg.DependencyTimeout)
	svc := service.New(service.Config{
		Store:      db,
		Keys:       keys,
		Authorizer: authorizer,
		Delcore:    dc,
		PublicURL:  cfg.PublicURL,
		Log:        log,
	})
	webHandler := web.New(web.Config{
		Store:           db,
		Logto:           logto,
		Authenticator:   authenticator,
		Delcore:         dc,
		PublicURL:       cfg.PublicURL,
		ReturnURLs:      cfg.WebReturnURLs,
		APIResource:     cfg.APIResource,
		DelcoreResource: cfg.DelcoreResource,
		MFAClaim:        cfg.MFAClaim,
		InsecureCookies: cfg.InsecureCookies(),
		Log:             log,
	})
	return &App{
		Config:        cfg,
		Log:           log,
		Metrics:       metrics,
		Store:         db,
		Keys:          keys,
		Delcore:       dc,
		Logto:         logto,
		Authenticator: authenticator,
		Authorizer:    authorizer,
		Service:       svc,
		Web:           webHandler,
	}, nil
}

// Mount adds HTTP routes contributed by later layers. Call it before Run.
func (a *App) Mount(m ...server.Mount) { a.mounts = append(a.mounts, m...) }

// AddWorker adds a background worker. Call it before Run.
func (a *App) AddWorker(w ...Worker) { a.workers = append(a.workers, w...) }

// Handler builds the root HTTP handler from the components and mounts.
func (a *App) Handler() http.Handler {
	return server.New(server.Config{
		Service:       a.Service,
		Authenticator: a.Authenticator,
		Web:           a.Web,
		Metrics:       a.Metrics,
		Log:           a.Log,
		WebOrigin:     a.Config.WebOrigin,
		MetricsToken:  a.Config.MetricsToken,
		Ready:         a.Store.CheckSchema,
		Mounts:        a.mounts,
	})
}

// Close releases the database pool.
func (a *App) Close() { a.Store.Close() }

// Run listens on the configured address and serves until ctx ends, then
// drains requests and workers within ShutdownTimeout. A failing listener or
// worker stops the others and is returned.
func (a *App) Run(ctx context.Context) error {
	ln, err := net.Listen("tcp", a.Config.Listen)
	if err != nil {
		return fmt.Errorf("listen %s: %w", a.Config.Listen, err)
	}
	return a.Serve(ctx, ln)
}

// Serve is Run on an existing listener. Tests use it with an ephemeral port.
func (a *App) Serve(ctx context.Context, ln net.Listener) error {
	return a.serve(ctx, ln, a.Handler())
}

func (a *App) serve(ctx context.Context, ln net.Listener, handler http.Handler) error {
	runCtx, cancel := context.WithCancelCause(ctx)
	defer cancel(nil)
	// Requests get their own lifetime: ending ctx starts the graceful drain,
	// and in-flight requests keep their context until Shutdown completes or
	// its deadline expires.
	reqCtx, cancelRequests := context.WithCancel(context.WithoutCancel(ctx))
	defer cancelRequests()

	srv := &http.Server{
		Handler:           handler,
		ReadHeaderTimeout: 10 * time.Second,
		IdleTimeout:       2 * time.Minute,
		// No read or write timeout: model calls stream for as long as the
		// provider does.
		BaseContext: func(net.Listener) context.Context { return reqCtx },
		ErrorLog:    slog.NewLogLogger(a.Log.Handler(), slog.LevelWarn),
	}

	var wg sync.WaitGroup
	for _, w := range a.workers {
		wg.Go(func() {
			a.Log.Info("worker started", "worker", w.Name)
			err := w.Run(runCtx)
			if err != nil && !errors.Is(err, context.Canceled) {
				a.Log.Error("worker failed", "worker", w.Name, "error", err)
				cancel(fmt.Errorf("worker %s: %w", w.Name, err))
				return
			}
			a.Log.Info("worker stopped", "worker", w.Name)
		})
	}

	serveErr := make(chan error, 1)
	go func() { serveErr <- srv.Serve(ln) }()
	a.Log.Info("mcpa-api listening", "listen", ln.Addr().String(), "workers", len(a.workers), "mounts", len(a.mounts))

	select {
	case err := <-serveErr:
		cancel(fmt.Errorf("http server: %w", err))
	case <-runCtx.Done():
	}

	a.Log.Info("mcpa-api shutting down")
	shutdownCtx, stop := context.WithTimeout(context.WithoutCancel(ctx), ShutdownTimeout)
	defer stop()
	shutdownErr := srv.Shutdown(shutdownCtx)
	// Cancel requests that outlived the drain deadline.
	cancelRequests()
	cancel(nil)

	done := make(chan struct{})
	go func() { wg.Wait(); close(done) }()
	select {
	case <-done:
	case <-shutdownCtx.Done():
		a.Log.Warn("workers did not stop within the shutdown timeout")
	}

	if cause := context.Cause(runCtx); cause != nil && !errors.Is(cause, context.Canceled) && ctx.Err() == nil {
		return cause
	}
	if shutdownErr != nil && !errors.Is(shutdownErr, http.ErrServerClosed) {
		return fmt.Errorf("http shutdown: %w", shutdownErr)
	}
	return nil
}
