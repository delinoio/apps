// SPDX-License-Identifier: FSL-1.1-ALv2

package config

import (
	"os"
	"strings"
	"testing"

	"github.com/joho/godotenv"
)

func TestLoadEnvironment(t *testing.T) {
	values, err := godotenv.Read("../../.env.example")
	if err != nil {
		t.Fatal(err)
	}
	c, err := load(func(key string) string { return values[key] })
	if err != nil {
		t.Fatal(err)
	}
	if c.Listen != "127.0.0.1:8080" || c.RetryLimit != 8 || c.MaxPayloadBytes != 1048576 || !c.Development || len(c.ReturnURLs) != 1 {
		t.Fatalf("environment fields were not loaded: listener=%s retries=%d", c.Listen, c.RetryLimit)
	}
	for _, key := range []string{"RETRY_LIMIT", "MAX_PAYLOAD_BYTES", "DEVELOPMENT", "RETURN_URLS"} {
		t.Run(key, func(t *testing.T) {
			values[key] = "invalid-secret-value"
			_, err := load(func(key string) string { return values[key] })
			if err == nil || !strings.Contains(err.Error(), key) || strings.Contains(err.Error(), values[key]) {
				t.Fatalf("expected a safe field-specific parse error, got %v", err)
			}
			values, _ = godotenv.Read("../../.env.example")
		})
	}
	delete(values, "TIMEOUT_SECONDS")
	if _, err := load(func(key string) string { return values[key] }); err == nil {
		t.Fatal("missing policy accepted")
	}
}

func TestLoadDotEnvPrecedence(t *testing.T) {
	body, err := os.ReadFile("../../.env.example")
	if err != nil {
		t.Fatal(err)
	}
	t.Chdir(t.TempDir())
	if err := os.WriteFile(".env", body, 0600); err != nil {
		t.Fatal(err)
	}
	t.Setenv("LISTEN", "127.0.0.1:18080")
	c, err := Load()
	if err != nil {
		t.Fatal(err)
	}
	if c.Listen != "127.0.0.1:18080" {
		t.Fatal("process environment must override .env")
	}
	t.Setenv("IDENTITY_KEY", "")
	if _, err := Load(); err == nil {
		t.Fatal(".env restored an explicitly cleared secret")
	}
}

func TestLoadWithoutDotEnv(t *testing.T) {
	values, err := godotenv.Read("../../.env.example")
	if err != nil {
		t.Fatal(err)
	}
	for key, value := range values {
		t.Setenv(key, value)
	}
	t.Chdir(t.TempDir())
	if _, err := Load(); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(".env", []byte("IDENTITY_KEY=\"unterminated"), 0600); err != nil {
		t.Fatal(err)
	}
	if _, err := Load(); err == nil {
		t.Fatal("malformed .env accepted")
	}
}

func valid() Config {
	return Config{Listen: "127.0.0.1:8080", DatabaseURL: "postgres://localhost/delcore", Issuer: "https://issuer.example.test", Audience: "delcore", OperatorRole: "operator", IdentityKey: "fixture-identity-key-at-least-32-bytes", PaddleURL: "https://sandbox-api.paddle.com", PaddleCreditProductID: "pro_fixture", PaddleToken: "fixture", PaddleWebhookSecret: "fixture-secret-at-least-16-bytes", EmailURL: "https://api.cloudflare.com/client/v4", EmailToken: "fixture", EmailAccount: "account", EmailSender: "sender@example.test", ReturnURLs: []string{"https://consumer.example.test/return"}, TimeoutSeconds: 10, RetryLimit: 3, RetrySeconds: 10, MaxPayloadBytes: 1048576, RequestsPerMinute: 100, InvitationSeconds: 3600, UsageLatenessSeconds: 3600, ReportingGraceSeconds: 3600, DeletionGraceSeconds: 3600, RawUsageRetentionSeconds: 7200, AccountingRetentionSeconds: 14400, AuditRetentionSeconds: 14400, WebhookRetentionSeconds: 7200, BacklogAlert: 100, SettlementAlertSeconds: 3600}
}
func TestConfigValidation(t *testing.T) {
	c := valid()
	if e := c.Validate(); e != nil {
		t.Fatal(e)
	}
	for _, mutate := range []func(*Config){func(c *Config) { c.RetryLimit = 0 }, func(c *Config) { c.ReportingGraceSeconds = 1 }, func(c *Config) { c.AccountingRetentionSeconds = 1 }, func(c *Config) { c.IdentityKey = "" }, func(c *Config) { c.ReturnURLs = []string{"https://user:pass@example.test/return"} }, func(c *Config) { c.PaddleURL = "http://external.example.test" }} {
		c = valid()
		mutate(&c)
		if c.Validate() == nil {
			t.Fatal("unsafe configuration accepted")
		}
	}
	c = valid()
	if c.AllowsReturn("https://consumer.example.test/return?attacker=1") {
		t.Fatal("redirect allowlist bypass")
	}
}
