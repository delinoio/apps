// SPDX-License-Identifier: FSL-1.1-ALv2

package config

import (
	"encoding/json"
	"errors"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/joho/godotenv"
)

// MaxCardinality is the largest accepted metering cardinality setting. It keeps
// one group's retained value multiset and one period's admission rows bounded.
const MaxCardinality = 1_000_000

type Config struct {
	PaddleCreditProductID      string
	Listen                     string
	DatabaseURL                string
	Issuer                     string
	Audience                   string
	OperatorRole               string
	IdentityKey                string
	PaddleURL                  string
	PaddleToken                string
	PaddleWebhookSecret        string
	EmailURL                   string
	EmailToken                 string
	EmailAccount               string
	EmailSender                string
	ReturnURLs                 []string
	TimeoutSeconds             int
	RetryLimit                 int
	RetrySeconds               int
	MaxPayloadBytes            int64
	RequestsPerMinute          int
	InvitationSeconds          int
	UsageLatenessSeconds       int
	ReportingGraceSeconds      int
	DeletionGraceSeconds       int
	RawUsageRetentionSeconds   int
	AccountingRetentionSeconds int
	AuditRetentionSeconds      int
	WebhookRetentionSeconds    int
	BacklogAlert               int
	SettlementAlertSeconds     int
	// MaxMeterGroups bounds the groups of one tenant meter billing period.
	MaxMeterGroups int
	// MaxMeterGroupValues bounds the distinct values retained by one minimum,
	// maximum, or unique-count group.
	MaxMeterGroupValues int
	Development         bool
	CatalogDir          string
}

// Load reads .env from the working directory. Process variables take precedence,
// including explicitly empty values, so a file cannot restore a cleared secret.
func Load() (Config, error) {
	local, err := godotenv.Read(".env")
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		return Config{}, errors.New("could not read .env configuration")
	}
	return load(func(key string) string {
		if value, ok := os.LookupEnv(key); ok {
			return value
		}
		return local[key]
	})
}

func load(getenv func(string) string) (Config, error) {
	var c Config
	c.PaddleCreditProductID = getenv("PADDLE_CREDIT_PRODUCT_ID")
	c.Listen = getenv("LISTEN")
	c.DatabaseURL = getenv("DATABASE_URL")
	c.Issuer = getenv("ISSUER")
	c.Audience = getenv("AUDIENCE")
	c.OperatorRole = getenv("OPERATOR_ROLE")
	c.IdentityKey = getenv("IDENTITY_KEY")
	c.PaddleURL = getenv("PADDLE_URL")
	c.PaddleToken = getenv("PADDLE_TOKEN")
	c.PaddleWebhookSecret = getenv("PADDLE_WEBHOOK_SECRET")
	c.EmailURL = getenv("EMAIL_URL")
	c.EmailToken = getenv("EMAIL_TOKEN")
	c.EmailAccount = getenv("EMAIL_ACCOUNT")
	c.EmailSender = getenv("EMAIL_SENDER")
	c.CatalogDir = getenv("CATALOG_DIR")
	if c.CatalogDir == "" {
		c.CatalogDir = "servers/delcore-api/catalog"
	} else {
		if strings.TrimSpace(c.CatalogDir) != c.CatalogDir || strings.ContainsRune(c.CatalogDir, 0) || filepath.Clean(c.CatalogDir) == "." {
			return c, errors.New("CATALOG_DIR must name a directory")
		}
		c.CatalogDir = filepath.Clean(c.CatalogDir)
	}
	if err := json.Unmarshal([]byte(getenv("RETURN_URLS")), &c.ReturnURLs); err != nil {
		return c, errors.New("RETURN_URLS must be a JSON array of strings")
	}
	for _, setting := range []struct {
		name   string
		target *int
	}{
		{"TIMEOUT_SECONDS", &c.TimeoutSeconds},
		{"RETRY_LIMIT", &c.RetryLimit},
		{"RETRY_SECONDS", &c.RetrySeconds},
		{"REQUESTS_PER_MINUTE", &c.RequestsPerMinute},
		{"INVITATION_SECONDS", &c.InvitationSeconds},
		{"USAGE_LATENESS_SECONDS", &c.UsageLatenessSeconds},
		{"REPORTING_GRACE_SECONDS", &c.ReportingGraceSeconds},
		{"DELETION_GRACE_SECONDS", &c.DeletionGraceSeconds},
		{"RAW_USAGE_RETENTION_SECONDS", &c.RawUsageRetentionSeconds},
		{"ACCOUNTING_RETENTION_SECONDS", &c.AccountingRetentionSeconds},
		{"AUDIT_RETENTION_SECONDS", &c.AuditRetentionSeconds},
		{"WEBHOOK_RETENTION_SECONDS", &c.WebhookRetentionSeconds},
		{"BACKLOG_ALERT", &c.BacklogAlert},
		{"SETTLEMENT_ALERT_SECONDS", &c.SettlementAlertSeconds},
		{"MAX_METER_GROUPS", &c.MaxMeterGroups},
		{"MAX_METER_GROUP_VALUES", &c.MaxMeterGroupValues},
	} {
		value, err := strconv.Atoi(getenv(setting.name))
		if err != nil {
			return c, fmt.Errorf("%s must be a base-10 integer", setting.name)
		}
		*setting.target = value
	}
	payloadBytes, err := strconv.ParseInt(getenv("MAX_PAYLOAD_BYTES"), 10, 64)
	if err != nil {
		return c, errors.New("MAX_PAYLOAD_BYTES must be a base-10 integer")
	}
	c.MaxPayloadBytes = payloadBytes
	if value := getenv("DEVELOPMENT"); value != "" {
		parsed, err := strconv.ParseBool(value)
		if err != nil {
			return c, errors.New("DEVELOPMENT has an invalid bool value")
		}
		c.Development = parsed
	}
	return c, c.Validate()
}
func (c Config) Validate() error {
	if c.Listen == "" || c.DatabaseURL == "" || c.Audience == "" || c.OperatorRole == "" || len(c.IdentityKey) < 32 || len(c.PaddleWebhookSecret) < 16 || c.PaddleCreditProductID == "" || c.PaddleToken == "" || c.EmailToken == "" || c.EmailAccount == "" || c.EmailSender == "" {
		return errors.New("required configuration or credentials missing")
	}
	for _, n := range []int{c.TimeoutSeconds, c.RetryLimit, c.RetrySeconds, c.RequestsPerMinute, c.InvitationSeconds, c.UsageLatenessSeconds, c.ReportingGraceSeconds, c.DeletionGraceSeconds, c.RawUsageRetentionSeconds, c.AccountingRetentionSeconds, c.AuditRetentionSeconds, c.WebhookRetentionSeconds, c.BacklogAlert, c.SettlementAlertSeconds} {
		if n <= 0 || n > 100*365*24*60*60 {
			return errors.New("policy limits must be explicitly positive and no greater than 100 years in seconds")
		}
	}
	if c.MaxMeterGroups <= 0 || c.MaxMeterGroups > MaxCardinality || c.MaxMeterGroupValues <= 0 || c.MaxMeterGroupValues > MaxCardinality {
		return fmt.Errorf("MAX_METER_GROUPS and MAX_METER_GROUP_VALUES must be 1 to %d", MaxCardinality)
	}
	if c.TimeoutSeconds > 120 || c.MaxPayloadBytes < 1024 || c.MaxPayloadBytes > 16<<20 || c.RetryLimit > 100 {
		return errors.New("request or retry bounds exceed safety limits")
	}
	if c.ReportingGraceSeconds < c.UsageLatenessSeconds || c.RawUsageRetentionSeconds < c.UsageLatenessSeconds || c.AccountingRetentionSeconds < c.RawUsageRetentionSeconds || c.WebhookRetentionSeconds < c.ReportingGraceSeconds {
		return errors.New("contradictory retention or reporting policies")
	}
	for _, s := range append([]string{c.Issuer, c.PaddleURL, c.EmailURL}, c.ReturnURLs...) {
		if !safeURL(s, c.Development) {
			return fmt.Errorf("invalid endpoint or return URL")
		}
	}
	if len(c.ReturnURLs) == 0 {
		return errors.New("return URL allowlist is required")
	}
	return nil
}
func safeURL(s string, dev bool) bool {
	u, e := url.Parse(s)
	return e == nil && u.Host != "" && u.User == nil && u.Fragment == "" && (u.Scheme == "https" || (dev && u.Scheme == "http" && (u.Hostname() == "127.0.0.1" || u.Hostname() == "localhost" || u.Hostname() == "::1")))
}
func (c Config) AllowsReturn(s string) bool {
	for _, v := range c.ReturnURLs {
		if s == v {
			return true
		}
	}
	return false
}
func (c Config) Timeout() time.Duration { return time.Duration(c.TimeoutSeconds) * time.Second }
