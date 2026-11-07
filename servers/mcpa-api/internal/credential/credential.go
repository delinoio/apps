// SPDX-License-Identifier: FSL-1.1-ALv2

// Package credential defines the encrypted payload of provider credentials
// and the validation shared by management and the data plane.
package credential

import (
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"net/netip"
	"net/url"
	"strings"
	"unicode"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/crypto"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

// PayloadVersion is the current payload format.
const PayloadVersion = 1

// Kind is the payload kind.
type Kind string

// Payload kinds.
const (
	KindAPIKey Kind = "api_key"
	KindOAuth  Kind = "oauth"
)

// Payload is the sealed secret of a credential. It is never logged or
// returned to callers.
type Payload struct {
	Version int  `json:"v"`
	Kind    Kind `json:"kind"`
	// APIKey is set for KindAPIKey.
	APIKey string `json:"api_key,omitempty"`
	// OAuth holds the provider token metadata of KindOAuth credentials in the
	// format owned by the data plane layer.
	OAuth json.RawMessage `json:"oauth,omitempty"`
}

// Seal encrypts p for credential c.
func Seal(keys *crypto.Keyring, c store.Credential, p Payload) (crypto.Sealed, error) {
	if p.Version == 0 {
		p.Version = PayloadVersion
	}
	plain, err := json.Marshal(p)
	if err != nil {
		return crypto.Sealed{}, err
	}
	defer clear(plain)
	return keys.Seal(plain, crypto.CredentialAAD(c.ID.String(), c.ResourceID.String()))
}

// Open decrypts the payload of credential c.
func Open(keys *crypto.Keyring, c store.Credential, s crypto.Sealed) (Payload, error) {
	plain, err := keys.Open(s, crypto.CredentialAAD(c.ID.String(), c.ResourceID.String()))
	if err != nil {
		return Payload{}, err
	}
	defer clear(plain)
	var p Payload
	if err := json.Unmarshal(plain, &p); err != nil {
		return Payload{}, fmt.Errorf("credential payload: %w", err)
	}
	if p.Version != PayloadVersion {
		return Payload{}, fmt.Errorf("credential payload: unsupported version %d", p.Version)
	}
	return p, nil
}

// SupportsAPIKey reports whether provider accepts API-key credentials.
func SupportsAPIKey(p mcpav1.Provider) bool {
	switch p {
	case mcpav1.Provider_PROVIDER_GEMINI, mcpav1.Provider_PROVIDER_VERTEX, mcpav1.Provider_PROVIDER_CLAUDE,
		mcpav1.Provider_PROVIDER_CODEX, mcpav1.Provider_PROVIDER_KIMI, mcpav1.Provider_PROVIDER_OPENAI_COMPATIBLE:
		return true
	default:
		return false
	}
}

// SupportsOAuth reports whether provider accepts OAuth accounts.
func SupportsOAuth(p mcpav1.Provider) bool {
	switch p {
	case mcpav1.Provider_PROVIDER_GEMINI_CLI, mcpav1.Provider_PROVIDER_CLAUDE, mcpav1.Provider_PROVIDER_CODEX,
		mcpav1.Provider_PROVIDER_ANTIGRAVITY, mcpav1.Provider_PROVIDER_KIMI:
		return true
	default:
		return false
	}
}

// Limits of credential fields.
const (
	MaxAPIKeyBytes    = 8192
	MaxModels         = 200
	MaxModelNameBytes = 200
	MaxBaseURLBytes   = 2048
)

// ValidateAPIKey checks the shape of an API key without logging it.
func ValidateAPIKey(key string) error {
	if key == "" || len(key) > MaxAPIKeyBytes {
		return fmt.Errorf("api_key must contain 1 to %d bytes", MaxAPIKeyBytes)
	}
	for _, r := range key {
		if unicode.IsSpace(r) || unicode.IsControl(r) || r > unicode.MaxASCII {
			return errors.New("api_key must contain printable ASCII characters without spaces")
		}
	}
	return nil
}

// SecretHint returns the last four characters of a long API key, or an
// empty string for keys too short to reveal any part of.
func SecretHint(key string) string {
	if len(key) < 12 {
		return ""
	}
	return key[len(key)-4:]
}

// NormalizeModels trims, validates and de-duplicates model names, keeping
// their first-seen order.
func NormalizeModels(models []string, field string) ([]string, error) {
	if len(models) > MaxModels {
		return nil, fmt.Errorf("%s may contain at most %d entries", field, MaxModels)
	}
	seen := map[string]bool{}
	out := make([]string, 0, len(models))
	for _, m := range models {
		m = strings.TrimSpace(m)
		if m == "" || len(m) > MaxModelNameBytes {
			return nil, fmt.Errorf("%s entries must contain 1 to %d bytes", field, MaxModelNameBytes)
		}
		for _, r := range m {
			if unicode.IsControl(r) || unicode.IsSpace(r) {
				return nil, fmt.Errorf("%s entries must not contain spaces or control characters", field)
			}
		}
		if !seen[m] {
			seen[m] = true
			out = append(out, m)
		}
	}
	return out, nil
}

// ValidateUpstreamURL checks a customer-supplied provider base URL. It must
// be an https URL whose host is a DNS name or a public IP literal. This is a
// first line of defense only: the data plane transport must also refuse to
// dial private, loopback and link-local addresses after DNS resolution.
func ValidateUpstreamURL(raw string) error {
	if len(raw) > MaxBaseURLBytes {
		return fmt.Errorf("base_url must contain at most %d bytes", MaxBaseURLBytes)
	}
	// url.Parse drops an empty fragment and keeps an empty query only in
	// ForceQuery, so a trailing "?" or "#" must be rejected on the raw text.
	u, err := url.Parse(raw)
	if err != nil || strings.ContainsAny(raw, "?#") || u.Scheme != "https" || u.Host == "" || u.User != nil || u.Fragment != "" || u.RawQuery != "" {
		return errors.New("base_url must be an https URL without credentials, query or fragment")
	}
	host := strings.TrimSuffix(strings.ToLower(u.Hostname()), ".")
	if host == "localhost" || strings.HasSuffix(host, ".localhost") || strings.HasSuffix(host, ".internal") || strings.HasSuffix(host, ".local") {
		return errors.New("base_url must target a public host")
	}
	if ip, err := netip.ParseAddr(host); err == nil {
		if !PublicAddr(ip) {
			return errors.New("base_url must target a public host")
		}
	} else if net.ParseIP(host) != nil || !strings.Contains(host, ".") {
		return errors.New("base_url must target a public host")
	}
	return nil
}

// PublicAddr reports whether ip is a globally routable unicast address.
func PublicAddr(ip netip.Addr) bool {
	ip = ip.Unmap()
	return ip.IsValid() && ip.IsGlobalUnicast() && !ip.IsPrivate() && !ip.IsLoopback() &&
		!ip.IsLinkLocalUnicast() && !ip.IsInterfaceLocalMulticast() && !ip.IsUnspecified() &&
		!netip.MustParsePrefix("100.64.0.0/10").Contains(ip) && !netip.MustParsePrefix("fc00::/7").Contains(ip)
}
