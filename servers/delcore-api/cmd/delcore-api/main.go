// SPDX-License-Identifier: FSL-1.1-ALv2

package main

import (
	"context"
	"errors"
	"flag"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"sync"
	"syscall"
	"time"

	"github.com/delinoio/apps/servers/delcore-api/internal/auth"
	"github.com/delinoio/apps/servers/delcore-api/internal/catalog"
	"github.com/delinoio/apps/servers/delcore-api/internal/config"
	"github.com/delinoio/apps/servers/delcore-api/internal/postgres"
	"github.com/delinoio/apps/servers/delcore-api/internal/provider"
	"github.com/delinoio/apps/servers/delcore-api/internal/rpc"
	"github.com/delinoio/apps/servers/delcore-api/internal/telemetry"
	"github.com/delinoio/apps/servers/delcore-api/internal/worker"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promhttp"
	"go.opentelemetry.io/contrib/instrumentation/net/http/otelhttp"
	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/exporters/otlp/otlptrace/otlptracehttp"
	"go.opentelemetry.io/otel/sdk/trace"
)

func main() {
	if e := execute(); e != nil {
		slog.Error("delcore stopped", "category", "startup_or_runtime_failure")
		os.Exit(1)
	}
}
func execute() error {
	migrate := flag.Bool("migrate", false, "apply explicit database migrations and exit")
	flag.Parse()
	logger := slog.New(slog.NewJSONHandler(os.Stderr, nil))
	slog.SetDefault(logger)
	c, e := config.Load()
	if e != nil {
		logger.Error("configuration rejected", "category", "invalid_configuration", "error", e.Error())
		return e
	}
	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()
	startup, stop := context.WithTimeout(ctx, c.Timeout())
	db, e := postgres.Open(startup, c.DatabaseURL)
	stop()
	if e != nil {
		return e
	}
	defer db.Pool.Close()
	if *migrate {
		e = db.Migrate(ctx)
		if e == nil {
			logger.Info("migrations complete")
		}
		return e
	}
	if e = db.CheckSchema(ctx); e != nil {
		return e
	}
	lock, e := db.AcquireInstance(ctx)
	if e != nil {
		return e
	}
	defer func() { lock.Conn().Close(context.Background()); lock.Release() }()
	// Tracing and metrics start before catalog application so the startup
	// reconciliation is observable.
	tp := trace.NewTracerProvider()
	if os.Getenv("OTEL_EXPORTER_OTLP_ENDPOINT") != "" || os.Getenv("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT") != "" {
		exporter, e := otlptracehttp.New(ctx)
		if e != nil {
			return e
		}
		tp = trace.NewTracerProvider(trace.WithBatcher(exporter))
	}
	otel.SetTracerProvider(tp)
	defer tp.Shutdown(context.Background())
	registry := prometheus.NewRegistry()
	metrics := telemetry.New(registry)
	if c.Development {
		// The result carries the correlation ID of the reconciliation span even
		// when the application is rejected, so the only startup failure log can be
		// matched with its trace.
		reconciled, e := catalog.Sync(ctx, db, c.CatalogDir, logger, metrics)
		if e != nil {
			logger.Error("catalog synchronization rejected", "category", "catalog_reconciliation", "correlation_id", reconciled.Correlation, "error", e.Error())
			return e
		}
	}
	verifier, e := auth.New(ctx, c)
	if e != nil {
		return e
	}
	s := &rpc.Server{DB: db, Auth: verifier, Config: c, Log: logger, Metrics: metrics}
	w := &worker.Worker{DB: db, Provider: provider.New(c), Config: c, Log: logger, Metrics: metrics}
	backlog := prometheus.NewGauge(prometheus.GaugeOpts{Name: "delcore_job_backlog", Help: "Pending or reconciliation jobs."})
	failed := prometheus.NewGauge(prometheus.GaugeOpts{Name: "delcore_failed_jobs", Help: "Jobs requiring operator attention."})
	mismatch := prometheus.NewGauge(prometheus.GaugeOpts{Name: "delcore_provider_mismatches", Help: "Provider payments requiring attribution review."})
	delivery := prometheus.NewGauge(prometheus.GaugeOpts{Name: "delcore_email_failures", Help: "Failed email delivery jobs."})
	retries := prometheus.NewGauge(prometheus.GaugeOpts{Name: "delcore_retry_exhausted", Help: "Jobs that exhausted automatic retries."})
	settlementAge := prometheus.NewGauge(prometheus.GaugeOpts{Name: "delcore_settlement_delay_seconds", Help: "Age of the oldest incomplete settlement."})
	meteringBacklog := prometheus.NewGauge(prometheus.GaugeOpts{Name: "delcore_metering_backlog", Help: "Accepted metering work awaiting aggregate materialization."})
	meteringFailed := prometheus.NewGauge(prometheus.GaugeOpts{Name: "delcore_metering_failed_jobs", Help: "Metering work requiring operator retry."})
	meteringLag := prometheus.NewGauge(prometheus.GaugeOpts{Name: "delcore_metering_lag_seconds", Help: "Age of the oldest pending metering work."})
	registry.MustRegister(backlog, failed, mismatch, delivery, retries, settlementAge, meteringBacklog, meteringFailed, meteringLag)
	mux := http.NewServeMux()
	pathRPC, handler := s.Handler()
	mux.Handle(pathRPC, otelhttp.NewHandler(handler, "delcore.connect"))
	mux.HandleFunc("/webhooks/paddle", w.Webhook)
	mux.HandleFunc("/healthz", func(rw http.ResponseWriter, r *http.Request) { rw.WriteHeader(http.StatusOK) })
	mux.HandleFunc("/readyz", func(rw http.ResponseWriter, r *http.Request) {
		ctx, cancel := context.WithTimeout(r.Context(), time.Second)
		defer cancel()
		if ctx.Err() != nil || db.CheckSchema(ctx) != nil {
			rw.WriteHeader(http.StatusServiceUnavailable)
			return
		}
		rw.WriteHeader(http.StatusOK)
	})
	mux.Handle("/metrics", promhttp.HandlerFor(registry, promhttp.HandlerOpts{}))
	// V1 uses a deployment-wide rate bound. Avoid unbounded maps keyed by user input.
	var rateMu sync.Mutex
	window := time.Now()
	count := 0
	bounded := http.HandlerFunc(func(rw http.ResponseWriter, r *http.Request) {
		rateMu.Lock()
		if time.Since(window) >= time.Minute {
			window = time.Now()
			count = 0
		}
		count++
		allowed := count <= c.RequestsPerMinute
		rateMu.Unlock()
		if !allowed {
			rw.WriteHeader(http.StatusTooManyRequests)
			return
		}
		mux.ServeHTTP(rw, r)
	})
	server := &http.Server{Addr: c.Listen, Handler: bounded, ReadHeaderTimeout: c.Timeout(), ReadTimeout: c.Timeout(), WriteTimeout: c.Timeout(), IdleTimeout: time.Minute, MaxHeaderBytes: 16 << 10}
	errorsCh := make(chan error, 2)
	workerDone := make(chan struct{})
	go func() { defer close(workerDone); errorsCh <- w.Run(ctx) }()
	go func() { errorsCh <- server.ListenAndServe() }()
	ticker := time.NewTicker(5 * time.Second)
	defer ticker.Stop()
	logger.Info("delcore started")
	var failure error
	running := true
	for running {
		select {
		case <-ctx.Done():
			running = false
		case failure = <-errorsCh:
			running = false
		case <-ticker.C:
			pingCtx, done := context.WithTimeout(ctx, time.Second)
			e = lock.Conn().Ping(pingCtx)
			done()
			if e != nil {
				failure = e
				running = false
				continue
			}
			var pending, exhausted int
			if e = db.Pool.QueryRow(ctx, "SELECT count(*) FILTER(WHERE state IN ('pending','reconcile')),count(*) FILTER(WHERE state='failed') FROM jobs").Scan(&pending, &exhausted); e == nil {
				backlog.Set(float64(pending))
				failed.Set(float64(exhausted))
				if pending >= c.BacklogAlert {
					logger.Warn("job backlog threshold exceeded", "category", "job_backlog")
				}
			}
			var mismatches, deliveries, exhaustedRetries int
			var delay float64
			metricsCtx, metricsDone := context.WithTimeout(ctx, c.Timeout())
			e = db.Pool.QueryRow(metricsCtx, `SELECT count(*) FILTER(WHERE error_category IN ('payment_mismatch','unknown_payment_outcome')),count(*) FILTER(WHERE kind='email' AND state='failed'),count(*) FILTER(WHERE error_category='retry_exhausted'),COALESCE(EXTRACT(EPOCH FROM now()-min(created_at) FILTER(WHERE kind='settlement' AND state<>'done')),0) FROM jobs`).Scan(&mismatches, &deliveries, &exhaustedRetries, &delay)
			metricsDone()
			if e == nil {
				mismatch.Set(float64(mismatches))
				delivery.Set(float64(deliveries))
				retries.Set(float64(exhaustedRetries))
				settlementAge.Set(delay)
				if delay >= float64(c.SettlementAlertSeconds) {
					logger.Warn("settlement delay threshold exceeded", "category", "settlement_delay")
				}
			}
			var meteringPending, meteringFailures int
			var lag float64
			metricsCtx, metricsDone = context.WithTimeout(ctx, c.Timeout())
			e = db.Pool.QueryRow(metricsCtx, `SELECT count(*) FILTER(WHERE state='pending'),count(*) FILTER(WHERE state='failed'),COALESCE(EXTRACT(EPOCH FROM now()-min(created_at) FILTER(WHERE state='pending')),0) FROM jobs WHERE kind='metering'`).Scan(&meteringPending, &meteringFailures, &lag)
			metricsDone()
			if e == nil {
				meteringBacklog.Set(float64(meteringPending))
				meteringFailed.Set(float64(meteringFailures))
				meteringLag.Set(lag)
				if meteringFailures > 0 {
					logger.Warn("metering work requires operator retry", "category", "metering_failure", "failed", meteringFailures)
				}
			}
		}
	}
	cancel()
	shutdown, done := context.WithTimeout(context.Background(), c.Timeout())
	defer done()
	if e = server.Shutdown(shutdown); e != nil {
		return e
	}
	select {
	case <-workerDone:
	case <-shutdown.Done():
		return shutdown.Err()
	}
	if errors.Is(failure, http.ErrServerClosed) {
		return nil
	}
	return failure
}
