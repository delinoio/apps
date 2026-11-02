// SPDX-License-Identifier: FSL-1.1-ALv2

package main

import (
	"bytes"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestRunRejectsBadArguments(t *testing.T) {
	cases := map[string][]string{
		"no command":       nil,
		"unknown command":  {"bogus"},
		"extra argument":   {"migrate", "extra"},
		"unknown flag":     {"serve", "--bogus"},
		"flag before name": {"--dev-env"},
	}
	for name, args := range cases {
		t.Run(name, func(t *testing.T) {
			var stderr bytes.Buffer
			if code := run(args, &stderr); code != 2 {
				t.Fatalf("exit code = %d, want 2; stderr: %s", code, stderr.String())
			}
		})
	}
}

// TestConfigErrorsNameSettingsWithoutValues runs check-schema with a secret
// DATABASE_URL from .env and an invalid process DEPENDENCY_TIMEOUT that must
// win over the file's valid value.
func TestConfigErrorsNameSettingsWithoutValues(t *testing.T) {
	dir := t.TempDir()
	const secret = "postgres://mcpa:hunter2-secret@127.0.0.1:1/mcpa"
	env := "DATABASE_URL=\"" + secret + "\"\nDEPENDENCY_TIMEOUT=1s\nLOG_FORMAT=json\n"
	if err := os.WriteFile(filepath.Join(dir, ".env"), []byte(env), 0o600); err != nil {
		t.Fatal(err)
	}
	t.Chdir(dir)
	t.Setenv("DEPENDENCY_TIMEOUT", "not-a-duration")
	// Unset inherited values so the .env file supplies them; t.Setenv
	// restores them at cleanup.
	for _, name := range []string{"DATABASE_URL", "LOG_FORMAT"} {
		if v, ok := os.LookupEnv(name); ok {
			t.Setenv(name, v)
			os.Unsetenv(name)
		}
	}

	var stderr bytes.Buffer
	if code := run([]string{"check-schema", "--dev-env"}, &stderr); code != 1 {
		t.Fatalf("exit code = %d, want 1; stderr: %s", code, stderr.String())
	}
	out := stderr.String()
	if !strings.Contains(out, "DEPENDENCY_TIMEOUT") {
		t.Fatalf("expected the process value to win and fail validation: %s", out)
	}
	if strings.Contains(out, "hunter2") || strings.Contains(out, "not-a-duration") {
		t.Fatalf("configuration error leaked a value: %s", out)
	}
	if !strings.HasPrefix(strings.TrimSpace(out), "{") {
		t.Fatalf("LOG_FORMAT=json from .env was not applied: %s", out)
	}
}

func TestDevEnvIsOptional(t *testing.T) {
	t.Chdir(t.TempDir())
	t.Setenv("DATABASE_URL", "")
	var stderr bytes.Buffer
	// No .env: configuration validation runs and reports the empty URL.
	if code := run([]string{"migrate", "--dev-env"}, &stderr); code != 1 {
		t.Fatalf("exit code = %d, want 1; stderr: %s", code, stderr.String())
	}
	if !strings.Contains(stderr.String(), "DATABASE_URL") {
		t.Fatalf("expected a DATABASE_URL error: %s", stderr.String())
	}
}
