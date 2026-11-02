// SPDX-License-Identifier: FSL-1.1-ALv2

// Package config parses and validates the mcpa-api environment.
package config

import (
	"bufio"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"net/url"
	"os"
	"sort"
	"strings"
	"time"

	"github.com/google/uuid"
)

// LogFormat selects the log handler.
type LogFormat int

const (
	// LogFormatText is the default human-readable handler with ANSI color.
	LogFormatText LogFormat = iota + 1
	// LogFormatJSON emits one JSON object per line without color.
	LogFormatJSON
)

func (f LogFormat) String() string {
	switch f {
	case LogFormatText:
		return "text"
	case LogFormatJSON:
		return "json"
	default:
		return "unknown"
	}
}

// DefaultListen is the fixed loopback development address.
const DefaultListen = "127.0.0.1:46329"

// DefaultMFAClaim is the boolean access-token claim that carries MFA evidence.
const DefaultMFAClaim = "mcpa_mfa"

// Lookup reads one environment value. It matches os.LookupEnv.
type Lookup func(name string) (string, bool)

// OAuthClient is one approved MCP OAuth client.
type OAuthClient struct {
	Name         string   `json:"name"`
	RedirectURIs []string `json:"redirect_uris"`
}

// Logging holds the log settings shared by every subcommand.
type Logging struct {
	Format  LogFormat
	NoColor bool
}

// Database holds the settings used by every subcommand that touches PostgreSQL.
type Database struct {
	URL string
	// DependencyTimeout bounds each database and Delcore call.
	DependencyTimeout time.Duration
}

// Keys holds the credential master keys.
type Keys struct {
	MasterKeys  map[string][]byte
	ActiveKeyID string
}

// Serve is the complete configuration of `mcpa-api serve`.
type Serve struct {
	Logging
	Database
	Keys

	Listen string
	// PublicURL is the API origin without a trailing slash, for example
	// https://mcpa.api.delino.io.
	PublicURL string
	// WebOrigin is the console origin used for CORS and Origin checks.
	WebOrigin string
	// WebReturnURLs are the exact console return_to URLs.
	WebReturnURLs []string

	DelcoreURL      string
	DelcoreResource string
	// ServiceID is the Delcore service ID of mcpa, used as service_id for
	// IngestEvent and ListMeterAggregates.
	ServiceID     string
	LogtoTokenURL string
	ClientID      string
	ClientSecret  string

	LogtoIssuer         string
	ConsoleClientID     string
	ConsoleClientSecret string
	APIResource         string
	MCPResource         string
	MFAClaim            string
	MFAReauthURL        string
	OAuthClients        map[string]OAuthClient

	AuthorizeCacheTTL  time.Duration
	CheckpointInterval time.Duration
	OutboxInterval     time.Duration
	MetricsToken       string

	// TracingEnabled is true when OTEL_EXPORTER_OTLP_ENDPOINT is set.
	TracingEnabled bool
}

// InsecureCookies reports whether the session cookie may omit Secure. Only a
// plain-HTTP loopback public URL used for development qualifies.
func (s *Serve) InsecureCookies() bool {
	u, err := url.Parse(s.PublicURL)
	if err != nil || u.Scheme != "http" {
		return false
	}
	return isLoopbackHost(u.Hostname())
}

// Errors collects every configuration problem so startup reports them at once.
type Errors []string

func (e Errors) Error() string {
	return "invalid configuration: " + strings.Join(e, "; ")
}

type parser struct {
	lookup Lookup
	errs   Errors
}

func (p *parser) fail(format string, args ...any) {
	p.errs = append(p.errs, fmt.Sprintf(format, args...))
}

func (p *parser) optional(name string) string {
	v, _ := p.lookup(name)
	return strings.TrimSpace(v)
}

func (p *parser) required(name string) string {
	v := p.optional(name)
	if v == "" {
		p.fail("%s is required", name)
	}
	return v
}

func (p *parser) secret(name string) string {
	v, _ := p.lookup(name)
	if v == "" {
		p.fail("%s is required", name)
	}
	return v
}

func (p *parser) duration(name string, required bool, fallback time.Duration) time.Duration {
	raw := p.optional(name)
	if raw == "" {
		if required {
			p.fail("%s is required", name)
		}
		return fallback
	}
	d, err := time.ParseDuration(raw)
	if err != nil || d <= 0 {
		p.fail("%s must be a positive Go duration such as 30s", name)
		return fallback
	}
	return d
}

// origin validates an absolute origin without path, query or fragment.
func (p *parser) origin(name string) string {
	raw := p.required(name)
	if raw == "" {
		return ""
	}
	o, err := NormalizeOrigin(raw)
	if err != nil {
		p.fail("%s %v", name, err)
		return ""
	}
	return o
}

// absoluteURL validates an absolute http(s) URL.
func (p *parser) absoluteURL(name string, required bool) string {
	var raw string
	if required {
		raw = p.required(name)
	} else {
		raw = p.optional(name)
	}
	if raw == "" {
		return ""
	}
	u, err := url.Parse(raw)
	if err != nil || (u.Scheme != "https" && u.Scheme != "http") || u.Host == "" || u.User != nil {
		p.fail("%s must be an absolute http or https URL", name)
		return ""
	}
	if u.Scheme == "http" && !isLoopbackHost(u.Hostname()) {
		p.fail("%s must use https unless it targets a loopback host", name)
		return ""
	}
	return raw
}

// NormalizeOrigin returns the canonical scheme://host[:port] form of an origin.
func NormalizeOrigin(raw string) (string, error) {
	u, err := url.Parse(strings.TrimSuffix(raw, "/"))
	if err != nil || u.Host == "" || u.User != nil {
		return "", errors.New("must be an absolute origin such as https://example.com")
	}
	if u.Scheme != "https" && u.Scheme != "http" {
		return "", errors.New("must use http or https")
	}
	if u.Path != "" || u.RawQuery != "" || u.Fragment != "" {
		return "", errors.New("must not contain a path, query or fragment")
	}
	if u.Scheme == "http" && !isLoopbackHost(u.Hostname()) {
		return "", errors.New("must use https unless it targets a loopback host")
	}
	return strings.ToLower(u.Scheme) + "://" + strings.ToLower(u.Host), nil
}

func isLoopbackHost(host string) bool {
	host = strings.ToLower(host)
	if host == "localhost" || strings.HasSuffix(host, ".localhost") {
		return true
	}
	ip := net.ParseIP(host)
	return ip != nil && ip.IsLoopback()
}

func (p *parser) logging() Logging {
	l := Logging{Format: LogFormatText}
	switch strings.ToLower(p.optional("LOG_FORMAT")) {
	case "", "text":
	case "json":
		l.Format = LogFormatJSON
	default:
		p.fail("LOG_FORMAT must be text or json")
	}
	// NO_COLOR follows https://no-color.org: any non-empty value disables color.
	if v, ok := p.lookup("NO_COLOR"); ok && v != "" {
		l.NoColor = true
	}
	if l.Format == LogFormatJSON {
		l.NoColor = true
	}
	return l
}

func (p *parser) database() Database {
	d := Database{URL: p.secret("DATABASE_URL")}
	d.DependencyTimeout = p.duration("DEPENDENCY_TIMEOUT", false, 5*time.Second)
	return d
}

func (p *parser) keys() Keys {
	k := Keys{MasterKeys: map[string][]byte{}}
	raw := p.secret("MCPA_MASTER_KEYS")
	k.ActiveKeyID = p.required("MCPA_ACTIVE_KEY_ID")
	if raw == "" {
		return k
	}
	var encoded map[string]string
	if err := json.Unmarshal([]byte(raw), &encoded); err != nil {
		p.fail("MCPA_MASTER_KEYS must be a JSON object of key IDs to base64url keys")
		return k
	}
	if len(encoded) == 0 {
		p.fail("MCPA_MASTER_KEYS must contain at least one key")
	}
	for id, value := range encoded {
		if !validKeyID(id) {
			p.fail("MCPA_MASTER_KEYS key IDs must match [A-Za-z0-9._-]{1,64}")
			continue
		}
		key, err := decodeKey(value)
		if err != nil {
			// Never include the key material in the error.
			p.fail("MCPA_MASTER_KEYS[%q] must be a base64url encoded 32-byte key", id)
			continue
		}
		k.MasterKeys[id] = key
	}
	if k.ActiveKeyID != "" && len(k.MasterKeys) > 0 {
		if _, ok := k.MasterKeys[k.ActiveKeyID]; !ok {
			p.fail("MCPA_ACTIVE_KEY_ID must name a key in MCPA_MASTER_KEYS")
		}
	}
	return k
}

func validKeyID(id string) bool {
	if id == "" || len(id) > 64 {
		return false
	}
	for _, r := range id {
		ok := r == '.' || r == '_' || r == '-' || (r >= '0' && r <= '9') || (r >= 'a' && r <= 'z') || (r >= 'A' && r <= 'Z')
		if !ok {
			return false
		}
	}
	return true
}

func decodeKey(value string) ([]byte, error) {
	value = strings.TrimSpace(value)
	for _, enc := range []*base64.Encoding{base64.RawURLEncoding, base64.URLEncoding} {
		if b, err := enc.DecodeString(value); err == nil {
			if len(b) != 32 {
				return nil, errors.New("wrong key length")
			}
			return b, nil
		}
	}
	return nil, errors.New("invalid base64url")
}

// LoadLogging reads only the log settings. It never fails; invalid values
// fall back to defaults so early errors can still be logged.
func LoadLogging(lookup Lookup) Logging {
	p := &parser{lookup: lookup}
	return p.logging()
}

// LoadDatabase reads the settings of `migrate` and `check-schema`.
func LoadDatabase(lookup Lookup) (Database, error) {
	p := &parser{lookup: lookup}
	d := p.database()
	if len(p.errs) > 0 {
		return Database{}, p.errs
	}
	return d, nil
}

// LoadRotation reads the settings of `rotate-master-key`.
func LoadRotation(lookup Lookup) (Database, Keys, error) {
	p := &parser{lookup: lookup}
	d := p.database()
	k := p.keys()
	if len(p.errs) > 0 {
		return Database{}, Keys{}, p.errs
	}
	return d, k, nil
}

// LoadServe reads and validates the settings of `serve`.
func LoadServe(lookup Lookup) (*Serve, error) {
	p := &parser{lookup: lookup}
	s := &Serve{
		Logging:  p.logging(),
		Database: p.database(),
		Keys:     p.keys(),
	}
	s.Listen = p.optional("LISTEN")
	if s.Listen == "" {
		s.Listen = DefaultListen
	} else if _, _, err := net.SplitHostPort(s.Listen); err != nil {
		p.fail("LISTEN must be host:port")
	}
	s.PublicURL = p.origin("MCPA_PUBLIC_URL")
	s.WebOrigin = p.origin("WEB_ORIGIN")
	s.WebReturnURLs = p.returnURLs(s.WebOrigin)

	s.DelcoreURL = p.absoluteURL("DELCORE_URL", true)
	s.DelcoreURL = strings.TrimSuffix(s.DelcoreURL, "/")
	s.DelcoreResource = p.required("DELCORE_RESOURCE")
	s.ServiceID = p.required("MCPA_SERVICE_ID")
	if s.ServiceID != "" {
		if id, err := uuid.Parse(s.ServiceID); err != nil || id.String() != s.ServiceID {
			p.fail("MCPA_SERVICE_ID must be a canonical lowercase UUID")
		}
	}
	s.LogtoTokenURL = p.absoluteURL("LOGTO_TOKEN_URL", true)
	s.ClientID = p.required("CLIENT_ID")
	s.ClientSecret = p.secret("CLIENT_SECRET")

	s.LogtoIssuer = p.absoluteURL("LOGTO_ISSUER", true)
	s.ConsoleClientID = p.required("MCPA_LOGTO_CLIENT_ID")
	s.ConsoleClientSecret = p.secret("MCPA_LOGTO_CLIENT_SECRET")
	s.APIResource = p.required("MCPA_API_RESOURCE")
	s.MCPResource = p.required("MCPA_MCP_RESOURCE")
	if s.APIResource != "" && s.APIResource == s.MCPResource {
		p.fail("MCPA_MCP_RESOURCE must differ from MCPA_API_RESOURCE")
	}
	if s.DelcoreResource != "" && (s.DelcoreResource == s.APIResource || s.DelcoreResource == s.MCPResource) {
		p.fail("DELCORE_RESOURCE must differ from the mcpa API and MCP resources")
	}
	s.MFAClaim = p.optional("MCPA_MFA_CLAIM")
	if s.MFAClaim == "" {
		s.MFAClaim = DefaultMFAClaim
	}
	s.MFAReauthURL = p.absoluteURL("MCPA_MFA_REAUTH_URL", true)
	s.OAuthClients = p.oauthClients()

	s.AuthorizeCacheTTL = p.duration("MCPA_AUTHORIZE_CACHE_TTL", true, 0)
	s.CheckpointInterval = p.duration("MCPA_CHECKPOINT_INTERVAL", true, 0)
	s.OutboxInterval = p.duration("MCPA_OUTBOX_INTERVAL", true, 0)
	s.MetricsToken, _ = lookup("MCPA_METRICS_TOKEN")
	s.TracingEnabled = p.optional("OTEL_EXPORTER_OTLP_ENDPOINT") != "" || p.optional("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT") != ""

	if len(p.errs) > 0 {
		return nil, p.errs
	}
	return s, nil
}

func (p *parser) returnURLs(webOrigin string) []string {
	raw := p.required("MCPA_WEB_RETURN_URLS")
	if raw == "" {
		return nil
	}
	var urls []string
	if err := json.Unmarshal([]byte(raw), &urls); err != nil || len(urls) == 0 {
		p.fail("MCPA_WEB_RETURN_URLS must be a non-empty JSON array of URLs")
		return nil
	}
	for _, r := range urls {
		u, err := url.Parse(r)
		if err != nil || u.Host == "" || u.RawQuery != "" || u.Fragment != "" || u.User != nil {
			p.fail("MCPA_WEB_RETURN_URLS entries must be absolute URLs without query or fragment")
			continue
		}
		origin, err := NormalizeOrigin(u.Scheme + "://" + u.Host)
		if err != nil || (webOrigin != "" && origin != webOrigin) {
			p.fail("MCPA_WEB_RETURN_URLS entries must use the WEB_ORIGIN origin")
		}
	}
	return urls
}

func (p *parser) oauthClients() map[string]OAuthClient {
	raw := p.required("MCPA_OAUTH_CLIENTS")
	clients := map[string]OAuthClient{}
	if raw == "" {
		return clients
	}
	dec := json.NewDecoder(strings.NewReader(raw))
	dec.DisallowUnknownFields()
	if err := dec.Decode(&clients); err != nil {
		p.fail("MCPA_OAUTH_CLIENTS must be a JSON object of client IDs to {name, redirect_uris}")
		return map[string]OAuthClient{}
	}
	ids := make([]string, 0, len(clients))
	for id := range clients {
		ids = append(ids, id)
	}
	sort.Strings(ids)
	for _, id := range ids {
		c := clients[id]
		if id == "" || c.Name == "" || len(c.RedirectURIs) == 0 {
			p.fail("MCPA_OAUTH_CLIENTS[%q] needs a name and at least one redirect URI", id)
			continue
		}
		for _, r := range c.RedirectURIs {
			u, err := url.Parse(r)
			if err != nil || u.Scheme == "" || u.Fragment != "" {
				p.fail("MCPA_OAUTH_CLIENTS[%q] redirect URIs must be absolute URIs without fragment", id)
			}
		}
	}
	return clients
}

// LoadDotEnv reads KEY=VALUE lines from path into a map. Missing files return
// an empty map. Values may be wrapped in single or double quotes.
func LoadDotEnv(path string) (map[string]string, error) {
	f, err := os.Open(path)
	if errors.Is(err, os.ErrNotExist) {
		return map[string]string{}, nil
	}
	if err != nil {
		return nil, err
	}
	defer f.Close()
	out := map[string]string{}
	scanner := bufio.NewScanner(f)
	scanner.Buffer(make([]byte, 64*1024), 1024*1024)
	line := 0
	for scanner.Scan() {
		line++
		text := strings.TrimSpace(scanner.Text())
		if text == "" || strings.HasPrefix(text, "#") {
			continue
		}
		text = strings.TrimPrefix(text, "export ")
		name, value, ok := strings.Cut(text, "=")
		if !ok || strings.TrimSpace(name) == "" {
			return nil, fmt.Errorf("%s:%d: expected NAME=VALUE", path, line)
		}
		value = strings.TrimSpace(value)
		if len(value) >= 2 && (value[0] == '"' && value[len(value)-1] == '"' || value[0] == '\'' && value[len(value)-1] == '\'') {
			value = value[1 : len(value)-1]
		}
		out[strings.TrimSpace(name)] = value
	}
	return out, scanner.Err()
}

// WithFallback returns a lookup that prefers primary and falls back to values.
// The process environment therefore wins over a service-local .env file.
func WithFallback(primary Lookup, values map[string]string) Lookup {
	return func(name string) (string, bool) {
		if v, ok := primary(name); ok {
			return v, true
		}
		v, ok := values[name]
		return v, ok
	}
}
