// SPDX-License-Identifier: FSL-1.1-ALv2

// Package config reads and validates the server's environment configuration.
package config

import (
	"errors"
	"fmt"
	"net/url"
	"strconv"
	"time"
)

const DefaultListen = "127.0.0.1:46317"

// Limits are the configurable resource limits and timeouts.
type Limits struct {
	MaxProjectsPerTenant    int64
	MaxUsersPerProject      int64
	MaxActiveKeysPerProject int64
	UserRate                float64
	UserBurst               int
	ManagementRate          float64
	ManagementBurst         int
	DependencyTimeout       time.Duration
	RequestTimeout          time.Duration
	CleanupInterval         time.Duration
}

// DefaultLimits returns the initial limits.
func DefaultLimits() Limits {
	return Limits{
		MaxProjectsPerTenant:    100,
		MaxUsersPerProject:      1_000_000,
		MaxActiveKeysPerProject: 20,
		UserRate:                1000,
		UserBurst:               2000,
		ManagementRate:          50,
		ManagementBurst:         100,
		DependencyTimeout:       3 * time.Second,
		RequestTimeout:          10 * time.Second,
		CleanupInterval:         5 * time.Minute,
	}
}

type Config struct {
	DatabaseURL     string
	Listen          string
	DelcoreURL      string
	DelcoreResource string
	TokenURL        string
	ClientID        string
	ClientSecret    string
	Limits
}

// Database reads only the settings that database commands need.
func Database(get func(string) string) (Config, error) {
	c := Config{DatabaseURL: get("DATABASE_URL"), Limits: DefaultLimits()}
	if c.DatabaseURL == "" {
		return c, errors.New("DATABASE_URL is required")
	}
	return c, nil
}

// Load reads and validates the full server configuration.
func Load(get func(string) string) (Config, error) {
	c, e := Database(get)
	if e != nil {
		return c, e
	}
	c.Listen = get("LISTEN")
	if c.Listen == "" {
		c.Listen = DefaultListen
	}
	c.DelcoreURL, c.DelcoreResource, c.TokenURL = get("DELCORE_URL"), get("DELCORE_RESOURCE"), get("LOGTO_TOKEN_URL")
	c.ClientID, c.ClientSecret = get("CLIENT_ID"), get("CLIENT_SECRET")
	for name, value := range map[string]string{"DELCORE_URL": c.DelcoreURL, "DELCORE_RESOURCE": c.DelcoreResource, "LOGTO_TOKEN_URL": c.TokenURL, "CLIENT_ID": c.ClientID, "CLIENT_SECRET": c.ClientSecret} {
		if value == "" {
			return c, fmt.Errorf("%s is required", name)
		}
	}
	ints := []struct {
		name string
		to   *int64
	}{{"MAX_PROJECTS_PER_TENANT", &c.MaxProjectsPerTenant}, {"MAX_USERS_PER_PROJECT", &c.MaxUsersPerProject}, {"MAX_ACTIVE_KEYS_PER_PROJECT", &c.MaxActiveKeysPerProject}}
	for _, v := range ints {
		if e = integer(get, v.name, v.to); e != nil {
			return c, e
		}
	}
	for _, v := range []struct {
		name string
		to   *float64
	}{{"USER_RATE_PER_SECOND", &c.UserRate}, {"MANAGEMENT_RATE_PER_SECOND", &c.ManagementRate}} {
		if s := get(v.name); s != "" {
			f, e := strconv.ParseFloat(s, 64)
			if e != nil {
				return c, fmt.Errorf("%s must be a number", v.name)
			}
			*v.to = f
		}
	}
	for _, v := range []struct {
		name string
		to   *int
	}{{"USER_RATE_BURST", &c.UserBurst}, {"MANAGEMENT_RATE_BURST", &c.ManagementBurst}} {
		var n int64 = int64(*v.to)
		if e = integer(get, v.name, &n); e != nil {
			return c, e
		}
		*v.to = int(n)
	}
	for _, v := range []struct {
		name string
		to   *time.Duration
	}{{"DEPENDENCY_TIMEOUT", &c.DependencyTimeout}, {"REQUEST_TIMEOUT", &c.RequestTimeout}, {"CLEANUP_INTERVAL", &c.CleanupInterval}} {
		if s := get(v.name); s != "" {
			d, e := time.ParseDuration(s)
			if e != nil {
				return c, fmt.Errorf("%s must be a duration", v.name)
			}
			*v.to = d
		}
	}
	return c, c.Validate()
}

func integer(get func(string) string, name string, to *int64) error {
	s := get(name)
	if s == "" {
		return nil
	}
	n, e := strconv.ParseInt(s, 10, 64)
	if e != nil {
		return fmt.Errorf("%s must be an integer", name)
	}
	*to = n
	return nil
}

// Validate rejects missing, non-positive and contradictory settings.
func (c Config) Validate() error {
	if c.DatabaseURL == "" || c.Listen == "" {
		return errors.New("database URL and listen address are required")
	}
	if c.MaxProjectsPerTenant <= 0 || c.MaxUsersPerProject <= 0 || c.MaxActiveKeysPerProject <= 0 {
		return errors.New("resource limits must be positive")
	}
	if c.UserRate <= 0 || c.ManagementRate <= 0 || c.UserBurst <= 0 || c.ManagementBurst <= 0 {
		return errors.New("rate limits and bursts must be positive")
	}
	if float64(c.UserBurst) < c.UserRate || float64(c.ManagementBurst) < c.ManagementRate {
		return errors.New("a burst must not be smaller than its sustained rate")
	}
	if c.DependencyTimeout <= 0 || c.RequestTimeout <= 0 || c.CleanupInterval <= 0 {
		return errors.New("timeouts and intervals must be positive")
	}
	if c.DependencyTimeout > c.RequestTimeout {
		return errors.New("the dependency timeout must not exceed the request timeout")
	}
	for _, s := range []string{c.DelcoreURL, c.TokenURL} {
		if s != "" && !safeURL(s) {
			return errors.New("Delcore and token endpoints must be https or loopback http URLs")
		}
	}
	return nil
}

func safeURL(s string) bool {
	u, e := url.Parse(s)
	if e != nil || u.Host == "" || u.User != nil || u.Fragment != "" {
		return false
	}
	h := u.Hostname()
	return u.Scheme == "https" || (u.Scheme == "http" && (h == "127.0.0.1" || h == "localhost" || h == "::1"))
}
