// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"errors"
	"time"

	"github.com/prometheus/client_golang/prometheus"
)

// Metrics holds the data-plane Prometheus collectors. Labels are bounded and
// never carry tenant, resource, key or model identifiers.
type Metrics struct {
	calls              *prometheus.CounterVec
	callDuration       *prometheus.HistogramVec
	providerFailures   *prometheus.CounterVec
	checkpointFailures prometheus.Counter
	budgetRejections   *prometheus.CounterVec
	billableBytes      *prometheus.CounterVec
	httpRequests       *prometheus.CounterVec
}

// NewMetrics registers the data-plane collectors on reg. httpRequests is the
// shared mcpa_http_requests_total{surface,code_class} counter owned by the
// server core; pass nil when the core does not provide it.
func NewMetrics(reg prometheus.Registerer, httpRequests *prometheus.CounterVec) (*Metrics, error) {
	m := &Metrics{
		calls: prometheus.NewCounterVec(prometheus.CounterOpts{
			Name: "mcpa_calls_total",
			Help: "Model calls by client format, provider and call status.",
		}, []string{"format", "provider", "status"}),
		callDuration: prometheus.NewHistogramVec(prometheus.HistogramOpts{
			Name:    "mcpa_call_duration_seconds",
			Help:    "Model call duration from request receipt to the last byte sent to the caller.",
			Buckets: []float64{0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30, 60, 120, 300, 600},
		}, []string{"format", "status"}),
		providerFailures: prometheus.NewCounterVec(prometheus.CounterOpts{
			Name: "mcpa_provider_failures_total",
			Help: "Provider failures by provider and status class.",
		}, []string{"provider", "status_class"}),
		checkpointFailures: prometheus.NewCounter(prometheus.CounterOpts{
			Name: "mcpa_metering_checkpoint_failures_total",
			Help: "Metering checkpoints that failed to commit.",
		}),
		budgetRejections: prometheus.NewCounterVec(prometheus.CounterOpts{
			Name: "mcpa_budget_rejections_total",
			Help: "Calls rejected because a budget was exhausted.",
		}, []string{"scope"}),
		billableBytes: prometheus.NewCounterVec(prometheus.CounterOpts{
			Name: "mcpa_billable_bytes_total",
			Help: "Committed billable bytes by segment.",
		}, []string{"segment"}),
		httpRequests: httpRequests,
	}
	if reg != nil {
		for _, collector := range []prometheus.Collector{
			m.calls, m.callDuration, m.providerFailures, m.checkpointFailures, m.budgetRejections, m.billableBytes,
		} {
			if err := reg.Register(collector); err != nil {
				var already prometheus.AlreadyRegisteredError
				if errors.As(err, &already) {
					continue
				}
				return nil, err
			}
		}
	}
	return m, nil
}

func (m *Metrics) observeCall(format, provider, status string, duration time.Duration) {
	if m == nil {
		return
	}
	m.calls.WithLabelValues(format, provider, status).Inc()
	m.callDuration.WithLabelValues(format, status).Observe(duration.Seconds())
}

func (m *Metrics) providerFailure(provider, statusClass string) {
	if m == nil || statusClass == "" {
		return
	}
	m.providerFailures.WithLabelValues(provider, statusClass).Inc()
}

func (m *Metrics) checkpointFailure() {
	if m == nil {
		return
	}
	m.checkpointFailures.Inc()
}

func (m *Metrics) budgetRejection(scope BudgetScope) {
	if m == nil {
		return
	}
	m.budgetRejections.WithLabelValues(scope.String()).Inc()
}

func (m *Metrics) billed(requestBytes, responseBytes int64) {
	if m == nil {
		return
	}
	if requestBytes > 0 {
		m.billableBytes.WithLabelValues("request").Add(float64(requestBytes))
	}
	if responseBytes > 0 {
		m.billableBytes.WithLabelValues("response").Add(float64(responseBytes))
	}
}

func (m *Metrics) httpRequest(status int) {
	if m == nil || m.httpRequests == nil {
		return
	}
	class := "5xx"
	switch {
	case status < 200:
		class = "1xx"
	case status < 300:
		class = "2xx"
	case status < 400:
		class = "3xx"
	case status < 500:
		class = "4xx"
	}
	m.httpRequests.WithLabelValues("proxy", class).Inc()
}
