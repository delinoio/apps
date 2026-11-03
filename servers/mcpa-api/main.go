// SPDX-License-Identifier: FSL-1.1-ALv2

// Command mcpa-api is the Managed CPA API server.
//
// Subcommands:
//
//	mcpa-api serve [--dev-env]   serve all HTTP surfaces and run the workers
//	mcpa-api migrate             apply the embedded SQL migrations
//	mcpa-api check-schema        exit non-zero when the schema differs
//	mcpa-api rotate-master-key   rewrap stored data keys under the active key
//
// --dev-env loads .env from the working directory when present; process
// variables, including empty ones, win. Every subcommand accepts it so local
// migrations can use the same file as serve.
package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"log/slog"
	"os"
	"os/signal"
	"syscall"

	"github.com/delinoio/apps/servers/mcpa-api/internal/app"
	"github.com/delinoio/apps/servers/mcpa-api/internal/config"
	"github.com/delinoio/apps/servers/mcpa-api/internal/crypto"
	"github.com/delinoio/apps/servers/mcpa-api/internal/dataplane"
	"github.com/delinoio/apps/servers/mcpa-api/internal/mcp"
	"github.com/delinoio/apps/servers/mcpa-api/internal/oauth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/obs"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

// Command is a subcommand name.
type Command string

const (
	CommandServe           Command = "serve"
	CommandMigrate         Command = "migrate"
	CommandCheckSchema     Command = "check-schema"
	CommandRotateMasterKey Command = "rotate-master-key"
)

// devEnvFile is opened by exact path in the working directory and never
// searched upward, matching the Rust server development loader. The
// development launcher runs from servers/mcpa-api.
const devEnvFile = ".env"

// rotationBatchSize is the number of rows rewrapped per transaction.
const rotationBatchSize = 100

const usage = `usage: mcpa-api <command> [--dev-env]

commands:
  serve               serve all HTTP surfaces and run the background workers
  migrate             apply the embedded SQL migrations
  check-schema        exit non-zero when the database schema differs
  rotate-master-key   rewrap every stored data key under the active master key
`

func main() {
	os.Exit(run(os.Args[1:], os.Stderr))
}

func run(args []string, stderr io.Writer) int {
	if len(args) == 0 {
		fmt.Fprint(stderr, usage)
		return 2
	}
	cmd := Command(args[0])
	fs := flag.NewFlagSet("mcpa-api "+args[0], flag.ContinueOnError)
	fs.SetOutput(stderr)
	devEnv := fs.Bool("dev-env", false, "load .env from the working directory; process variables win")
	if err := fs.Parse(args[1:]); err != nil {
		return 2
	}
	if fs.NArg() > 0 {
		fmt.Fprintf(stderr, "mcpa-api %s: unexpected argument %q\n", cmd, fs.Arg(0))
		return 2
	}

	lookup := config.Lookup(os.LookupEnv)
	if *devEnv {
		values, err := config.LoadDotEnv(devEnvFile)
		if err != nil {
			fmt.Fprintf(stderr, "mcpa-api: read %s: %v\n", devEnvFile, err)
			return 1
		}
		lookup = config.WithFallback(lookup, values)
	}
	log := obs.NewLogger(stderr, config.LoadLogging(lookup))
	slog.SetDefault(log)

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	var err error
	switch cmd {
	case CommandServe:
		err = serve(ctx, lookup, log)
	case CommandMigrate:
		err = migrate(ctx, lookup, log)
	case CommandCheckSchema:
		err = checkSchema(ctx, lookup, log)
	case CommandRotateMasterKey:
		err = rotateMasterKey(ctx, lookup, log)
	default:
		fmt.Fprintf(stderr, "mcpa-api: unknown command %q\n\n%s", cmd, usage)
		return 2
	}
	if err != nil {
		var cfgErr config.Errors
		if errors.As(err, &cfgErr) {
			// Each entry names the setting and never contains its value.
			for _, e := range cfgErr {
				log.Error("invalid configuration", "command", string(cmd), "problem", e)
			}
		} else {
			log.Error("command failed", "command", string(cmd), "error", err)
		}
		return 1
	}
	return 0
}

func serve(ctx context.Context, lookup config.Lookup, log *slog.Logger) error {
	cfg, err := config.LoadServe(lookup)
	if err != nil {
		return err
	}
	shutdownTracing, err := obs.SetupTracing(ctx, cfg.TracingEnabled)
	if err != nil {
		return fmt.Errorf("tracing: %w", err)
	}
	defer func() {
		flushCtx, cancel := context.WithTimeout(context.WithoutCancel(ctx), app.ShutdownTimeout)
		defer cancel()
		if err := shutdownTracing(flushCtx); err != nil {
			log.Warn("trace flush failed", "error", err)
		}
	}()

	a, err := app.New(ctx, cfg, log, obs.NewMetrics())
	if err != nil {
		return err
	}
	defer a.Close()
	if _, err := dataplane.Install(a, dataplane.InstallOptions{}); err != nil {
		return err
	}
	if err := installMCP(a); err != nil {
		return err
	}
	log.Info("mcpa-api starting",
		"public_url", cfg.PublicURL,
		"web_origin", cfg.WebOrigin,
		"log_format", cfg.Format.String(),
		"tracing", cfg.TracingEnabled,
		"metrics_token", cfg.MetricsToken != "",
		"active_key_id", cfg.ActiveKeyID,
		"oauth_clients", len(cfg.OAuthClients),
		"checkpoint_interval", cfg.CheckpointInterval.String(),
		"outbox_interval", cfg.OutboxInterval.String(),
	)
	return a.Run(ctx)
}

// installMCP mounts the OAuth broker and /mcp and registers the broker
// cleanup worker. MCP-audience tokens are accepted on /mcp and Connect once
// the broker resolves them to grants.
func installMCP(a *app.App) error {
	cfg := a.Config
	broker := oauth.New(oauth.Config{
		Store:             store.NewMCPOAuth(a.Store),
		Keys:              a.Keys,
		Logto:             a.Logto,
		Delcore:           a.Delcore,
		Groups:            a.Authorizer,
		Clients:           cfg.OAuthClients,
		PublicURL:         cfg.PublicURL,
		WebOrigin:         cfg.WebOrigin,
		MCPResource:       cfg.MCPResource,
		DelcoreResource:   cfg.DelcoreResource,
		MFAClaim:          cfg.MFAClaim,
		ReauthURL:         cfg.MFAReauthURL,
		InsecureCookies:   cfg.InsecureCookies(),
		Log:               a.Log,
		DependencyTimeout: cfg.DependencyTimeout,
	})
	a.Authenticator.SetGrantResolver(broker)
	mcpMount, err := mcp.Mount(mcp.Config{
		Service:             a.Service,
		Authenticator:       a.Authenticator,
		ResourceMetadataURL: oauth.ResourceMetadataURL(cfg.PublicURL),
		WebOrigin:           cfg.WebOrigin,
		Log:                 a.Log,
	})
	if err != nil {
		return err
	}
	a.Mount(broker.Mounts()...)
	a.Mount(mcpMount)
	a.AddWorker(app.Worker{Name: "mcp-oauth-cleanup", Run: broker.RunCleanup})
	return nil
}

func migrate(ctx context.Context, lookup config.Lookup, log *slog.Logger) error {
	cfg, err := config.LoadDatabase(lookup)
	if err != nil {
		return err
	}
	db, err := store.Open(ctx, cfg.URL, cfg.DependencyTimeout, store.ModeMaintenance)
	if err != nil {
		return err
	}
	defer db.Close()
	applied, err := db.Migrate(ctx)
	if err != nil {
		return err
	}
	version, err := store.ExpectedVersion()
	if err != nil {
		return err
	}
	log.Info("migrations applied", "applied", applied, "schema_version", version)
	return nil
}

func checkSchema(ctx context.Context, lookup config.Lookup, log *slog.Logger) error {
	cfg, err := config.LoadDatabase(lookup)
	if err != nil {
		return err
	}
	db, err := store.Open(ctx, cfg.URL, cfg.DependencyTimeout, store.ModeMaintenance)
	if err != nil {
		return err
	}
	defer db.Close()
	if err := db.CheckSchema(ctx); err != nil {
		return err
	}
	version, err := store.ExpectedVersion()
	if err != nil {
		return err
	}
	log.Info("schema is current", "schema_version", version)
	return nil
}

func rotateMasterKey(ctx context.Context, lookup config.Lookup, log *slog.Logger) error {
	dbCfg, keyCfg, err := config.LoadRotation(lookup)
	if err != nil {
		return err
	}
	keys, err := crypto.NewKeyring(keyCfg.MasterKeys, keyCfg.ActiveKeyID)
	if err != nil {
		return fmt.Errorf("master keys: %w", err)
	}
	db, err := store.Open(ctx, dbCfg.URL, dbCfg.DependencyTimeout, store.ModeMaintenance)
	if err != nil {
		return err
	}
	defer db.Close()
	// Rotation reads and rewrites the sealed tables, so it must match the
	// binary's schema like serve does.
	if err := db.CheckSchema(ctx); err != nil {
		return err
	}
	report, err := store.RotateMasterKey(ctx, db, keys, rotationBatchSize, log)
	if err != nil {
		return err
	}
	if len(report.Remaining) > 0 {
		log.Error("rows still reference inactive master keys; keep every listed key configured and rerun rotate-master-key",
			"alert", true, "active_key_id", keys.ActiveKeyID(), "remaining", report.Remaining)
		return errors.New("master-key rotation incomplete")
	}
	log.Info("master-key rotation complete", "active_key_id", keys.ActiveKeyID(), "rewrapped", report.Rewrapped)
	return nil
}
