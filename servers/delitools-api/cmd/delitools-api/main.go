// SPDX-License-Identifier: FSL-1.1-ALv2
package main

import (
	"context"
	"errors"
	"flag"
	"github.com/coreos/go-oidc/v3/oidc"
	"github.com/delinoio/apps/protos/delcore/v1/delcorev1connect"
	"github.com/delinoio/apps/servers/delitools-api/internal/api"
	"github.com/delinoio/apps/servers/delitools-api/internal/config"
	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/exporters/otlp/otlptrace/otlptracehttp"
	"go.opentelemetry.io/otel/sdk/trace"
	"golang.org/x/oauth2"
	"golang.org/x/oauth2/clientcredentials"
	"io"
	"log/slog"
	"net"
	"net/http"
	"net/url"
	"os"
	"os/signal"
	"syscall"
	"time"
)

type colorWriter struct{ out io.Writer }

func (w colorWriter) Write(b []byte) (int, error) {
	if _, e := w.out.Write(append(append([]byte("\x1b[36m"), b...), []byte("\x1b[0m")...)); e != nil {
		return 0, e
	}
	return len(b), nil
}
func main() {
	if e := run(); e != nil {
		slog.Error("DeliTools stopped", "category", "startup_or_runtime_failure")
		os.Exit(1)
	}
}
func run() error {
	path := flag.String("config", "", "operator-owned JSON configuration")
	migrate := flag.Bool("migrate", false, "apply forward migrations and exit")
	jsonLogs := flag.Bool("log-json", false, "emit uncolored structured JSON")
	flag.Parse()
	var output io.Writer = os.Stderr
	if os.Getenv("NO_COLOR") == "" {
		output = colorWriter{os.Stderr}
	}
	var handler slog.Handler = slog.NewTextHandler(output, nil)
	if *jsonLogs {
		handler = slog.NewJSONHandler(os.Stderr, nil)
	}
	logger := slog.New(handler)
	slog.SetDefault(logger)
	c, e := config.Load(*path, os.Getenv, *migrate)
	if e != nil {
		return e
	}
	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()
	timeout := time.Duration(c.HTTPTimeoutSeconds) * time.Second
	if timeout == 0 {
		timeout = 10 * time.Second
	}
	startup, done := context.WithTimeout(ctx, timeout)
	db, e := api.Open(startup, c.DatabaseURL)
	done()
	if e != nil {
		return e
	}
	defer db.Pool.Close()
	if *migrate {
		return db.Migrate(ctx)
	}
	if e = db.CheckSchema(ctx); e != nil {
		return e
	}
	lock, e := db.AcquireInstance(ctx)
	if e != nil {
		return e
	}
	defer func() { lock.Conn().Close(context.Background()); lock.Release() }()
	if e = db.BootstrapSettings(ctx, c.Settings); e != nil {
		return e
	}
	if _, e = db.Settings(ctx); e != nil {
		return e
	}
	if e = db.Recover(ctx); e != nil {
		return e
	}
	client := &http.Client{Timeout: timeout}
	oidcCtx := oidc.ClientContext(ctx, client)
	provider, e := oidc.NewProvider(oidcCtx, c.Issuer)
	if e != nil {
		return e
	}
	tokenCtx := context.WithValue(ctx, oauth2.HTTPClient, client)
	credentials := clientcredentials.Config{ClientID: c.ClientID, ClientSecret: c.ClientSecret, TokenURL: c.TokenURL, Scopes: []string{"authorize", "usage:write"}, EndpointParams: url.Values{"resource": {c.Audience}}}
	auth := &api.Delcore{Client: delcorev1connect.NewDelcoreServiceClient(client, c.DelcoreURL), Tokens: credentials.TokenSource(tokenCtx), OperatorVerifier: provider.Verifier(&oidc.Config{ClientID: c.Audience, SupportedSigningAlgs: []string{"RS256", "ES256"}}), OperatorRole: c.OperatorRole}
	tp := trace.NewTracerProvider()
	if os.Getenv("OTEL_EXPORTER_OTLP_ENDPOINT") != "" || os.Getenv("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT") != "" {
		exporter, e := otlptracehttp.New(ctx)
		if e != nil {
			return e
		}
		tp = trace.NewTracerProvider(trace.WithBatcher(exporter))
	}
	otel.SetTracerProvider(tp)
	defer func() {
		shutdown, stop := context.WithTimeout(context.Background(), timeout)
		defer stop()
		_ = tp.Shutdown(shutdown)
	}()
	// The request context owns inference duration; a static http.Client timeout
	// must not override operator changes to the execution timeout.
	gatewayClient := &http.Client{Transport: &http.Transport{ResponseHeaderTimeout: timeout}}
	s := &api.Server{DB: db, Auth: auth, Gateway: &api.HTTPGateway{Client: gatewayClient, BaseURL: c.GatewayURL, Token: c.GatewayKey, MaxResponseBytes: c.MaxGatewayResponseBytes}, ServiceID: c.ServiceID, Log: logger}
	mux := http.NewServeMux()
	rpcPath, _ := s.Handler(int(c.Settings.MaxInputBytes))
	mux.Handle(rpcPath, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		settings, e := db.Settings(r.Context())
		if e != nil {
			http.Error(w, "service unavailable", http.StatusServiceUnavailable)
			return
		}
		_, h := s.Handler(int(settings.MaxInputBytes))
		h.ServeHTTP(w, r)
	}))
	mux.Handle("/metrics", s.Metrics())
	mux.HandleFunc("/healthz", func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(http.StatusOK) })
	mux.HandleFunc("/readyz", func(w http.ResponseWriter, r *http.Request) {
		check, stop := context.WithTimeout(r.Context(), timeout)
		defer stop()
		if ctx.Err() != nil || db.CheckSchema(check) != nil {
			w.WriteHeader(http.StatusServiceUnavailable)
			return
		}
		w.WriteHeader(http.StatusOK)
	})
	server := &http.Server{Addr: c.Listen, Handler: mux, ReadHeaderTimeout: timeout, ReadTimeout: timeout, IdleTimeout: timeout, BaseContext: func(net.Listener) context.Context { return ctx }}
	errorsCh := make(chan error, 2)
	workerDone := make(chan struct{})
	go func() { defer close(workerDone); errorsCh <- s.Background(ctx) }()
	go func() { errorsCh <- server.ListenAndServe() }()
	logger.Info("DeliTools started")
	ticker := time.NewTicker(timeout)
	defer ticker.Stop()
	var failure error
loop:
	for {
		select {
		case <-ctx.Done():
			break loop
		case failure = <-errorsCh:
			break loop
		case <-ticker.C:
			check, stop := context.WithTimeout(ctx, timeout)
			e = lock.Conn().Ping(check)
			stop()
			if e != nil {
				failure = e
				break loop
			}
		}
	}
	cancel()
	shutdown, stop := context.WithTimeout(context.Background(), timeout)
	defer stop()
	_ = server.Shutdown(shutdown)
	_ = server.Close()
	<-workerDone
	if errors.Is(failure, http.ErrServerClosed) || errors.Is(failure, context.Canceled) {
		return nil
	}
	return failure
}
