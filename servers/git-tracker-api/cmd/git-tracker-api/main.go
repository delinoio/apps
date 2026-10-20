// SPDX-License-Identifier: FSL-1.1-ALv2

package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/url"
	"os"
	"os/signal"
	"strings"
	"syscall"
	"time"

	"github.com/coreos/go-oidc/v3/oidc"
	"github.com/delinoio/apps/protos/delcore/v1/delcorev1connect"
	"github.com/delinoio/apps/servers/git-tracker-api/internal/operations"
	"github.com/delinoio/apps/servers/git-tracker-api/internal/service"
	"github.com/delinoio/apps/servers/git-tracker-api/migrations"
	"github.com/golang-jwt/jwt/v4"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promhttp"
	"go.opentelemetry.io/contrib/instrumentation/net/http/otelhttp"
	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/exporters/otlp/otlptrace/otlptracehttp"
	"go.opentelemetry.io/otel/sdk/trace"
)

func required(key string) (string, error) {
	v := os.Getenv(key)
	if v == "" {
		return "", fmt.Errorf("%s is required", key)
	}
	return v, nil
}
func main() {
	slog.SetDefault(slog.New(slog.NewTextHandler(terminalWriter{writer: os.Stderr, color: os.Getenv("NO_COLOR") == ""}, nil)))
	if e := run(); e != nil {
		slog.Error("server stopped", "error", e.Error())
		os.Exit(1)
	}
}
func run() error {
	if len(os.Args) > 1 && os.Args[1] == "alerts" {
		rules, e := operations.AlertRules(os.Getenv)
		if e != nil {
			return e
		}
		fmt.Print(rules)
		return nil
	}
	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGTERM, syscall.SIGINT)
	defer stop()
	dsn, e := required("DATABASE_URL")
	if e != nil {
		return e
	}
	db, e := pgxpool.New(ctx, dsn)
	if e != nil {
		return errors.New("database configuration failed")
	}
	defer db.Close()
	if len(os.Args) > 1 && os.Args[1] == "migrate" {
		_, e = db.Exec(ctx, migrations.Initial)
		return e
	}
	retention, e := required("AUDIT_RETENTION")
	if e != nil {
		return e
	}
	duration, e := time.ParseDuration(retention)
	if e != nil || duration <= 0 {
		return errors.New("positive audit retention is required")
	}
	settings := map[string]string{}
	for _, key := range []string{"GITHUB_APP_ID", "GITHUB_KEY_FILE", "WEBHOOK_SECRET", "DELCORE_URL", "LOGTO_TOKEN_URL", "CLIENT_ID", "CLIENT_SECRET", "DELCORE_RESOURCE", "WEB_ORIGIN"} {
		v, e := required(key)
		if e != nil {
			return e
		}
		settings[key] = v
	}
	pem, e := os.ReadFile(settings["GITHUB_KEY_FILE"])
	if e != nil {
		return errors.New("GitHub key unavailable")
	}
	key, e := jwt.ParseRSAPrivateKeyFromPEM(pem)
	if e != nil {
		return errors.New("GitHub key invalid")
	}
	client := &http.Client{Timeout: 30 * time.Second}
	g := &service.GitHubClient{HTTP: client, BaseURL: "https://api.github.com", AppID: settings["GITHUB_APP_ID"], Key: key}
	provider, e := oidc.NewProvider(oidc.ClientContext(ctx, client), "https://token.actions.githubusercontent.com")
	if e != nil {
		return errors.New("Actions OIDC discovery failed")
	}
	authority := &service.DelcoreAuthority{Client: delcorev1connect.NewDelcoreServiceClient(client, settings["DELCORE_URL"]), Actions: provider.Verifier(&oidc.Config{ClientID: "https://git-tracker.api.delino.io"}), GitHub: g}
	authority.ServiceToken = func(ctx context.Context) (string, error) {
		form := url.Values{"grant_type": {"client_credentials"}, "resource": {settings["DELCORE_RESOURCE"]}, "scope": {"authorize"}}
		req, e := http.NewRequestWithContext(ctx, "POST", settings["LOGTO_TOKEN_URL"], strings.NewReader(form.Encode()))
		if e != nil {
			return "", e
		}
		req.SetBasicAuth(settings["CLIENT_ID"], settings["CLIENT_SECRET"])
		req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
		resp, e := client.Do(req)
		if e != nil {
			return "", errors.New("service token unavailable")
		}
		defer resp.Body.Close()
		if resp.StatusCode != 200 {
			return "", errors.New("service token rejected")
		}
		var result struct {
			Token string `json:"access_token"`
		}
		e = json.NewDecoder(resp.Body).Decode(&result)
		return result.Token, e
	}
	s := &service.Server{DB: db, Auth: authority, GitHub: g, Log: slog.Default(), AuditRetention: duration}
	exporter, e := otlptracehttp.New(ctx)
	if e != nil {
		return e
	}
	tracer := trace.NewTracerProvider(trace.WithBatcher(exporter))
	defer tracer.Shutdown(context.Background())
	otel.SetTracerProvider(tracer)
	registry := prometheus.NewRegistry()
	requests := prometheus.NewCounterVec(prometheus.CounterOpts{Name: "git_tracker_http_requests_total", Help: "Requests by bounded outcome."}, []string{"outcome"})
	events := prometheus.NewCounterVec(prometheus.CounterOpts{Name: "git_tracker_events_total", Help: "Bounded Git Tracker lifecycle events."}, []string{"kind", "category"})
	registry.MustRegister(requests, events)
	s.Event = func(kind, category string) { events.WithLabelValues(kind, category).Inc() }
	mux := http.NewServeMux()
	path, handler := s.Handler()
	mux.Handle(path, handler)
	mux.Handle("/github/events", s.Webhook(settings["WEBHOOK_SECRET"]))
	mux.Handle("/metrics", promhttp.HandlerFor(registry, promhttp.HandlerOpts{}))
	mux.HandleFunc("/healthz", func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(200) })
	mux.HandleFunc("/readyz", func(w http.ResponseWriter, r *http.Request) {
		if e := db.Ping(r.Context()); e != nil {
			w.WriteHeader(503)
			return
		}
		var exists bool
		if e := db.QueryRow(r.Context(), "SELECT to_regclass('git_tracker_repositories') IS NOT NULL").Scan(&exists); e != nil || !exists {
			w.WriteHeader(503)
			return
		}
		w.WriteHeader(200)
	})
	cors := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Cache-Control", "no-store")
		if origin := r.Header.Get("Origin"); origin != "" {
			if origin != settings["WEB_ORIGIN"] {
				w.WriteHeader(403)
				return
			}
			w.Header().Set("Access-Control-Allow-Origin", origin)
			w.Header().Set("Vary", "Origin")
			w.Header().Set("Access-Control-Allow-Headers", "Authorization, Content-Type, Connect-Protocol-Version")
			w.Header().Set("Access-Control-Expose-Headers", "X-Correlation-Id")
			w.Header().Set("Access-Control-Allow-Methods", "POST, OPTIONS")
		}
		if r.Method == "OPTIONS" {
			w.WriteHeader(204)
			return
		}
		recorder := &statusWriter{ResponseWriter: w, status: 200}
		mux.ServeHTTP(recorder, r)
		outcome := "success"
		if recorder.status >= 500 {
			outcome = "server_error"
		} else if recorder.status >= 400 {
			outcome = "client_error"
		}
		requests.WithLabelValues(outcome).Inc()
	})
	address := os.Getenv("LISTEN")
	if address == "" {
		address = "127.0.0.1:46321"
	}
	server := &http.Server{Addr: address, Handler: otelhttp.NewHandler(cors, "git-tracker"), ReadHeaderTimeout: 10 * time.Second, ReadTimeout: 60 * time.Second, WriteTimeout: 60 * time.Second, IdleTimeout: 60 * time.Second}
	go func() {
		ticker := time.NewTicker(time.Minute)
		defer ticker.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				if e := s.Maintain(ctx); e != nil {
					slog.ErrorContext(ctx, "maintenance failed", "category", "persistence")
				}
			}
		}
	}()
	done := make(chan error, 1)
	go func() { done <- server.ListenAndServe() }()
	slog.Info("server listening", "address", address)
	select {
	case e := <-done:
		if e != http.ErrServerClosed {
			return e
		}
	case <-ctx.Done():
		shutdown, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		return server.Shutdown(shutdown)
	}
	return nil
}

type statusWriter struct {
	http.ResponseWriter
	status int
}

func (w *statusWriter) WriteHeader(status int) {
	w.status = status
	w.ResponseWriter.WriteHeader(status)
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
	_, e := fmt.Fprintf(w.writer, "%s%s\x1b[0m", color, p)
	if e != nil {
		return 0, e
	}
	return len(p), nil
}
