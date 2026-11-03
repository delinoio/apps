// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"net"
	"net/http"
	"sort"
	"sync"
	"time"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/gin-gonic/gin"
	"github.com/google/uuid"
	"github.com/router-for-me/CLIProxyAPI/v6/sdk/api/handlers"
	"github.com/router-for-me/CLIProxyAPI/v6/sdk/api/handlers/claude"
	"github.com/router-for-me/CLIProxyAPI/v6/sdk/api/handlers/gemini"
	"github.com/router-for-me/CLIProxyAPI/v6/sdk/api/handlers/openai"
	"github.com/router-for-me/CLIProxyAPI/v6/sdk/cliproxy"
	coreauth "github.com/router-for-me/CLIProxyAPI/v6/sdk/cliproxy/auth"
	"github.com/router-for-me/CLIProxyAPI/v6/sdk/config"
)

// Credential isolation
//
// Every resource gets its own coreauth.Manager, built lazily on the first
// call and holding only the credentials CredentialSource returned for that
// resource. The SDK selects credentials exclusively from the manager the
// handler was constructed with: handlers call h.AuthManager.Execute*
// (sdk/api/handlers/handlers.go:557, 605, 655) and the manager picks from its
// own m.auths and scheduler (conductor.go:2930 pickNextMixed, 2838
// pickNextMixedLegacy). Each resource's SDK handlers are constructed with
// that resource's manager, and the call middleware routes /r/{id}/... only to
// the runtime of {id}. A call for resource R can therefore only select R's
// credentials. resourceExecutor additionally refuses any auth whose
// mcpa_resource_id attribute differs from the executor's resource or the
// call's resource, so a defect elsewhere fails closed.
//
// The SDK's global model registry is shared by all managers. It is keyed by
// auth ID (UUIDs, unique across resources) for support checks, and the
// provider list it yields for a model only narrows which of R's own auths
// are eligible. The middleware answers model listings from R's credentials
// and rejects models R cannot serve before the SDK sees the call, so the
// registry's cross-resource provider index is never exposed to callers.

// Options configures the engine. Fields marked required must be set.
type Options struct {
	Credentials CredentialSource // required
	Keys        KeyAuthenticator // required
	Authorizer  CallAuthorizer   // required
	Routing     RoutingSource    // required
	Sink        MeterSink        // required
	// Tokens receives refreshed OAuth payloads. When nil, refreshed tokens
	// stay in memory only.
	Tokens  TokenPersister
	Metrics *Metrics
	Logger  *slog.Logger
	// CheckpointInterval is the metering checkpoint interval (required).
	CheckpointInterval time.Duration
	// CheckpointTimeout bounds one MeterSink call. Default 5s.
	CheckpointTimeout time.Duration
	// MaxRequestBodyBytes bounds inbound call bodies. Default 32 MiB.
	MaxRequestBodyBytes int64
	// BaseTransport performs provider requests for executors that use the
	// context RoundTripper. Default: a clone of http.DefaultTransport.
	BaseTransport http.RoundTripper
	// DialUpstream dials provider connections of the SDK Claude client,
	// which ignores BaseTransport (see upstreamRelay). It must apply the same
	// destination policy as BaseTransport. Default: a net.Dialer without
	// destination restrictions.
	DialUpstream DialFunc
	// AutoRefreshInterval is the SDK auth refresh check interval. Default
	// 15 minutes, matching cliproxy.Service. A negative value disables
	// background refresh.
	AutoRefreshInterval time.Duration
	// LoadTimeout bounds loading a resource's credentials. Default 10s.
	LoadTimeout time.Duration
	// NewCallID returns call record IDs. Default: UUID v7.
	NewCallID func() string
	// Now returns the current time. Default: time.Now.
	Now func() time.Time
}

// Engine is the embedded CLIProxyAPI data plane. It serves model calls under
// /r/{resource_id}/ as an http.Handler.
type Engine struct {
	opts      Options
	logger    *slog.Logger
	cfg       *config.Config
	executors map[string]coreauth.ProviderExecutor
	transport *meteringTransport
	relay     *upstreamRelay
	persister *asyncPersister
	router    *gin.Engine
	cp        *checkpointer
	ctx       context.Context
	cancel    context.CancelFunc

	mu       sync.Mutex
	runtimes map[string]*resourceRuntime
	closed   bool
}

var _ http.Handler = (*Engine)(nil)

// New creates the engine. It harvests the SDK executors on first use in the
// process (see harvestExecutors).
func New(opts Options) (*Engine, error) {
	switch {
	case opts.Credentials == nil:
		return nil, errors.New("proxy: Credentials is required")
	case opts.Keys == nil:
		return nil, errors.New("proxy: Keys is required")
	case opts.Authorizer == nil:
		return nil, errors.New("proxy: Authorizer is required")
	case opts.Routing == nil:
		return nil, errors.New("proxy: Routing is required")
	case opts.Sink == nil:
		return nil, errors.New("proxy: Sink is required")
	case opts.CheckpointInterval <= 0:
		return nil, errors.New("proxy: CheckpointInterval must be positive")
	}
	if opts.Logger == nil {
		opts.Logger = slog.Default()
	}
	if opts.CheckpointTimeout <= 0 {
		opts.CheckpointTimeout = 5 * time.Second
	}
	if opts.MaxRequestBodyBytes <= 0 {
		opts.MaxRequestBodyBytes = 32 << 20
	}
	if opts.BaseTransport == nil {
		opts.BaseTransport = http.DefaultTransport.(*http.Transport).Clone()
	}
	if opts.DialUpstream == nil {
		opts.DialUpstream = (&net.Dialer{Timeout: 30 * time.Second, KeepAlive: 30 * time.Second}).DialContext
	}
	if opts.AutoRefreshInterval == 0 {
		opts.AutoRefreshInterval = 15 * time.Minute
	}
	if opts.LoadTimeout <= 0 {
		opts.LoadTimeout = 10 * time.Second
	}
	if opts.NewCallID == nil {
		opts.NewCallID = func() string { return uuid.Must(uuid.NewV7()).String() }
	}
	if opts.Now == nil {
		opts.Now = time.Now
	}
	executors, cfg, err := harvestExecutors()
	if err != nil {
		return nil, fmt.Errorf("proxy: %w", err)
	}
	logger := opts.Logger.With(slog.String("component", "proxy"))
	relay, err := startUpstreamRelay(opts.DialUpstream, logger)
	if err != nil {
		return nil, fmt.Errorf("proxy: %w", err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	e := &Engine{
		opts:      opts,
		logger:    logger,
		cfg:       cfg,
		executors: executors,
		transport: &meteringTransport{base: opts.BaseTransport},
		relay:     relay,
		persister: newAsyncPersister(opts.Tokens, logger, opts.CheckpointTimeout),
		cp: &checkpointer{
			sink:          opts.Sink,
			metrics:       opts.Metrics,
			logger:        logger,
			interval:      opts.CheckpointInterval,
			timeout:       opts.CheckpointTimeout,
			finalAttempts: 3,
			retryDelay:    100 * time.Millisecond,
		},
		ctx:      ctx,
		cancel:   cancel,
		runtimes: map[string]*resourceRuntime{},
	}
	e.router = e.newRouter()
	return e, nil
}

// Close stops background refresh, flushes queued token writes and stops
// accepting upstream relay connections. In-flight calls are not interrupted.
func (e *Engine) Close() error {
	e.mu.Lock()
	if e.closed {
		e.mu.Unlock()
		return nil
	}
	e.closed = true
	runtimes := make([]*resourceRuntime, 0, len(e.runtimes))
	for _, rt := range e.runtimes {
		runtimes = append(runtimes, rt)
	}
	e.runtimes = map[string]*resourceRuntime{}
	e.mu.Unlock()
	for _, rt := range runtimes {
		rt.shutdown()
	}
	e.cancel()
	e.persister.close()
	if err := e.relay.Close(); err != nil {
		e.logger.Warn("closing the upstream relay failed", slog.String("error", err.Error()))
	}
	return nil
}

// ValidateCredential reports whether the engine can use cred.
func ValidateCredential(cred Credential) error {
	_, err := buildAuth(cred, time.Now())
	return err
}

// UpsertCredential registers or replaces a credential. The change applies to
// calls that select a credential after UpsertCredential returns. When the
// resource's runtime is not loaded, the next call loads the credential from
// CredentialSource.
func (e *Engine) UpsertCredential(ctx context.Context, cred Credential) error {
	if err := ValidateCredential(cred); err != nil {
		return err
	}
	rt, err := e.loadedRuntime(ctx, cred.ResourceID)
	if err != nil || rt == nil {
		return err
	}
	return rt.upsert(cred, e.opts.Now())
}

// RemoveCredential removes a credential from new calls immediately. Calls
// that already selected it finish.
func (e *Engine) RemoveCredential(ctx context.Context, resourceID, credentialID string) error {
	rt, err := e.loadedRuntime(ctx, resourceID)
	if err != nil || rt == nil {
		return err
	}
	rt.remove(credentialID)
	e.persister.drop(resourceID, credentialID)
	return nil
}

// InvalidateResource drops a resource's runtime, for example after resource
// deletion. A later call rebuilds it from CredentialSource.
//
// The old runtime shuts down while e.mu is held so a replacement runtime,
// which registers the same credential IDs in the SDK's global model
// registry, cannot be built before the old one unregisters them. Holding
// e.mu while waiting for an in-progress build is safe: build closes ready
// before it takes e.mu.
func (e *Engine) InvalidateResource(resourceID string) {
	e.mu.Lock()
	defer e.mu.Unlock()
	if rt := e.runtimes[resourceID]; rt != nil {
		delete(e.runtimes, resourceID)
		rt.shutdown()
	}
}

// InvalidateAll drops every loaded runtime, for example after the process
// may have missed a credential change made by another replica. Later calls
// rebuild runtimes from CredentialSource.
// It holds e.mu during shutdown for the same reason as InvalidateResource.
func (e *Engine) InvalidateAll() {
	e.mu.Lock()
	defer e.mu.Unlock()
	for id, rt := range e.runtimes {
		delete(e.runtimes, id)
		rt.shutdown()
	}
}

// loadedRuntime returns the runtime when it exists, waiting for an
// in-progress load.
func (e *Engine) loadedRuntime(ctx context.Context, resourceID string) (*resourceRuntime, error) {
	e.mu.Lock()
	rt := e.runtimes[resourceID]
	e.mu.Unlock()
	if rt == nil {
		return nil, nil
	}
	select {
	case <-rt.ready:
	case <-ctx.Done():
		return nil, ctx.Err()
	}
	if rt.err != nil {
		return nil, nil
	}
	return rt, nil
}

// runtime returns the resource's runtime, building it on first use.
func (e *Engine) runtime(ctx context.Context, resourceID string) (*resourceRuntime, error) {
	e.mu.Lock()
	if e.closed {
		e.mu.Unlock()
		return nil, errors.New("engine closed")
	}
	rt := e.runtimes[resourceID]
	if rt == nil {
		rt = &resourceRuntime{id: resourceID, ready: make(chan struct{})}
		e.runtimes[resourceID] = rt
		e.mu.Unlock()
		e.build(ctx, rt)
		close(rt.ready)
		if rt.err != nil {
			e.mu.Lock()
			if e.runtimes[resourceID] == rt {
				delete(e.runtimes, resourceID)
			}
			e.mu.Unlock()
		}
	} else {
		e.mu.Unlock()
	}
	select {
	case <-rt.ready:
	case <-ctx.Done():
		return nil, ctx.Err()
	}
	if rt.err != nil {
		return nil, rt.err
	}
	return rt, nil
}

func (e *Engine) build(ctx context.Context, rt *resourceRuntime) {
	rt.logger = e.logger.With(slog.String("resource_id", rt.id))
	rt.store = newResourceStore(rt.id, e.persister)
	manager := coreauth.NewManager(rt.store, &coreauth.RoundRobinSelector{}, nil)
	manager.SetConfig(e.cfg)
	// One attempt, one credential, no cooldown wait (no retry across accounts
	// or providers).
	manager.SetRetryConfig(0, 0, 1)
	manager.SetRoundTripperProvider(roundTripperProvider{rt: e.transport})
	for key, executor := range e.executors {
		manager.RegisterExecutor(&resourceExecutor{inner: executor, resourceID: rt.id, sdkProvider: key, relay: e.relay, logger: rt.logger})
	}
	rt.manager = manager
	base := handlers.NewBaseAPIHandlers(&e.cfg.SDKConfig, manager)
	rt.openai = openai.NewOpenAIAPIHandler(base)
	rt.responses = openai.NewOpenAIResponsesAPIHandler(base)
	rt.claude = claude.NewClaudeCodeAPIHandler(base)
	rt.gemini = gemini.NewGeminiAPIHandler(base)
	rt.creds = map[string]runtimeCredential{}

	loadCtx, cancel := context.WithTimeout(context.WithoutCancel(ctx), e.opts.LoadTimeout)
	defer cancel()
	creds, err := e.opts.Credentials.LoadResourceCredentials(loadCtx, rt.id)
	if err != nil {
		rt.err = fmt.Errorf("load credentials: %w", err)
		return
	}
	now := e.opts.Now()
	for _, cred := range creds {
		if cred.ResourceID != rt.id {
			rt.logger.Error("credential source returned a credential of another resource",
				slog.String("credential_id", cred.ID))
			continue
		}
		if err := rt.upsert(cred, now); err != nil {
			rt.logger.Warn("skipping unusable credential",
				slog.String("credential_id", cred.ID),
				slog.String("error", err.Error()))
		}
	}
	if e.opts.AutoRefreshInterval > 0 {
		manager.StartAutoRefresh(e.ctx, e.opts.AutoRefreshInterval)
	}
}

type roundTripperProvider struct {
	rt http.RoundTripper
}

func (p roundTripperProvider) RoundTripperFor(*coreauth.Auth) http.RoundTripper { return p.rt }

// runtimeCredential is the non-secret view of a registered credential.
type runtimeCredential struct {
	provider mcpav1.Provider
	models   []*cliproxy.ModelInfo
}

// resourceRuntime is the in-process SDK state of one resource.
type resourceRuntime struct {
	id     string
	ready  chan struct{}
	err    error
	logger *slog.Logger

	manager   *coreauth.Manager
	store     *resourceStore
	openai    *openai.OpenAIAPIHandler
	responses *openai.OpenAIResponsesAPIHandler
	claude    *claude.ClaudeCodeAPIHandler
	gemini    *gemini.GeminiAPIHandler

	mu    sync.RWMutex
	creds map[string]runtimeCredential
}

func (rt *resourceRuntime) upsert(cred Credential, now time.Time) error {
	built, err := buildAuth(cred, now)
	if err != nil {
		return err
	}
	ctx := coreauth.WithSkipPersist(context.Background())
	rt.mu.Lock()
	defer rt.mu.Unlock()
	// Models are registered before the auth so the scheduler builds the
	// auth's supported-model set from them (see Service.applyCoreAuthAddOrUpdate).
	cliproxy.GlobalModelRegistry().RegisterClient(built.auth.ID, built.sdkProvider, built.models)
	if _, exists := rt.manager.GetByID(built.auth.ID); exists {
		_, err = rt.manager.Update(ctx, built.auth)
	} else {
		_, err = rt.manager.Register(ctx, built.auth)
	}
	if err != nil {
		cliproxy.GlobalModelRegistry().UnregisterClient(built.auth.ID)
		return err
	}
	rt.manager.RefreshSchedulerEntry(built.auth.ID)
	if cred.Kind == mcpav1.CredentialKind_CREDENTIAL_KIND_OAUTH {
		rt.store.track(built.auth)
	}
	rt.creds[cred.ID] = runtimeCredential{provider: cred.Provider, models: built.models}
	return nil
}

func (rt *resourceRuntime) remove(credentialID string) {
	rt.mu.Lock()
	defer rt.mu.Unlock()
	delete(rt.creds, credentialID)
	rt.store.forget(credentialID)
	cliproxy.GlobalModelRegistry().UnregisterClient(credentialID)
	// The SDK manager has no delete operation; Service.applyCoreAuthRemoval
	// disables the auth, which removes it from selection immediately.
	if existing, ok := rt.manager.GetByID(credentialID); ok && existing != nil {
		existing.Disabled = true
		existing.Status = coreauth.StatusDisabled
		existing.Metadata = nil
		existing.Attributes = map[string]string{attrResourceID: rt.id, attrCredentialID: credentialID}
		_, _ = rt.manager.Update(coreauth.WithSkipPersist(context.Background()), existing)
	}
}

func (rt *resourceRuntime) shutdown() {
	<-rt.ready
	if rt.err != nil || rt.manager == nil {
		return
	}
	rt.manager.StopAutoRefresh()
	rt.mu.Lock()
	defer rt.mu.Unlock()
	for id := range rt.creds {
		cliproxy.GlobalModelRegistry().UnregisterClient(id)
	}
	rt.creds = map[string]runtimeCredential{}
}

// models returns the resource's models, deduplicated by ID and sorted.
func (rt *resourceRuntime) models() []*cliproxy.ModelInfo {
	rt.mu.RLock()
	defer rt.mu.RUnlock()
	seen := map[string]*cliproxy.ModelInfo{}
	for _, cred := range rt.creds {
		for _, model := range cred.models {
			if model == nil || model.ID == "" {
				continue
			}
			if _, ok := seen[model.ID]; !ok {
				seen[model.ID] = model
			}
		}
	}
	out := make([]*cliproxy.ModelInfo, 0, len(seen))
	for _, model := range seen {
		out = append(out, model)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].ID < out[j].ID })
	return out
}

// serves reports whether a credential of the resource serves model.
func (rt *resourceRuntime) serves(model string) bool {
	model = baseModelName(model)
	rt.mu.RLock()
	defer rt.mu.RUnlock()
	for _, cred := range rt.creds {
		for _, info := range cred.models {
			if info != nil && info.ID == model {
				return true
			}
		}
	}
	return false
}
