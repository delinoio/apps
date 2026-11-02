// SPDX-License-Identifier: FSL-1.1-ALv2

package provideroauth

// Provider OAuth constants
//
// The client IDs, client secrets, endpoints, scopes, redirect URIs, request
// shapes and token payload shapes below are copied from CLIProxyAPI v6.10.9
// (github.com/router-for-me/CLIProxyAPI/v6: internal/auth/claude,
// internal/auth/codex, internal/auth/gemini, internal/auth/antigravity,
// internal/auth/kimi, internal/cmd/login.go and sdk/auth), Copyright (c)
// 2025-2005.9 Luis Pater and Copyright (c) 2025.9-present Router-For.ME,
// used under the MIT License; see third-party/LICENSE.
//
// Workaround: the SDK's public sdk/auth login helpers bind localhost
// callback ports, print to stdout and block until the browser returns, so a
// multi-tenant server cannot call them. This package rebuilds the same flows
// as resumable, PostgreSQL-backed steps. The redirect URIs stay the
// providers' registered localhost URIs: the customer's browser fails to load
// them after consent and the customer pastes the final URL into
// CompleteProviderOAuth. Update these values together with the SDK pin in
// the root go.mod, and remove the copy when the SDK exports non-blocking
// flow primitives.

import (
	"strings"
)

// Claude Code OAuth (internal/auth/claude/anthropic_auth.go).
const (
	claudeAuthURL     = "https://claude.ai/oauth/authorize"
	claudeTokenURL    = "https://api.anthropic.com/v1/oauth/token"
	claudeClientID    = "9d1c250a-e61b-44d9-88ed-5944d1962f5e"
	claudeRedirectURI = "http://localhost:54545/callback"
	claudeScope       = "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload"
)

// Codex OAuth and device login (internal/auth/codex/openai_auth.go and
// sdk/auth/codex_device.go).
const (
	codexTokenURL                   = "https://auth.openai.com/oauth/token"
	codexClientID                   = "app_EMoamEEZ73f0CkXaXp7hrann"
	codexDeviceUserCodeURL          = "https://auth.openai.com/api/accounts/deviceauth/usercode"
	codexDeviceTokenURL             = "https://auth.openai.com/api/accounts/deviceauth/token"
	codexDeviceVerificationURL      = "https://auth.openai.com/codex/device"
	codexDeviceTokenExchangeURI     = "https://auth.openai.com/deviceauth/callback"
	codexDeviceDefaultPollInterval  = 5
	codexDeviceFlowLifetimeMinutes  = 15
	codexIDTokenAuthClaimsNamespace = "https://api.openai.com/auth"
)

// Gemini CLI OAuth (internal/auth/gemini/gemini_auth.go) and Code Assist
// project setup (internal/cmd/login.go).
//
// The Gemini CLI and Antigravity OAuth clients are Google installed-app
// clients. Their IDs and secrets are operator configuration
// (GoogleOAuthClient), never repository source.
const (
	geminiRedirectURI = "http://localhost:8085/oauth2callback"
	geminiUserInfoURL = "https://www.googleapis.com/oauth2/v1/userinfo?alt=json"
	geminiCodeAssist  = "https://cloudcode-pa.googleapis.com"
	geminiCLIVersion  = "0.34.0"
	googleAuthURL     = "https://accounts.google.com/o/oauth2/auth"
	googleTokenURL    = "https://oauth2.googleapis.com/token"
)

var geminiScopes = []string{
	"https://www.googleapis.com/auth/cloud-platform",
	"https://www.googleapis.com/auth/userinfo.email",
	"https://www.googleapis.com/auth/userinfo.profile",
}

// Antigravity OAuth (internal/auth/antigravity/constants.go).
const (
	antigravityRedirectURI = "http://localhost:51121/oauth-callback"
	antigravityAuthURL     = "https://accounts.google.com/o/oauth2/v2/auth"
	antigravityUserInfoURL = "https://www.googleapis.com/oauth2/v2/userinfo?alt=json"
)

var antigravityScopes = []string{
	"https://www.googleapis.com/auth/cloud-platform",
	"https://www.googleapis.com/auth/userinfo.email",
	"https://www.googleapis.com/auth/userinfo.profile",
	"https://www.googleapis.com/auth/cclog",
	"https://www.googleapis.com/auth/experimentsandconfigs",
}

// Kimi device login (internal/auth/kimi/kimi.go).
const (
	kimiClientID               = "17e5f671-d194-4dfb-9706-5516cb48c098"
	kimiDeviceAuthorizationURL = "https://auth.kimi.com/api/oauth/device_authorization"
	kimiTokenURL               = "https://auth.kimi.com/api/oauth/token"
	kimiDeviceGrantType        = "urn:ietf:params:oauth:grant-type:device_code"
	kimiDefaultPollInterval    = 5
	kimiMaxFlowMinutes         = 15
	// The SDK sends "cli-proxy-api" and the host name. mcpa identifies itself
	// and never discloses server host names.
	kimiPlatform    = "mcpa"
	kimiVersion     = "1.0.0"
	kimiDeviceName  = "Managed CPA"
	kimiDeviceModel = "mcpa-api"
)

// Endpoints are the provider URLs. Tests replace them with local servers.
type Endpoints struct {
	ClaudeAuthURL  string
	ClaudeTokenURL string

	CodexTokenURL              string
	CodexDeviceUserCodeURL     string
	CodexDeviceTokenURL        string
	CodexDeviceVerificationURL string

	GoogleAuthURL      string
	GoogleTokenURL     string
	GeminiUserInfoURL  string
	GeminiCodeAssist   string
	AntigravityAuthURL string
	// AntigravityUserInfoURL is used for the account email. The project
	// lookup uses sdkauth.FetchAntigravityProjectID, whose endpoint is fixed;
	// tests redirect it through the HTTP client.
	AntigravityUserInfoURL string

	KimiDeviceAuthorizationURL string
	KimiTokenURL               string
}

// DefaultEndpoints returns the production provider URLs of SDK v6.10.9.
func DefaultEndpoints() Endpoints {
	return Endpoints{
		ClaudeAuthURL:              claudeAuthURL,
		ClaudeTokenURL:             claudeTokenURL,
		CodexTokenURL:              codexTokenURL,
		CodexDeviceUserCodeURL:     codexDeviceUserCodeURL,
		CodexDeviceTokenURL:        codexDeviceTokenURL,
		CodexDeviceVerificationURL: codexDeviceVerificationURL,
		GoogleAuthURL:              googleAuthURL,
		GoogleTokenURL:             googleTokenURL,
		GeminiUserInfoURL:          geminiUserInfoURL,
		GeminiCodeAssist:           geminiCodeAssist,
		AntigravityAuthURL:         antigravityAuthURL,
		AntigravityUserInfoURL:     antigravityUserInfoURL,
		KimiDeviceAuthorizationURL: kimiDeviceAuthorizationURL,
		KimiTokenURL:               kimiTokenURL,
	}
}

func (e Endpoints) withDefaults() Endpoints {
	d := DefaultEndpoints()
	fill := func(v *string, def string) {
		if strings.TrimSpace(*v) == "" {
			*v = def
		}
	}
	fill(&e.ClaudeAuthURL, d.ClaudeAuthURL)
	fill(&e.ClaudeTokenURL, d.ClaudeTokenURL)
	fill(&e.CodexTokenURL, d.CodexTokenURL)
	fill(&e.CodexDeviceUserCodeURL, d.CodexDeviceUserCodeURL)
	fill(&e.CodexDeviceTokenURL, d.CodexDeviceTokenURL)
	fill(&e.CodexDeviceVerificationURL, d.CodexDeviceVerificationURL)
	fill(&e.GoogleAuthURL, d.GoogleAuthURL)
	fill(&e.GoogleTokenURL, d.GoogleTokenURL)
	fill(&e.GeminiUserInfoURL, d.GeminiUserInfoURL)
	fill(&e.GeminiCodeAssist, d.GeminiCodeAssist)
	fill(&e.AntigravityAuthURL, d.AntigravityAuthURL)
	fill(&e.AntigravityUserInfoURL, d.AntigravityUserInfoURL)
	fill(&e.KimiDeviceAuthorizationURL, d.KimiDeviceAuthorizationURL)
	fill(&e.KimiTokenURL, d.KimiTokenURL)
	return e
}
