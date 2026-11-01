// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"context"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/gin-gonic/gin"
	sdkapi "github.com/router-for-me/CLIProxyAPI/v6/sdk/api"
	sdkauth "github.com/router-for-me/CLIProxyAPI/v6/sdk/auth"
	"github.com/router-for-me/CLIProxyAPI/v6/sdk/cliproxy"
	coreauth "github.com/router-for-me/CLIProxyAPI/v6/sdk/cliproxy/auth"
	"github.com/router-for-me/CLIProxyAPI/v6/sdk/config"
	"github.com/router-for-me/CLIProxyAPI/v6/sdk/logging"
	// The SDK registers its schema translators only through this package
	// (cmd/server/main.go imports it the same way); without it some
	// format pairs reach providers untranslated.
	_ "github.com/router-for-me/CLIProxyAPI/v6/sdk/translator/builtin"
	"github.com/sirupsen/logrus"
)

// newSDKConfig returns the SDK configuration used by every resource. All SDK
// features that write files, log bodies, aggregate usage, serve management
// endpoints or retry are disabled.
func newSDKConfig() *config.Config {
	cfg := &config.Config{}
	// The bootstrap service must fail to listen; see harvestExecutors.
	cfg.Host = "127.0.0.1"
	cfg.Port = -1
	// The SDK requires an existing auth directory (Service.ensureAuthDir) but
	// mcpa never writes to it: the global token store is replaced by
	// noFileTokenStore and no watcher or file synthesizer runs.
	cfg.AuthDir = os.TempDir()
	cfg.CommercialMode = true
	cfg.RequestLog = false
	cfg.LoggingToFile = false
	cfg.UsageStatisticsEnabled = false
	cfg.RemoteManagement.SecretKey = ""
	cfg.RemoteManagement.AllowRemote = false
	cfg.RemoteManagement.DisableControlPanel = true
	// No retries: one attempt with one credential, no cooldown-based
	// exclusion of credentials after provider errors, and no streaming
	// bootstrap retries (sdk/api/handlers/handlers.go:180).
	cfg.RequestRetry = 0
	cfg.MaxRetryCredentials = 1
	cfg.MaxRetryInterval = 0
	cfg.DisableCooling = true
	cfg.Streaming.BootstrapRetries = 0
	cfg.Streaming.KeepAliveSeconds = 0
	cfg.NonStreamKeepAliveInterval = 0
	cfg.QuotaExceeded.SwitchProject = false
	cfg.QuotaExceeded.SwitchPreviewModel = false
	cfg.QuotaExceeded.AntigravityCredits = false
	cfg.Routing.Strategy = "round-robin"
	cfg.Routing.SessionAffinity = false
	cfg.Routing.ClaudeCodeSessionAffinity = false
	cfg.WebsocketAuth = true
	cfg.EnableGeminiCLIEndpoint = false
	// Upstream response headers such as Retry-After reach the caller.
	cfg.PassthroughHeaders = true
	return cfg
}

// noFileTokenStore replaces the SDK's global file token store
// (sdk/auth/store_registry.go). It keeps nothing: provider credentials live
// in the mcpa database and each resource's auth manager persists through
// its own resourceStore.
type noFileTokenStore struct{}

func (noFileTokenStore) List(context.Context) ([]*coreauth.Auth, error) { return nil, nil }

func (noFileTokenStore) Save(context.Context, *coreauth.Auth) (string, error) {
	return "", errors.New("mcpa: the SDK file token store is disabled")
}

func (noFileTokenStore) Delete(context.Context, string) error { return nil }

var (
	harvestOnce      sync.Once
	harvestedErr     error
	harvested        map[string]coreauth.ProviderExecutor
	harvestedConfig  *config.Config
	sdkLoggingConfig sync.Once
)

// harvestExecutors obtains the SDK's built-in provider executors.
//
// Workaround: the executors live in the SDK's internal/runtime/executor
// package and are only constructed by cliproxy.Service
// (sdk/cliproxy/service.go:376-434, ensureExecutorsForAuthWithMode). The
// only exported path that reaches that code without the file watcher is
// Service.Run's reload callback, which calls rebindExecutors for every auth
// in the core manager (service.go:448 and 719). harvestExecutors therefore
// runs one bootstrap Service per process:
//
//   - the core manager holds one synthetic, credential-free auth per provider
//     so rebindExecutors constructs each executor;
//   - the HTTP server is configured with port -1 so net.Listen fails and the
//     SDK never accepts a connection (internal/api/server.go:810);
//   - the watcher factory returns a no-op watcher and invokes the reload
//     callback once, which registers the executors;
//   - Run then returns the listen error and its deferred Shutdown stops the
//     bootstrap's refresh loop and the SDK usage dispatcher, so later usage
//     records are discarded (sdk/cliproxy/usage/manager.go Publish).
//
// The executors are stateless with respect to auths (each call receives the
// selected auth) and are shared by every resource's own manager. Remove this
// workaround when the SDK exports executor constructors.
func harvestExecutors() (map[string]coreauth.ProviderExecutor, *config.Config, error) {
	harvestOnce.Do(func() {
		harvested, harvestedConfig, harvestedErr = runHarvest()
	})
	return harvested, harvestedConfig, harvestedErr
}

func runHarvest() (map[string]coreauth.ProviderExecutor, *config.Config, error) {
	configureSDKGlobals()
	cfg := newSDKConfig()
	bootstrap := coreauth.NewManager(nil, nil, nil)
	ctx := coreauth.WithSkipPersist(context.Background())
	now := time.Now()
	for _, provider := range sdkProviderKeys {
		auth := &coreauth.Auth{
			ID:         "mcpa-bootstrap-" + provider,
			Provider:   provider,
			Status:     coreauth.StatusActive,
			Attributes: map[string]string{"runtime_only": "true"},
			CreatedAt:  now,
			UpdatedAt:  now,
		}
		if provider == sdkProviderOpenAICompat {
			auth.Attributes["compat_name"] = sdkProviderOpenAICompat
			auth.Attributes["provider_key"] = sdkProviderOpenAICompat
		}
		if _, err := bootstrap.Register(ctx, auth); err != nil {
			return nil, nil, fmt.Errorf("register bootstrap auth %s: %w", provider, err)
		}
	}

	reloaded := false
	service, err := cliproxy.NewBuilder().
		WithConfig(cfg).
		WithConfigPath("mcpa-embedded-sdk").
		WithCoreAuthManager(bootstrap).
		WithTokenClientProvider(noopTokenClients{}).
		WithAPIKeyClientProvider(noopAPIKeyClients{}).
		WithWatcherFactory(func(_, _ string, reload func(*config.Config)) (*cliproxy.WatcherWrapper, error) {
			reload(nil)
			reloaded = true
			return &cliproxy.WatcherWrapper{}, nil
		}).
		WithServerOptions(sdkapi.WithRequestLoggerFactory(func(*config.Config, string) logging.RequestLogger { return nil })).
		Build()
	if err != nil {
		return nil, nil, fmt.Errorf("build sdk bootstrap service: %w", err)
	}
	runCtx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	runErr := service.Run(runCtx)
	if !reloaded {
		return nil, nil, fmt.Errorf("sdk bootstrap did not register executors: %v", runErr)
	}
	for _, auth := range bootstrap.List() {
		cliproxy.GlobalModelRegistry().UnregisterClient(auth.ID)
	}
	executors := make(map[string]coreauth.ProviderExecutor, len(sdkProviderKeys))
	for _, provider := range sdkProviderKeys {
		executor, ok := bootstrap.Executor(provider)
		if !ok {
			return nil, nil, fmt.Errorf("sdk bootstrap did not register the %s executor", provider)
		}
		executors[provider] = executor
	}
	return executors, cfg, nil
}

type noopTokenClients struct{}

func (noopTokenClients) Load(context.Context, *config.Config) (*cliproxy.TokenClientResult, error) {
	return &cliproxy.TokenClientResult{}, nil
}

type noopAPIKeyClients struct{}

func (noopAPIKeyClients) Load(context.Context, *config.Config) (*cliproxy.APIKeyClientResult, error) {
	return &cliproxy.APIKeyClientResult{}, nil
}

// configureSDKGlobals applies process-wide SDK settings once.
func configureSDKGlobals() {
	sdkLoggingConfig.Do(func() {
		gin.SetMode(gin.ReleaseMode)
		sdkauth.RegisterTokenStore(noFileTokenStore{})
		coreauth.SetQuotaCooldownDisabled(true)
		// The SDK logs through logrus. Only warnings and errors are forwarded;
		// debug output of the SDK can include OAuth code prefixes.
		logrus.SetLevel(logrus.WarnLevel)
		logrus.SetOutput(io.Discard)
		logrus.AddHook(slogBridge{logger: slog.Default().With(slog.String("component", "cliproxy-sdk"))})
	})
}

// slogBridge forwards SDK logrus entries to slog without their fields, which
// may carry request metadata.
type slogBridge struct {
	logger *slog.Logger
}

func (slogBridge) Levels() []logrus.Level {
	return []logrus.Level{logrus.PanicLevel, logrus.FatalLevel, logrus.ErrorLevel, logrus.WarnLevel}
}

func (b slogBridge) Fire(entry *logrus.Entry) error {
	level := slog.LevelWarn
	if entry.Level <= logrus.ErrorLevel {
		level = slog.LevelError
	}
	b.logger.Log(context.Background(), level, strings.TrimSpace(entry.Message))
	return nil
}
