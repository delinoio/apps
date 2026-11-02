// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"time"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/proxy/sdkmodels"
	"github.com/router-for-me/CLIProxyAPI/v6/sdk/cliproxy"
	coreauth "github.com/router-for-me/CLIProxyAPI/v6/sdk/cliproxy/auth"
)

// SDK provider keys. They match the executor identifiers that
// cliproxy.Service registers in sdk/cliproxy/service.go
// (ensureExecutorsForAuthWithMode) for SDK v6.10.9.
const (
	sdkProviderGemini       = "gemini"
	sdkProviderVertex       = "vertex"
	sdkProviderGeminiCLI    = "gemini-cli"
	sdkProviderAntigravity  = "antigravity"
	sdkProviderClaude       = "claude"
	sdkProviderCodex        = "codex"
	sdkProviderKimi         = "kimi"
	sdkProviderOpenAICompat = "openai-compatibility"
)

// Auth attribute keys owned by mcpa. The SDK copies attributes with each
// auth clone, so executors receive them with every selected auth.
const (
	attrResourceID   = "mcpa_resource_id"
	attrCredentialID = "mcpa_credential_id"
)

// sdkProviderKeys lists every executor the engine harvests from the SDK.
var sdkProviderKeys = []string{
	sdkProviderGemini,
	sdkProviderVertex,
	sdkProviderGeminiCLI,
	sdkProviderAntigravity,
	sdkProviderClaude,
	sdkProviderCodex,
	sdkProviderKimi,
	sdkProviderOpenAICompat,
}

// providerLabel returns the bounded metric label for a provider.
func providerLabel(provider mcpav1.Provider) string {
	switch provider {
	case mcpav1.Provider_PROVIDER_GEMINI:
		return "gemini"
	case mcpav1.Provider_PROVIDER_GEMINI_CLI:
		return "gemini_cli"
	case mcpav1.Provider_PROVIDER_VERTEX:
		return "vertex"
	case mcpav1.Provider_PROVIDER_CLAUDE:
		return "claude"
	case mcpav1.Provider_PROVIDER_CODEX:
		return "codex"
	case mcpav1.Provider_PROVIDER_ANTIGRAVITY:
		return "antigravity"
	case mcpav1.Provider_PROVIDER_KIMI:
		return "kimi"
	case mcpav1.Provider_PROVIDER_OPENAI_COMPATIBLE:
		return "openai_compatible"
	default:
		return "unspecified"
	}
}

// SupportsCredential reports whether the embedded SDK version can execute
// calls with the provider and credential kind.
func SupportsCredential(provider mcpav1.Provider, kind mcpav1.CredentialKind) bool {
	switch kind {
	case mcpav1.CredentialKind_CREDENTIAL_KIND_API_KEY:
		switch provider {
		case mcpav1.Provider_PROVIDER_GEMINI,
			mcpav1.Provider_PROVIDER_VERTEX,
			mcpav1.Provider_PROVIDER_CLAUDE,
			mcpav1.Provider_PROVIDER_CODEX,
			mcpav1.Provider_PROVIDER_KIMI,
			mcpav1.Provider_PROVIDER_OPENAI_COMPATIBLE:
			return true
		}
	case mcpav1.CredentialKind_CREDENTIAL_KIND_OAUTH:
		switch provider {
		case mcpav1.Provider_PROVIDER_GEMINI_CLI,
			mcpav1.Provider_PROVIDER_CLAUDE,
			mcpav1.Provider_PROVIDER_CODEX,
			mcpav1.Provider_PROVIDER_ANTIGRAVITY,
			mcpav1.Provider_PROVIDER_KIMI:
			return true
		}
	}
	return false
}

// OAuthPayloadType returns the SDK token "type" value for an OAuth provider.
func OAuthPayloadType(provider mcpav1.Provider) (string, bool) {
	switch provider {
	case mcpav1.Provider_PROVIDER_GEMINI_CLI:
		// The SDK token file uses "gemini"; the file synthesizer maps it to the
		// "gemini-cli" executor (internal/watcher/synthesizer/file.go).
		return "gemini", true
	case mcpav1.Provider_PROVIDER_CLAUDE:
		return "claude", true
	case mcpav1.Provider_PROVIDER_CODEX:
		return "codex", true
	case mcpav1.Provider_PROVIDER_ANTIGRAVITY:
		return "antigravity", true
	case mcpav1.Provider_PROVIDER_KIMI:
		return "kimi", true
	default:
		return "", false
	}
}

// builtAuth is the SDK form of one credential.
type builtAuth struct {
	auth        *coreauth.Auth
	sdkProvider string
	models      []*cliproxy.ModelInfo
}

// buildAuth converts a credential into an SDK auth entry, mirroring the
// mapping of the SDK's config and file synthesizers
// (internal/watcher/synthesizer/config.go and file.go) without files.
func buildAuth(cred Credential, now time.Time) (*builtAuth, error) {
	if strings.TrimSpace(cred.ID) == "" || strings.TrimSpace(cred.ResourceID) == "" {
		return nil, errors.New("credential id and resource id are required")
	}
	if !SupportsCredential(cred.Provider, cred.Kind) {
		return nil, fmt.Errorf("provider %s does not support credential kind %s", cred.Provider, cred.Kind)
	}
	auth := &coreauth.Auth{
		ID:        cred.ID,
		Label:     cred.Label,
		Status:    coreauth.StatusActive,
		CreatedAt: now,
		UpdatedAt: now,
		Attributes: map[string]string{
			attrResourceID:   cred.ResourceID,
			attrCredentialID: cred.ID,
		},
	}
	baseURL := strings.TrimSpace(cred.BaseURL)
	planType := ""

	switch cred.Kind {
	case mcpav1.CredentialKind_CREDENTIAL_KIND_API_KEY:
		secret := strings.TrimSpace(string(cred.Secret))
		if secret == "" {
			return nil, errors.New("api key is empty")
		}
		switch cred.Provider {
		case mcpav1.Provider_PROVIDER_GEMINI:
			auth.Provider = sdkProviderGemini
		case mcpav1.Provider_PROVIDER_VERTEX:
			auth.Provider = sdkProviderVertex
			auth.Attributes["provider_key"] = sdkProviderVertex
			if strings.HasPrefix(secret, "{") {
				metadata, err := parseVertexServiceAccount(cred.Secret)
				if err != nil {
					return nil, err
				}
				auth.Metadata = metadata
				secret = ""
			}
		case mcpav1.Provider_PROVIDER_CLAUDE:
			auth.Provider = sdkProviderClaude
		case mcpav1.Provider_PROVIDER_CODEX:
			auth.Provider = sdkProviderCodex
		case mcpav1.Provider_PROVIDER_KIMI:
			auth.Provider = sdkProviderKimi
		case mcpav1.Provider_PROVIDER_OPENAI_COMPATIBLE:
			if baseURL == "" {
				return nil, errors.New("base url is required for openai-compatible credentials")
			}
			if len(cred.Models) == 0 {
				return nil, errors.New("models are required for openai-compatible credentials")
			}
			// Every OpenAI-compatible credential shares one executor key. The
			// resource's own auth manager holds only that resource's
			// credentials, so the shared key cannot route across resources.
			auth.Provider = sdkProviderOpenAICompat
			auth.Attributes["compat_name"] = sdkProviderOpenAICompat
			auth.Attributes["provider_key"] = sdkProviderOpenAICompat
		}
		if secret != "" {
			auth.Attributes["api_key"] = secret
		}
		if baseURL != "" {
			auth.Attributes["base_url"] = baseURL
		}
	case mcpav1.CredentialKind_CREDENTIAL_KIND_OAUTH:
		metadata := map[string]any{}
		if err := json.Unmarshal(cred.Secret, &metadata); err != nil {
			return nil, errors.New("oauth payload is not a JSON object")
		}
		wantType, _ := OAuthPayloadType(cred.Provider)
		if gotType, _ := metadata["type"].(string); !strings.EqualFold(gotType, wantType) {
			return nil, fmt.Errorf("oauth payload type does not match provider %s", cred.Provider)
		}
		switch cred.Provider {
		case mcpav1.Provider_PROVIDER_GEMINI_CLI:
			auth.Provider = sdkProviderGeminiCLI
			if project, _ := metadata["project_id"].(string); strings.Contains(project, ",") {
				// Multi-project Gemini CLI credentials need the SDK's internal
				// virtual-auth runtime, which is not exported.
				return nil, errors.New("gemini cli credentials with multiple projects are not supported")
			}
		case mcpav1.Provider_PROVIDER_CLAUDE:
			auth.Provider = sdkProviderClaude
		case mcpav1.Provider_PROVIDER_CODEX:
			auth.Provider = sdkProviderCodex
			planType = codexPlanType(metadata)
			if planType != "" {
				auth.Attributes["plan_type"] = planType
			}
		case mcpav1.Provider_PROVIDER_ANTIGRAVITY:
			auth.Provider = sdkProviderAntigravity
		case mcpav1.Provider_PROVIDER_KIMI:
			auth.Provider = sdkProviderKimi
		}
		if email, _ := metadata["email"].(string); email != "" {
			auth.Attributes["email"] = email
			if auth.Label == "" {
				auth.Label = email
			}
		}
		auth.Metadata = metadata
		coreauth.ApplyCustomHeadersFromMetadata(auth)
	}

	models, err := credentialModels(cred, planType)
	if err != nil {
		return nil, err
	}
	return &builtAuth{auth: auth, sdkProvider: auth.Provider, models: models}, nil
}

func parseVertexServiceAccount(secret []byte) (map[string]any, error) {
	serviceAccount := map[string]any{}
	if err := json.Unmarshal(secret, &serviceAccount); err != nil {
		return nil, errors.New("vertex service account is not valid JSON")
	}
	if kind, _ := serviceAccount["type"].(string); kind != "service_account" {
		return nil, errors.New("vertex JSON credential must be a service account key")
	}
	projectID, _ := serviceAccount["project_id"].(string)
	metadata := map[string]any{
		"type":            "vertex",
		"service_account": serviceAccount,
		"project_id":      projectID,
	}
	if location, _ := serviceAccount["location"].(string); location != "" {
		metadata["location"] = location
	}
	return metadata, nil
}

// codexPlanType reads the ChatGPT plan from the unverified id_token claims,
// as the SDK's file synthesizer does. The token came from the provider's
// token endpoint; mcpa only uses the plan to choose a default model list.
func codexPlanType(metadata map[string]any) string {
	idToken, _ := metadata["id_token"].(string)
	parts := strings.Split(idToken, ".")
	if len(parts) < 2 {
		return ""
	}
	payload, err := base64.RawURLEncoding.DecodeString(parts[1])
	if err != nil {
		return ""
	}
	var claims struct {
		Auth struct {
			PlanType string `json:"chatgpt_plan_type"`
		} `json:"https://api.openai.com/auth"`
	}
	if json.Unmarshal(payload, &claims) != nil {
		return ""
	}
	return strings.ToLower(strings.TrimSpace(claims.Auth.PlanType))
}

// credentialModels returns the models a credential serves.
func credentialModels(cred Credential, codexPlan string) ([]*cliproxy.ModelInfo, error) {
	if len(cred.Models) > 0 {
		out := make([]*cliproxy.ModelInfo, 0, len(cred.Models))
		seen := map[string]struct{}{}
		for _, name := range cred.Models {
			name = strings.TrimSpace(name)
			if name == "" {
				continue
			}
			if _, dup := seen[name]; dup {
				continue
			}
			seen[name] = struct{}{}
			out = append(out, &cliproxy.ModelInfo{
				ID:          name,
				Object:      "model",
				OwnedBy:     providerLabel(cred.Provider),
				Type:        providerLabel(cred.Provider),
				DisplayName: name,
				UserDefined: true,
			})
		}
		if len(out) == 0 {
			return nil, errors.New("credential declares no usable models")
		}
		return out, nil
	}
	var channel sdkmodels.Channel
	switch cred.Provider {
	case mcpav1.Provider_PROVIDER_GEMINI:
		channel = sdkmodels.ChannelGemini
	case mcpav1.Provider_PROVIDER_VERTEX:
		channel = sdkmodels.ChannelVertex
	case mcpav1.Provider_PROVIDER_GEMINI_CLI:
		channel = sdkmodels.ChannelGeminiCLI
	case mcpav1.Provider_PROVIDER_CLAUDE:
		channel = sdkmodels.ChannelClaude
	case mcpav1.Provider_PROVIDER_ANTIGRAVITY:
		channel = sdkmodels.ChannelAntigravity
	case mcpav1.Provider_PROVIDER_KIMI:
		channel = sdkmodels.ChannelKimi
	case mcpav1.Provider_PROVIDER_CODEX:
		// Same plan mapping as Service.registerModelsForAuth.
		switch codexPlan {
		case "plus":
			channel = sdkmodels.ChannelCodexPlus
		case "team", "business", "go":
			channel = sdkmodels.ChannelCodexTeam
		case "free":
			channel = sdkmodels.ChannelCodexFree
		default:
			channel = sdkmodels.ChannelCodexPro
		}
	default:
		return nil, fmt.Errorf("provider %s has no default model catalog", cred.Provider)
	}
	return sdkmodels.Models(channel)
}

// baseModelName strips an SDK thinking suffix such as "model(8192)".
func baseModelName(model string) string {
	model = strings.TrimSpace(model)
	if strings.HasSuffix(model, ")") {
		if open := strings.LastIndex(model, "("); open > 0 {
			return model[:open]
		}
	}
	return model
}
