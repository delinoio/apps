// SPDX-License-Identifier: FSL-1.1-ALv2

// Package sdkmodels exposes the static model catalog that ships with the
// embedded CLIProxyAPI SDK.
//
// third-party/models.json is a verbatim copy of
// internal/registry/models/models.json from
// github.com/router-for-me/CLIProxyAPI/v6 v6.10.9. It is MIT-licensed;
// third-party/LICENSE carries the upstream copyright and permission notice.
// The third-party directory is excluded from repository formatting so the
// copy stays byte-identical.
//
// Workaround: the SDK keeps this catalog in an internal package and only
// registers it through cliproxy.Service, which mcpa does not run per resource.
// The copy lets mcpa register the same default model lists for credentials
// that do not declare models. Replace the copy whenever the SDK version in the
// root go.mod changes, and remove it once the SDK exports the static catalog.
package sdkmodels

import (
	_ "embed"
	"encoding/json"
	"fmt"
	"sync"

	"github.com/router-for-me/CLIProxyAPI/v6/sdk/cliproxy"
)

//go:embed third-party/models.json
var raw []byte

// Channel identifies one section of the SDK catalog.
type Channel string

const (
	ChannelClaude      Channel = "claude"
	ChannelGemini      Channel = "gemini"
	ChannelVertex      Channel = "vertex"
	ChannelGeminiCLI   Channel = "gemini-cli"
	ChannelCodexFree   Channel = "codex-free"
	ChannelCodexTeam   Channel = "codex-team"
	ChannelCodexPlus   Channel = "codex-plus"
	ChannelCodexPro    Channel = "codex-pro"
	ChannelKimi        Channel = "kimi"
	ChannelAntigravity Channel = "antigravity"
)

var (
	loadOnce sync.Once
	catalog  map[Channel][]*cliproxy.ModelInfo
	loadErr  error
)

func load() {
	parsed := map[Channel][]*cliproxy.ModelInfo{}
	if err := json.Unmarshal(raw, &parsed); err != nil {
		loadErr = fmt.Errorf("sdkmodels: parse embedded catalog: %w", err)
		return
	}
	catalog = parsed
}

// Models returns a deep copy of the catalog section for channel.
func Models(channel Channel) ([]*cliproxy.ModelInfo, error) {
	loadOnce.Do(load)
	if loadErr != nil {
		return nil, loadErr
	}
	section := catalog[channel]
	out := make([]*cliproxy.ModelInfo, 0, len(section))
	for _, model := range section {
		if model == nil {
			continue
		}
		encoded, err := json.Marshal(model)
		if err != nil {
			return nil, fmt.Errorf("sdkmodels: copy %s: %w", model.ID, err)
		}
		var clone cliproxy.ModelInfo
		if err := json.Unmarshal(encoded, &clone); err != nil {
			return nil, fmt.Errorf("sdkmodels: copy %s: %w", model.ID, err)
		}
		out = append(out, &clone)
	}
	return out, nil
}
