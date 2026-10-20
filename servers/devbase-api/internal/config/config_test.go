// SPDX-License-Identifier: FSL-1.1-ALv2

package config

import (
	"testing"
	"time"
)

func env(extra map[string]string) func(string) string {
	base := map[string]string{
		"DATABASE_URL": "postgres://x", "DELCORE_URL": "https://delcore.example", "DELCORE_RESOURCE": "https://delcore.example",
		"LOGTO_TOKEN_URL": "https://logto.example/oidc/token", "CLIENT_ID": "id", "CLIENT_SECRET": "secret",
	}
	for k, v := range extra {
		base[k] = v
	}
	return func(k string) string { return base[k] }
}

func TestDefaults(t *testing.T) {
	c, e := Load(env(nil))
	if e != nil {
		t.Fatal(e)
	}
	if c.Listen != "127.0.0.1:46317" || c.MaxProjectsPerTenant != 100 || c.MaxUsersPerProject != 1_000_000 || c.MaxActiveKeysPerProject != 20 {
		t.Fatalf("unexpected defaults %+v", c)
	}
	if c.UserRate != 1000 || c.UserBurst != 2000 || c.ManagementRate != 50 || c.ManagementBurst != 100 {
		t.Fatalf("unexpected rates %+v", c)
	}
	if c.DependencyTimeout != 3*time.Second || c.RequestTimeout != 10*time.Second || c.CleanupInterval != 5*time.Minute {
		t.Fatalf("unexpected timeouts %+v", c)
	}
}

func TestRejections(t *testing.T) {
	for name, extra := range map[string]map[string]string{
		"missing client":     {"CLIENT_ID": ""},
		"zero limit":         {"MAX_PROJECTS_PER_TENANT": "0"},
		"bad integer":        {"MAX_USERS_PER_PROJECT": "many"},
		"burst below rate":   {"USER_RATE_BURST": "10"},
		"timeout ordering":   {"DEPENDENCY_TIMEOUT": "20s"},
		"bad duration":       {"CLEANUP_INTERVAL": "soon"},
		"insecure endpoint":  {"DELCORE_URL": "http://delcore.example"},
		"negative rate":      {"MANAGEMENT_RATE_PER_SECOND": "-1"},
		"non-numeric rate":   {"USER_RATE_PER_SECOND": "fast"},
		"zero cleanup cycle": {"CLEANUP_INTERVAL": "0s"},
	} {
		if _, e := Load(env(extra)); e == nil {
			t.Errorf("%s accepted", name)
		}
	}
	if _, e := Load(env(map[string]string{"DELCORE_URL": "http://127.0.0.1:8080"})); e != nil {
		t.Fatal("loopback development endpoint rejected", e)
	}
	if _, e := Database(func(string) string { return "" }); e == nil {
		t.Fatal("database command accepted missing DATABASE_URL")
	}
}
