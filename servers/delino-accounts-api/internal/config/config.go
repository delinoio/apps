// SPDX-License-Identifier: FSL-1.1-ALv2

// Package config loads and validates the delino-accounts-api configuration
// from environment variables.
package config

import (
	"fmt"
	"log/slog"
	"net"
	"strconv"
	"strings"
)

// Environment variable names read by the server.
const (
	EnvAddr     = "DELINO_ACCOUNTS_API_ADDR"
	EnvLogLevel = "DELINO_ACCOUNTS_API_LOG_LEVEL"
)

// DefaultAddr listens on loopback only so that a local run is never exposed
// to the network by accident.
const DefaultAddr = "127.0.0.1:8100"

// LogLevel is the accepted set of log level values.
type LogLevel string

const (
	LogLevelDebug LogLevel = "debug"
	LogLevelInfo  LogLevel = "info"
	LogLevelWarn  LogLevel = "warn"
	LogLevelError LogLevel = "error"
)

// SlogLevel converts the configured level to a slog level.
func (l LogLevel) SlogLevel() slog.Level {
	switch l {
	case LogLevelDebug:
		return slog.LevelDebug
	case LogLevelWarn:
		return slog.LevelWarn
	case LogLevelError:
		return slog.LevelError
	default:
		return slog.LevelInfo
	}
}

func parseLogLevel(value string) (LogLevel, error) {
	switch level := LogLevel(strings.ToLower(value)); level {
	case LogLevelDebug, LogLevelInfo, LogLevelWarn, LogLevelError:
		return level, nil
	default:
		return "", fmt.Errorf("%s must be one of debug, info, warn, error; got %q", EnvLogLevel, value)
	}
}

// Config is the validated server configuration.
type Config struct {
	Addr     string
	LogLevel LogLevel
}

// Load reads the configuration through lookup, which has the signature of
// os.LookupEnv. Unset or empty values select the documented defaults.
func Load(lookup func(string) (string, bool)) (Config, error) {
	cfg := Config{Addr: DefaultAddr, LogLevel: LogLevelInfo}

	if value := lookupNonEmpty(lookup, EnvAddr); value != "" {
		if err := validateAddr(value); err != nil {
			return Config{}, err
		}
		cfg.Addr = value
	}

	if value := lookupNonEmpty(lookup, EnvLogLevel); value != "" {
		level, err := parseLogLevel(value)
		if err != nil {
			return Config{}, err
		}
		cfg.LogLevel = level
	}

	return cfg, nil
}

func lookupNonEmpty(lookup func(string) (string, bool), name string) string {
	value, _ := lookup(name)
	return strings.TrimSpace(value)
}

func validateAddr(value string) error {
	_, port, err := net.SplitHostPort(value)
	if err != nil {
		return fmt.Errorf("%s must be a host:port address: %w", EnvAddr, err)
	}
	if _, err := strconv.ParseUint(port, 10, 16); err != nil {
		return fmt.Errorf("%s port must be a number from 0 to 65535; got %q", EnvAddr, port)
	}
	return nil
}
