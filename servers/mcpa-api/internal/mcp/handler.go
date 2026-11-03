// SPDX-License-Identifier: FSL-1.1-ALv2

// Package mcp serves Streamable HTTP MCP at /mcp.
//
// Every McpaService RPC is one tool with a snake_case name. Tool input and
// output are the ProtoJSON forms of the request and response messages, with
// proto field names. Tools call the same service handlers as Connect, so
// authorization, audit and validation are identical; the actor kind is
// ACTOR_KIND_MCP for broker tokens and ACTOR_KIND_PERSONAL_KEY for Delcore
// personal keys.
//
// /mcp accepts Authorization: Bearer with a Logto access token for the
// MCP-only audience, issued through the OAuth broker and carrying MFA
// evidence, or a Delcore personal key (dlk_). The server is stateless: every
// HTTP request carries its credential and no MCP session state is kept.
package mcp

import (
	"context"
	"encoding/json"
	"fmt"
	"log/slog"
	"net/http"
	"time"

	sdkauth "github.com/modelcontextprotocol/go-sdk/auth"
	gomcp "github.com/modelcontextprotocol/go-sdk/mcp"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/protos/mcpa/v1/mcpav1connect"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/obs"
	"github.com/delinoio/apps/servers/mcpa-api/internal/server"
)

// Path is the MCP endpoint.
const Path = "/mcp"

// Authenticator resolves bearer credentials. *auth.Authenticator implements
// it.
type Authenticator interface {
	Bearer(ctx context.Context, token string) (*auth.Principal, error)
}

// Config configures the MCP handler.
type Config struct {
	Service       mcpav1connect.McpaServiceHandler
	Authenticator Authenticator
	// ResourceMetadataURL is announced in WWW-Authenticate challenges.
	ResourceMetadataURL string
	// WebOrigin is the only browser origin admitted besides none.
	WebOrigin string
	Log       *slog.Logger
}

// NewServer builds the MCP server with one tool per McpaService RPC.
func NewServer(svc mcpav1connect.McpaServiceHandler, log *slog.Logger) (*gomcp.Server, error) {
	tools, handlers, err := Tools(svc, log)
	if err != nil {
		return nil, err
	}
	s := gomcp.NewServer(&gomcp.Implementation{
		Name:  "mcpa",
		Title: "Managed CPA",
		// The server is versioned by the mcpa.v1 protocol.
		Version: "v1",
	}, &gomcp.ServerOptions{
		Instructions: "Manage Managed CPA resources, provider credentials, call keys, routing, access control, budgets, usage, call records and audit events. " +
			"Tool arguments and results are the ProtoJSON forms of the mcpa.v1 messages with proto field names.",
	})
	for i, t := range tools {
		s.AddTool(t, handlers[i])
	}
	return s, nil
}

// Handler builds the /mcp HTTP handler.
func Handler(cfg Config) (http.Handler, error) {
	s, err := NewServer(cfg.Service, cfg.Log)
	if err != nil {
		return nil, err
	}
	streamable := gomcp.NewStreamableHTTPHandler(func(*http.Request) *gomcp.Server { return s }, &gomcp.StreamableHTTPOptions{
		Stateless: true,
		// Workaround: the SDK rejects requests that arrive on a loopback
		// address with a non-loopback Host, which breaks the Portless
		// development proxy and loopback reverse proxies in deployments. It
		// guards DNS rebinding against servers with ambient credentials;
		// /mcp requires a bearer credential and checks Origin itself. Remove
		// this when the SDK can be told about trusted proxies.
		DisableLocalhostProtection: true,
	})
	// The principal was resolved by authenticate; this verifier only hands it
	// to tool handlers through the SDK's TokenInfo.
	withTokenInfo := sdkauth.RequireBearerToken(func(ctx context.Context, _ string, _ *http.Request) (*sdkauth.TokenInfo, error) {
		p := auth.FromContext(ctx)
		if p == nil {
			return nil, sdkauth.ErrInvalidToken
		}
		return &sdkauth.TokenInfo{UserID: p.UserID, Expiration: time.Now().Add(time.Hour), Extra: map[string]any{principalKey: p}}, nil
	}, nil)
	h := &handler{cfg: cfg, next: withTokenInfo(streamable)}
	return h, nil
}

// Mount returns the /mcp route for app.Mount.
func Mount(cfg Config) (server.Mount, error) {
	h, err := Handler(cfg)
	if err != nil {
		return server.Mount{}, err
	}
	return server.Mount{Pattern: Path, Handler: h, Surface: obs.SurfaceMCP}, nil
}

type handler struct {
	cfg  Config
	next http.Handler
}

func (h *handler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	// A present Origin must be the console origin (DNS rebinding and
	// cross-site protection). No CORS headers are sent.
	if !auth.CheckOrigin(r.Header.Get("Origin"), h.cfg.WebOrigin) {
		writeJSON(w, http.StatusForbidden, map[string]string{"error": "origin_not_allowed"})
		return
	}
	header := r.Header.Get("Authorization")
	if header == "" {
		h.challenge(w, "", "")
		writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "unauthenticated"})
		return
	}
	token, ok := auth.BearerToken(header)
	if !ok {
		h.challenge(w, "invalid_request", "use the Bearer scheme")
		writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "unauthenticated"})
		return
	}
	p, err := h.cfg.Authenticator.Bearer(r.Context(), token)
	if err != nil {
		h.reject(w, r, err)
		return
	}
	h.next.ServeHTTP(w, r.WithContext(auth.WithPrincipal(r.Context(), p)))
}

// challenge sets the RFC 6750 and RFC 9728 WWW-Authenticate header.
func (h *handler) challenge(w http.ResponseWriter, code, description string) {
	v := "Bearer"
	sep := " "
	if code != "" {
		v += fmt.Sprintf(`%serror=%q`, sep, code)
		sep = ", "
		if description != "" {
			v += fmt.Sprintf(`%serror_description=%q`, sep, description)
		}
	}
	v += fmt.Sprintf(`%sresource_metadata=%q`, sep, h.cfg.ResourceMetadataURL)
	w.Header().Set("WWW-Authenticate", v)
}

func (h *handler) reject(w http.ResponseWriter, r *http.Request, err error) {
	e := apperr.As(err)
	attrs := []any{"reason", e.Reason.String()}
	switch e.Reason {
	case mcpav1.ErrorReason_ERROR_REASON_UNAUTHENTICATED:
		h.cfg.Log.InfoContext(r.Context(), "mcp request rejected", attrs...)
		h.challenge(w, "invalid_token", e.Message)
		writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "unauthenticated"})
	case mcpav1.ErrorReason_ERROR_REASON_MFA_REQUIRED:
		h.cfg.Log.InfoContext(r.Context(), "mcp request rejected", attrs...)
		// RFC 9470 step-up challenge; the body is the shared mfa_required shape.
		h.challenge(w, "insufficient_user_authentication", "multi-factor authentication is required")
		writeJSON(w, http.StatusForbidden, map[string]string{"error": "mfa_required", "reauthentication_url": e.ReauthURL})
	case mcpav1.ErrorReason_ERROR_REASON_TENANT_SERVICE_KEY_REJECTED:
		h.cfg.Log.InfoContext(r.Context(), "mcp request rejected", attrs...)
		writeJSON(w, http.StatusForbidden, map[string]string{"error": "tenant_service_key_rejected", "message": e.Message})
	case mcpav1.ErrorReason_ERROR_REASON_PERMISSION_DENIED:
		h.cfg.Log.InfoContext(r.Context(), "mcp request rejected", attrs...)
		writeJSON(w, http.StatusForbidden, map[string]string{"error": "forbidden"})
	case mcpav1.ErrorReason_ERROR_REASON_DEPENDENCY_UNAVAILABLE:
		if e.Cause != nil {
			attrs = append(attrs, "error", e.Cause.Error())
		}
		h.cfg.Log.WarnContext(r.Context(), "mcp authentication unavailable", attrs...)
		writeJSON(w, http.StatusServiceUnavailable, map[string]string{"error": "dependency_unavailable"})
	default:
		if e.Cause != nil {
			attrs = append(attrs, "error", e.Cause.Error())
		}
		h.cfg.Log.ErrorContext(r.Context(), "mcp authentication failed", attrs...)
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": "internal"})
	}
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}
