// SPDX-License-Identifier: FSL-1.1-ALv2

// Package dataplane connects the embedded CLIProxyAPI engine to PostgreSQL,
// Delcore and the management service. It implements the engine's
// credential, key, authorization, routing and metering interfaces, the
// service's usage, spend and credential hooks, the usage outbox worker and
// the retention and tenant-lifecycle workers.
package dataplane

import (
	"context"
	"errors"
	"log/slog"
	"net/http"
	"time"

	delcorev1 "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/crypto"
	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
	"github.com/delinoio/apps/servers/mcpa-api/internal/obs"
	"github.com/delinoio/apps/servers/mcpa-api/internal/proxy"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

// Delcore is the subset of the Delcore client the data plane uses.
type Delcore interface {
	Authorize(ctx context.Context, tenantID, userID string, capability delcore.Capability, paid bool) (delcore.Decision, error)
	TenantState(ctx context.Context, tenantID string) (delcorev1.TenantState, error)
	GetTenantStates(ctx context.Context, tenantIDs []string) (map[string]delcorev1.TenantState, error)
	CheckTenantLifecycle(ctx context.Context, tenantIDs []string) ([]string, error)
	BillingPeriod(ctx context.Context, tenantID string) (delcore.Period, error)
	IngestEvent(ctx context.Context, requestKey string, event *delcorev1.Event) (*delcorev1.IngestEventResponse, error)
}

var _ Delcore = (*delcore.Client)(nil)

// Config configures the data plane.
type Config struct {
	Store   *store.Store
	Keys    *crypto.Keyring
	Delcore Delcore
	Metrics *obs.Metrics
	Log     *slog.Logger
	// CheckpointInterval is MCPA_CHECKPOINT_INTERVAL.
	CheckpointInterval time.Duration
	// OutboxInterval is MCPA_OUTBOX_INTERVAL; it is also the first outbox
	// retry backoff.
	OutboxInterval time.Duration
	// DependencyTimeout bounds each database and Delcore call of the workers.
	DependencyTimeout time.Duration
	// RetentionInterval and LifecycleInterval default to one hour.
	RetentionInterval time.Duration
	LifecycleInterval time.Duration
	// BaseTransport performs provider requests. The default refuses to dial
	// private, loopback and link-local addresses.
	BaseTransport http.RoundTripper
	// DialUpstream dials provider connections of the SDK Claude client,
	// which does not use BaseTransport. The default applies the same
	// public-address guard as the default BaseTransport.
	DialUpstream proxy.DialFunc
	// Now defaults to time.Now.
	Now func() time.Time
}

// DataPlane owns the engine and its adapters.
type DataPlane struct {
	cfg     Config
	db      *store.Store
	log     *slog.Logger
	now     func() time.Time
	Engine  *proxy.Engine
	Sync    *CredentialSync
	Usage   *UsageReader
	Outbox  *OutboxWorker
	metrics *Metrics
}

// New builds the engine and its adapters. It does not start workers.
func New(cfg Config) (*DataPlane, error) {
	if cfg.Store == nil || cfg.Keys == nil || cfg.Delcore == nil {
		return nil, errors.New("dataplane: store, keys and Delcore are required")
	}
	if cfg.CheckpointInterval <= 0 || cfg.OutboxInterval <= 0 {
		return nil, errors.New("dataplane: checkpoint and outbox intervals must be positive")
	}
	if cfg.Log == nil {
		cfg.Log = slog.Default()
	}
	if cfg.Now == nil {
		cfg.Now = time.Now
	}
	if cfg.DependencyTimeout <= 0 {
		cfg.DependencyTimeout = 10 * time.Second
	}
	if cfg.RetentionInterval <= 0 {
		cfg.RetentionInterval = time.Hour
	}
	if cfg.LifecycleInterval <= 0 {
		cfg.LifecycleInterval = time.Hour
	}
	if cfg.BaseTransport == nil {
		cfg.BaseTransport = PublicTransport()
	}
	if cfg.DialUpstream == nil {
		cfg.DialUpstream = PublicDialer().DialContext
	}
	log := cfg.Log.With("component", "dataplane")
	metrics, err := NewMetrics(cfg.Metrics)
	if err != nil {
		return nil, err
	}
	var engineMetrics *proxy.Metrics
	if cfg.Metrics != nil {
		if engineMetrics, err = proxy.NewMetrics(cfg.Metrics.Registry, cfg.Metrics.HTTPRequests); err != nil {
			return nil, err
		}
	}
	dp := &DataPlane{cfg: cfg, db: cfg.Store, log: log, now: cfg.Now, metrics: metrics}
	creds := &Credentials{db: cfg.Store, keys: cfg.Keys, log: log}
	dp.Sync = &CredentialSync{db: cfg.Store, keys: cfg.Keys, log: log, instance: store.NewID().String()}
	creds.notify = dp.Sync.Notify
	keys := NewKeyAuthenticator(cfg.Store, log, cfg.Now)
	authorizer := NewCallAuthorizer(cfg.Store, cfg.Delcore, log, cfg.Now, cfg.CheckpointInterval)
	engine, err := proxy.New(proxy.Options{
		Credentials:        creds,
		Tokens:             creds,
		Keys:               keys,
		Authorizer:         authorizer,
		Routing:            Routing{db: cfg.Store},
		Sink:               &MeterSink{db: cfg.Store},
		Metrics:            engineMetrics,
		Logger:             cfg.Log,
		CheckpointInterval: cfg.CheckpointInterval,
		CheckpointTimeout:  cfg.DependencyTimeout,
		BaseTransport:      cfg.BaseTransport,
		DialUpstream:       cfg.DialUpstream,
		Now:                cfg.Now,
	})
	if err != nil {
		return nil, err
	}
	dp.Engine = engine
	dp.Sync.engine = engine
	dp.Usage = &UsageReader{db: cfg.Store}
	dp.Outbox = &OutboxWorker{
		db: cfg.Store, dc: cfg.Delcore, log: log, now: cfg.Now, metrics: metrics,
		interval: cfg.OutboxInterval, timeout: cfg.DependencyTimeout,
	}
	return dp, nil
}

// Close stops the engine's background refresh and flushes token writes.
func (dp *DataPlane) Close() error { return dp.Engine.Close() }

// every runs fn immediately and then every interval until ctx ends. Errors
// are logged and never stop the loop.
func every(ctx context.Context, interval time.Duration, log *slog.Logger, name string, fn func(context.Context) error) error {
	ticker := time.NewTicker(interval)
	defer ticker.Stop()
	for {
		if err := fn(ctx); err != nil && ctx.Err() == nil {
			log.Error("worker cycle failed", "worker", name, "error", err)
		}
		select {
		case <-ctx.Done():
			return nil
		case <-ticker.C:
		}
	}
}
