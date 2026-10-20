// SPDX-License-Identifier: FSL-1.1-ALv2
package config

import (
	"encoding/json"
	"errors"
	pb "github.com/delinoio/apps/protos/delitools/v1"
	"github.com/delinoio/apps/servers/delitools-api/internal/api"
	"github.com/google/uuid"
	"google.golang.org/protobuf/encoding/protojson"
	"net"
	"net/url"
	"os"
	"strings"
)

type Config struct {
	Listen                                string          `json:"listen"`
	Issuer                                string          `json:"issuer"`
	Audience                              string          `json:"audience"`
	OperatorRole                          string          `json:"operator_role"`
	DelcoreURL                            string          `json:"delcore_url"`
	ServiceID                             string          `json:"service_id"`
	TokenURL                              string          `json:"token_url"`
	ClientID                              string          `json:"client_id"`
	GatewayURL                            string          `json:"gateway_url"`
	HTTPTimeoutSeconds                    uint32          `json:"http_timeout_seconds"`
	MaxGatewayResponseBytes               int64           `json:"max_gateway_response_bytes"`
	SettingsJSON                          json.RawMessage `json:"settings"`
	Settings                              *pb.Settings    `json:"-"`
	DatabaseURL, GatewayKey, ClientSecret string          `json:"-"`
}

func Load(path string, getenv func(string) string, migrate bool) (Config, error) {
	var c Config
	b, e := os.ReadFile(path)
	if e != nil {
		return c, errors.New("operator configuration unavailable")
	}
	d := json.NewDecoder(strings.NewReader(string(b)))
	d.DisallowUnknownFields()
	if e = d.Decode(&c); e != nil {
		return c, errors.New("invalid operator configuration")
	}
	c.DatabaseURL = getenv("DELITOOLS_DATABASE_URL")
	if c.DatabaseURL == "" {
		return c, errors.New("database configuration required")
	}
	if migrate {
		return c, nil
	}
	c.GatewayKey = getenv("DELITOOLS_GATEWAY_KEY")
	c.ClientSecret = getenv("DELITOOLS_CLIENT_SECRET")
	if c.GatewayKey == "" || c.ClientSecret == "" || c.ClientID == "" || c.Audience == "" || c.OperatorRole == "" || c.ServiceID == "" || c.HTTPTimeoutSeconds == 0 || c.MaxGatewayResponseBytes <= 0 {
		return c, errors.New("missing runtime configuration")
	}
	if u, err := uuid.Parse(c.ServiceID); err != nil || u.Version() != 7 || u.String() != c.ServiceID {
		return c, errors.New("invalid service identity")
	}
	if _, _, e = net.SplitHostPort(c.Listen); e != nil {
		return c, errors.New("invalid listen address")
	}
	for _, raw := range []string{c.Issuer, c.DelcoreURL, c.TokenURL, c.GatewayURL} {
		u, e := url.Parse(raw)
		if e != nil || u.Host == "" || u.User != nil || u.RawQuery != "" || u.Fragment != "" {
			return c, errors.New("invalid dependency URL")
		}
		if u.Scheme != "https" && (u.Scheme != "http" || (u.Hostname() != "127.0.0.1" && u.Hostname() != "localhost" && u.Hostname() != "::1")) {
			return c, errors.New("dependency URL requires TLS or loopback")
		}
	}
	c.Settings = &pb.Settings{}
	if e = protojson.Unmarshal(c.SettingsJSON, c.Settings); e != nil {
		return c, errors.New("invalid settings")
	}
	return c, api.ValidateSettings(c.Settings)
}
