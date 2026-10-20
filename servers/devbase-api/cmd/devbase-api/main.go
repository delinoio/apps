// SPDX-License-Identifier: FSL-1.1-ALv2

// Command devbase-api serves the DevBase end-user directory API.
//
//	devbase-api            serve the API
//	devbase-api migrate    apply explicit database migrations and exit
//	devbase-api check-schema  fail unless the schema matches this binary
package main

import (
	"context"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"strings"
	"syscall"
	"time"

	"github.com/delinoio/apps/protos/delcore/v1/delcorev1connect"
	"github.com/delinoio/apps/servers/devbase-api/internal/api"
	"github.com/delinoio/apps/servers/devbase-api/internal/config"
	"github.com/delinoio/apps/servers/devbase-api/internal/delcore"
	"github.com/delinoio/apps/servers/devbase-api/internal/metrics"
	"github.com/delinoio/apps/servers/devbase-api/internal/store"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promhttp"
	"go.opentelemetry.io/contrib/instrumentation/net/http/otelhttp"
	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/exporters/otlp/otlptrace/otlptracehttp"
	"go.opentelemetry.io/otel/sdk/trace"
)

// newLogger builds the process logger. The default is colored key/value text for
// operators; NO_COLOR disables color, and LOG_FORMAT=json emits one JSON object per
// record for log ingestion.
func newLogger(format string, noColor bool, out io.Writer) (*slog.Logger, error) {
	switch format {
	case "", "text":
		return slog.New(slog.NewTextHandler(terminalWriter{writer: out, color: !noColor}, nil)), nil
	case "json":
		return slog.New(slog.NewJSONHandler(out, nil)), nil
	}
	return nil, fmt.Errorf("LOG_FORMAT must be text or json")
}

func main() {
	logger, e := newLogger(os.Getenv("LOG_FORMAT"), os.Getenv("NO_COLOR") != "", os.Stderr)
	if e != nil {
		fmt.Fprintln(os.Stderr, e)
		os.Exit(1)
	}
	slog.SetDefault(logger)
	if e := run(os.Args[1:]); e != nil {
		slog.Error("devbase-api stopped", "error", e.Error())
		os.Exit(1)
	}
}

func run(args []string) error {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	command := "serve"
	if len(args) > 0 {
		command = args[0]
	}
	switch command {
	case "migrate", "check-schema":
		cfg, e := config.Database(os.Getenv)
		if e != nil {
			return e
		}
		db, e := store.Open(ctx, cfg.DatabaseURL)
		if e != nil {
			return e
		}
		defer db.Close()
		if command == "migrate" {
			if e = db.Migrate(ctx); e == nil {
				slog.Info("migrations complete")
			}
			return e
		}
		if e = db.CheckSchema(ctx); e == nil {
			slog.Info("schema is compatible")
		}
		return e
	case "serve":
		return serve(ctx)
	}
	return fmt.Errorf("unknown command %q", command)
}

func serve(ctx context.Context) error {
	cfg, e := config.Load(os.Getenv)
	if e != nil {
		return e
	}
	db, e := store.Open(ctx, cfg.DatabaseURL)
	if e != nil {
		return e
	}
	defer db.Close()
	if e = db.CheckSchema(ctx); e != nil {
		return e
	}
	lock, e := db.AcquireInstance(ctx)
	if e != nil {
		return e
	}
	defer func() { lock.Conn().Close(context.Background()); lock.Release() }()

	if os.Getenv("OTEL_EXPORTER_OTLP_ENDPOINT") != "" || os.Getenv("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT") != "" {
		exporter, e := otlptracehttp.New(ctx)
		if e != nil {
			return e
		}
		tracer := trace.NewTracerProvider(trace.WithBatcher(exporter))
		otel.SetTracerProvider(tracer)
		defer tracer.Shutdown(context.Background())
	}

	registry := prometheus.NewRegistry()
	m := metrics.New(registry)
	client := &http.Client{Timeout: cfg.DependencyTimeout}
	credentials := &delcore.ClientCredentials{HTTP: client, URL: cfg.TokenURL, Resource: cfg.DelcoreResource, ID: cfg.ClientID, Secret: cfg.ClientSecret, Scope: "authorize"}
	authority := &delcore.Client{RPC: delcorev1connect.NewDelcoreServiceClient(client, cfg.DelcoreURL), Token: credentials.Token, Timeout: cfg.DependencyTimeout}
	server := api.New(db.Pool, authority, cfg.Limits, slog.Default(), m, nil)

	mux := http.NewServeMux()
	path, handler := server.Handler()
	mux.Handle(path, api.Deadline(cfg.RequestTimeout, api.CORS(cfg.WebOrigin, otelhttp.NewHandler(handler, "devbase.connect"))))
	mux.Handle("/metrics", promhttp.HandlerFor(registry, promhttp.HandlerOpts{}))
	mux.HandleFunc("/healthz", func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(http.StatusOK) })
	mux.HandleFunc("/readyz", func(w http.ResponseWriter, r *http.Request) {
		check, cancel := context.WithTimeout(r.Context(), time.Second)
		defer cancel()
		if db.CheckSchema(check) != nil {
			w.WriteHeader(http.StatusServiceUnavailable)
			return
		}
		w.WriteHeader(http.StatusOK)
	})
	httpServer := &http.Server{
		Addr: cfg.Listen, Handler: mux,
		ReadHeaderTimeout: 5 * time.Second, ReadTimeout: cfg.RequestTimeout, WriteTimeout: cfg.RequestTimeout + 5*time.Second,
		IdleTimeout: time.Minute, MaxHeaderBytes: 16 << 10,
	}
	failed := make(chan error, 2)
	go func() { failed <- httpServer.ListenAndServe() }()
	// A lost session lock means another instance could start; stop serving.
	go func() {
		ticker := time.NewTicker(5 * time.Second)
		defer ticker.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				ping, cancel := context.WithTimeout(ctx, time.Second)
				e := lock.Conn().Ping(ping)
				cancel()
				if e != nil {
					failed <- errors.New("instance lock session lost")
					return
				}
			}
		}
	}()
	slog.Info("devbase-api listening", "address", cfg.Listen)
	var failure error
	select {
	case <-ctx.Done():
	case failure = <-failed:
	}
	shutdown, cancel := context.WithTimeout(context.Background(), cfg.RequestTimeout)
	defer cancel()
	if e = httpServer.Shutdown(shutdown); e != nil {
		return e
	}
	if errors.Is(failure, http.ErrServerClosed) {
		return nil
	}
	return failure
}

type terminalWriter struct {
	writer io.Writer
	color  bool
}

func (w terminalWriter) Write(p []byte) (int, error) {
	if !w.color {
		return w.writer.Write(p)
	}
	color := "\x1b[36m"
	if strings.Contains(string(p), "level=ERROR") {
		color = "\x1b[31m"
	} else if strings.Contains(string(p), "level=WARN") {
		color = "\x1b[33m"
	}
	if _, e := fmt.Fprintf(w.writer, "%s%s\x1b[0m", color, p); e != nil {
		return 0, e
	}
	return len(p), nil
}
