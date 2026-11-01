// SPDX-License-Identifier: FSL-1.1-ALv2

package dataplane

import (
	"context"
	"net/http"
	"time"

	"github.com/delinoio/apps/servers/mcpa-api/internal/app"
	"github.com/delinoio/apps/servers/mcpa-api/internal/obs"
	"github.com/delinoio/apps/servers/mcpa-api/internal/provideroauth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/proxy"
	"github.com/delinoio/apps/servers/mcpa-api/internal/server"
	"github.com/delinoio/apps/servers/mcpa-api/internal/service"
)

// InstallOptions overrides defaults of Install. Production uses the zero
// value; tests inject transports, provider endpoints and intervals.
type InstallOptions struct {
	// BaseTransport performs provider requests of model calls.
	BaseTransport http.RoundTripper
	// OAuth overrides provider OAuth settings. Store, Keys, Listener and Log
	// are always set by Install.
	OAuth provideroauth.Options
	// RetentionInterval and LifecycleInterval default to one hour.
	RetentionInterval time.Duration
	LifecycleInterval time.Duration
	Now               func() time.Time
}

// Install builds the data plane for a, mounts the engine at /r/, installs
// the service hooks and registers the workers: outbox delivery, retention,
// tenant lifecycle, the cross-replica credential listener and engine
// shutdown.
func Install(a *app.App, opts InstallOptions) (*DataPlane, error) {
	dp, err := New(Config{
		Store: a.Store, Keys: a.Keys, Delcore: a.Delcore, Metrics: a.Metrics, Log: a.Log,
		CheckpointInterval: a.Config.CheckpointInterval, OutboxInterval: a.Config.OutboxInterval,
		DependencyTimeout: a.Config.DependencyTimeout, RetentionInterval: opts.RetentionInterval,
		LifecycleInterval: opts.LifecycleInterval, BaseTransport: opts.BaseTransport, Now: opts.Now,
	})
	if err != nil {
		return nil, err
	}
	oauth := opts.OAuth
	oauth.Store, oauth.Keys, oauth.Listener, oauth.Log = a.Store, a.Keys, dp.Sync, a.Log
	if oauth.GeminiCLIClient == (provideroauth.GoogleOAuthClient{}) {
		oauth.GeminiCLIClient = provideroauth.GoogleOAuthClient{ClientID: a.Config.GeminiCLIOAuthClientID, ClientSecret: a.Config.GeminiCLIOAuthClientSecret}
	}
	if oauth.AntigravityClient == (provideroauth.GoogleOAuthClient{}) {
		oauth.AntigravityClient = provideroauth.GoogleOAuthClient{ClientID: a.Config.AntigravityOAuthClientID, ClientSecret: a.Config.AntigravityOAuthClientSecret}
	}
	if oauth.Now == nil {
		oauth.Now = opts.Now
	}
	if oauth.Timeout <= 0 {
		oauth.Timeout = max(a.Config.DependencyTimeout, 10*time.Second)
	}
	a.Service.SetHooks(service.Hooks{
		OAuth:       provideroauth.New(oauth),
		Usage:       dp.Usage,
		Spend:       dp.Usage,
		Credentials: dp.Sync,
	})
	a.Mount(server.Mount{Pattern: proxy.ResourcePathPrefix, Handler: dp.Engine, Surface: obs.SurfaceProxy, SelfMetered: true})
	a.AddWorker(
		app.Worker{Name: "usage_outbox", Run: dp.Outbox.Run},
		app.Worker{Name: "retention", Run: dp.RunRetention},
		app.Worker{Name: "tenant_lifecycle", Run: dp.RunLifecycle},
		app.Worker{Name: "credential_sync", Run: dp.Sync.Listen},
		app.Worker{Name: "data_plane_engine", Run: func(ctx context.Context) error {
			<-ctx.Done()
			return dp.Close()
		}},
	)
	return dp, nil
}
