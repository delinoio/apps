// SPDX-License-Identifier: FSL-1.1-ALv2

package config

import "testing"

func lookupFrom(values map[string]string) func(string) (string, bool) {
	return func(name string) (string, bool) {
		value, ok := values[name]
		return value, ok
	}
}

func TestLoadDefaults(t *testing.T) {
	cfg, err := Load(lookupFrom(map[string]string{EnvAddr: "  ", EnvLogLevel: ""}))
	if err != nil {
		t.Fatalf("Load() error = %v", err)
	}
	if cfg.Addr != DefaultAddr {
		t.Errorf("Addr = %q, want %q", cfg.Addr, DefaultAddr)
	}
	if cfg.LogLevel != LogLevelInfo {
		t.Errorf("LogLevel = %q, want %q", cfg.LogLevel, LogLevelInfo)
	}
}

func TestLoadOverrides(t *testing.T) {
	cfg, err := Load(lookupFrom(map[string]string{EnvAddr: "0.0.0.0:9000", EnvLogLevel: "DEBUG"}))
	if err != nil {
		t.Fatalf("Load() error = %v", err)
	}
	if cfg.Addr != "0.0.0.0:9000" {
		t.Errorf("Addr = %q, want %q", cfg.Addr, "0.0.0.0:9000")
	}
	if cfg.LogLevel != LogLevelDebug {
		t.Errorf("LogLevel = %q, want %q", cfg.LogLevel, LogLevelDebug)
	}
}

func TestLoadRejectsInvalidValues(t *testing.T) {
	for name, values := range map[string]map[string]string{
		"missing port":   {EnvAddr: "127.0.0.1"},
		"port too large": {EnvAddr: "127.0.0.1:70000"},
		"named port":     {EnvAddr: "127.0.0.1:http"},
		"unknown level":  {EnvLogLevel: "verbose"},
	} {
		t.Run(name, func(t *testing.T) {
			if _, err := Load(lookupFrom(values)); err == nil {
				t.Fatal("Load() error = nil, want an error")
			}
		})
	}
}

func TestSlogLevel(t *testing.T) {
	for level, want := range map[LogLevel]string{
		LogLevelDebug: "DEBUG",
		LogLevelInfo:  "INFO",
		LogLevelWarn:  "WARN",
		LogLevelError: "ERROR",
	} {
		if got := level.SlogLevel().String(); got != want {
			t.Errorf("%q.SlogLevel() = %s, want %s", level, got, want)
		}
	}
}
