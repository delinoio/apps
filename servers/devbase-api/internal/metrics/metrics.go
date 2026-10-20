// SPDX-License-Identifier: FSL-1.1-ALv2

// Package metrics defines Prometheus collectors. Labels are bounded enumerations;
// tenant, project, user and key IDs never become labels.
package metrics

import "github.com/prometheus/client_golang/prometheus"

type Metrics struct {
	Requests        *prometheus.CounterVec
	Duration        *prometheus.HistogramVec
	LimitRejections *prometheus.CounterVec
	CleanupFailures prometheus.Counter
	CleanupRuns     prometheus.Counter
}

func New(reg prometheus.Registerer) *Metrics {
	m := &Metrics{
		Requests:        prometheus.NewCounterVec(prometheus.CounterOpts{Name: "devbase_requests_total", Help: "Requests by procedure and Connect code."}, []string{"procedure", "code"}),
		Duration:        prometheus.NewHistogramVec(prometheus.HistogramOpts{Name: "devbase_request_duration_seconds", Help: "Request latency by procedure.", Buckets: prometheus.DefBuckets}, []string{"procedure"}),
		LimitRejections: prometheus.NewCounterVec(prometheus.CounterOpts{Name: "devbase_limit_rejections_total", Help: "Rejections by limit kind."}, []string{"kind"}),
		CleanupFailures: prometheus.NewCounter(prometheus.CounterOpts{Name: "devbase_cleanup_failures_total", Help: "Tenant cleanup batches that failed and will retry."}),
		CleanupRuns:     prometheus.NewCounter(prometheus.CounterOpts{Name: "devbase_cleanup_runs_total", Help: "Completed maintenance cycles."}),
	}
	reg.MustRegister(m.Requests, m.Duration, m.LimitRejections, m.CleanupFailures, m.CleanupRuns)
	return m
}
