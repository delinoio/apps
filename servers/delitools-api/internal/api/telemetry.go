// SPDX-License-Identifier: FSL-1.1-ALv2
package api

import (
	"connectrpc.com/connect"
	"context"
	"errors"
	pb "github.com/delinoio/apps/protos/delitools/v1"
	"github.com/jackc/pgx/v5"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promhttp"
	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/attribute"
	"net/http"
	"time"
)

// Trace only the fixed RPC procedure and bounded outcome. HTTP middleware would
// also record caller-controlled URL query strings, which can contain raw input.
func (s *Server) interceptor() connect.Interceptor {
	return connect.UnaryInterceptorFunc(func(next connect.UnaryFunc) connect.UnaryFunc {
		return func(ctx context.Context, req connect.AnyRequest) (connect.AnyResponse, error) {
			ctx, span := otel.Tracer("delitools").Start(ctx, req.Spec().Procedure)
			defer span.End()
			out, e := next(ctx, req)
			if e != nil {
				span.SetAttributes(attribute.String("rpc.outcome", connect.CodeOf(e).String()))
				id := ID()
				var ce *connect.Error
				if errors.As(e, &ce) {
					if value := ce.Meta().Get("X-DeliTools-Request-Id"); validID(value) {
						id = value
					}
					for _, detail := range ce.Details() {
						value, err := detail.Value()
						if err == nil {
							if typed, ok := value.(*pb.ErrorDetail); ok {
								switch typed.Reason {
								case pb.ErrorReason_ERROR_REASON_PRICE_UNAVAILABLE:
									s.missingPrices.Add(1)
								case pb.ErrorReason_ERROR_REASON_UNAUTHENTICATED, pb.ErrorReason_ERROR_REASON_FORBIDDEN, pb.ErrorReason_ERROR_REASON_BILLING_BLOCKED:
									s.authorizationFailures.Add(1)
								}
							}
						}
					}
				}
				span.SetAttributes(attribute.String("request.id", id))
				s.rpcFailures.Add(1)
			} else {
				span.SetAttributes(attribute.String("rpc.outcome", "ok"))
				if response, ok := out.Any().(interface{ GetRequestId() string }); ok {
					span.SetAttributes(attribute.String("request.id", response.GetRequestId()))
				}
			}
			return out, e
		}
	})
}
func (s *Server) Metrics() http.Handler {
	registry := prometheus.NewRegistry()
	// Public RPC failures never write retained rows. Fixed process counters bound
	// memory and avoid letting unauthenticated traffic amplify database writes.
	registry.MustRegister(prometheus.NewCounterFunc(prometheus.CounterOpts{Name: "delitools_authorization_failures", Help: "Rejected credential, permission and billing checks since process start."}, func() float64 { return float64(s.authorizationFailures.Load()) }))
	registry.MustRegister(prometheus.NewCounterFunc(prometheus.CounterOpts{Name: "delitools_missing_prices", Help: "Requests rejected without prices since process start."}, func() float64 { return float64(s.missingPrices.Load()) }))
	registry.MustRegister(prometheus.NewCounterFunc(prometheus.CounterOpts{Name: "delitools_rpc_failures", Help: "Failed RPCs since process start."}, func() float64 { return float64(s.rpcFailures.Load()) }))
	for _, item := range []struct{ name, help, query string }{{"pending_usage", "Undelivered usage attempts.", "SELECT count(*) FROM attempts WHERE NOT delivered"}, {"unknown_usage", "Attempts with unknown measured usage.", "SELECT count(*) FROM attempts WHERE usage IS NULL"}, {"unknown_requests", "Requests with unknown execution state.", "SELECT count(*) FROM requests WHERE state=3"}, {"provider_failures", "Provider failures within retained operational history.", "SELECT count(*) FROM operational_logs WHERE category='provider_failure'"}, {"price_sync_failures", "Catalog synchronization failures within retained history.", "SELECT count(*) FROM operational_logs WHERE category='price_sync_failure'"}} {
		registry.MustRegister(prometheus.NewGaugeFunc(prometheus.GaugeOpts{Name: "delitools_" + item.name, Help: item.help}, func() float64 {
			ctx, cancel := context.WithTimeout(context.Background(), time.Second)
			defer cancel()
			var n int64
			if s.DB.Pool.QueryRow(ctx, item.query).Scan(&n) != nil {
				return -1
			}
			return float64(n)
		}))
	}
	registry.MustRegister(prometheus.NewGaugeFunc(prometheus.GaugeOpts{Name: "delitools_expiring_usage", Help: "Pending attempts within the configured expiration alert window."}, func() float64 {
		ctx, cancel := context.WithTimeout(context.Background(), time.Second)
		defer cancel()
		v, e := s.DB.Settings(ctx)
		if e != nil {
			return -1
		}
		var n int64
		if s.DB.Pool.QueryRow(ctx, "SELECT count(*) FROM attempts WHERE NOT delivered AND created_at<=$1", s.now().Add(-Retention+time.Duration(v.AlertExpirySeconds)*time.Second)).Scan(&n) != nil {
			return -1
		}
		return float64(n)
	}))
	return promhttp.HandlerFor(registry, promhttp.HandlerOpts{})
}
func (s *Server) Alerts(ctx context.Context) error {
	v, e := s.DB.Settings(ctx)
	if e != nil {
		return e
	}
	var backlog, expiring int
	if e = s.DB.Pool.QueryRow(ctx, "SELECT count(*),count(*) FILTER(WHERE created_at<=$1) FROM attempts WHERE NOT delivered", s.now().Add(-Retention+time.Duration(v.AlertExpirySeconds)*time.Second)).Scan(&backlog, &expiring); e != nil {
		return e
	}
	if backlog >= int(v.AlertBacklog) {
		if err := s.alertEvent(ctx, "backlog_alert"); err != nil {
			return err
		}
	}
	if expiring > 0 {
		if err := s.alertEvent(ctx, "expiry_alert"); err != nil {
			return err
		}
	}
	return nil
}

// The cooldown survives process restarts. Alert rows retain the normal 30-day TTL.
func (s *Server) alertEvent(ctx context.Context, category string) error {
	now := s.now()
	id := ID()
	inserted := false
	err := begin(ctx, s.DB, func(tx pgx.Tx) error {
		if _, err := tx.Exec(ctx, "SELECT pg_advisory_xact_lock(41824704)"); err != nil {
			return err
		}
		result, err := tx.Exec(ctx, "INSERT INTO operational_logs SELECT $1,$2,$3,$4 WHERE NOT EXISTS(SELECT 1 FROM operational_logs WHERE category=$2 AND created_at>$5)", ID(), category, id, now, now.Add(-time.Hour))
		if err == nil {
			inserted = result.RowsAffected() == 1
		}
		return err
	})
	if err == nil && inserted && s.Log != nil {
		s.Log.Warn("DeliTools alert", "category", category, "request_id", id)
	}
	return err
}
