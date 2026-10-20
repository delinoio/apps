// SPDX-License-Identifier: FSL-1.1-ALv2
package config

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestStartupValidation(t *testing.T) {
	b, e := os.ReadFile("../../config.example.json")
	if e != nil {
		t.Fatal(e)
	}
	path := filepath.Join(t.TempDir(), "config.json")
	if e = os.WriteFile(path, b, 0600); e != nil {
		t.Fatal(e)
	}
	env := func(string) string { return "fixture" }
	if _, e = Load(path, env, false); e != nil {
		t.Fatal(e)
	}
	if _, e = Load(path, func(string) string { return "" }, false); e == nil {
		t.Fatal("missing secrets accepted")
	}
	bad := strings.Replace(string(b), `"maxAiRetries": 2,`, "", 1)
	if e = os.WriteFile(path, []byte(bad), 0600); e != nil {
		t.Fatal(e)
	}
	if _, e = Load(path, env, false); e == nil {
		t.Fatal("missing retry setting accepted")
	}
	if _, e = Load(path, env, true); e != nil {
		t.Fatal("migration unnecessarily requires runtime settings", e)
	}
}

func TestRejectTrailingConfiguration(t *testing.T) {
	b, err := os.ReadFile("../../config.example.json")
	if err != nil {
		t.Fatal(err)
	}
	for _, suffix := range []string{"{}", "false", "garbage", " \n\t"} {
		path := filepath.Join(t.TempDir(), "config.json")
		if err := os.WriteFile(path, append(b, []byte(suffix)...), 0600); err != nil {
			t.Fatal(err)
		}
		for _, migrate := range []bool{false, true} {
			_, err := Load(path, func(string) string { return "fixture" }, migrate)
			if (err == nil) != (strings.TrimSpace(suffix) == "") {
				t.Fatalf("suffix %q migration %v: %v", suffix, migrate, err)
			}
		}
	}
}
