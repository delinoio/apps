// SPDX-License-Identifier: Apache-2.0
package main

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func put(t *testing.T, root, path, body string) {
	t.Helper()
	full := filepath.Join(root, path)
	if err := os.MkdirAll(filepath.Dir(full), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(full, []byte(body), 0644); err != nil {
		t.Fatal(err)
	}
}
func fixture(t *testing.T) string {
	t.Helper()
	root := t.TempDir()
	put(t, root, "pnpm-workspace.yaml", "packages:\n  - apps/*\n  - packages/**\n  - '!packages/excluded'\n")
	put(t, root, "package.json", `{"name":"repo","private":true}`)
	put(t, root, "apps/web/package.json", `{"name":"@example/web","private":true}`)
	put(t, root, "packages/nested/lib/package.json", `{"name":"@example/lib"}`)
	put(t, root, "packages/excluded/package.json", `{"name":"excluded"}`)
	put(t, root, "node_modules/vendor/package.json", `{"name":"vendor"}`)
	put(t, root, "packages/nested/lib/node_modules/vendor/package.json", `{"name":"vendor"}`)
	put(t, root, ".changeset/README.md", "Pending records")
	return root
}
func invoke(t *testing.T, root string, args ...string) string {
	t.Helper()
	var output bytes.Buffer
	if err := run(root, args, &output); err != nil {
		t.Fatal(err)
	}
	return output.String()
}
func TestInventory(t *testing.T) {
	root := fixture(t)
	got, err := inventory(root)
	if err != nil {
		t.Fatal(err)
	}
	if len(got) != 3 || got["@example/web"] != "apps/web/package.json" || got["@example/lib"] != "packages/nested/lib/package.json" {
		t.Fatalf("unexpected inventory: %v", got)
	}
	put(t, root, "apps/duplicate/package.json", `{"name":"@example/web"}`)
	if _, err := inventory(root); err == nil {
		t.Fatal("accepted duplicate package names")
	}
}
func TestLifecycleAndPublicBoundary(t *testing.T) {
	root := fixture(t)
	invoke(t, root, "add", "--id", "private-fix", "--package", "@example/web:patch", "--summary", "INTERNAL_ONLY")
	invoke(t, root, "add", "--id", "visible-fix", "--package", "@example/web:minor", "--package", "@example/lib:patch", "--summary", "ENGINEERING_ONLY", "--public", "Faster search.\n\nMore reliable results.")
	before, err := os.ReadFile(filepath.Join(root, ".changeset/visible-fix.json"))
	if err != nil {
		t.Fatal(err)
	}
	if got := invoke(t, root, "check"); got != "validated 2 changesets\n" {
		t.Fatal(got)
	}
	public := invoke(t, root, "changelog")
	if strings.Contains(public, "ONLY") || strings.Contains(public, "visible-fix") || strings.Count(public, "Faster search.") != 2 {
		t.Fatalf("incorrect public selection: %s", public)
	}
	if strings.Index(public, "## @example/lib") > strings.Index(public, "## @example/web") {
		t.Fatal("packages not sorted")
	}
	internal := invoke(t, root, "changelog", "--internal")
	if !strings.Contains(internal, "INTERNAL_ONLY") || !strings.Contains(internal, "ENGINEERING_ONLY") || strings.Contains(internal, "Faster search.") {
		t.Fatal(internal)
	}
	var records []pending
	if err := json.Unmarshal([]byte(invoke(t, root, "status")), &records); err != nil {
		t.Fatal(err)
	}
	if len(records) != 2 || records[0].ID != "private-fix" {
		t.Fatal(records)
	}
	after, _ := os.ReadFile(filepath.Join(root, ".changeset/visible-fix.json"))
	if !bytes.Equal(before, after) {
		t.Fatal("read commands mutated record")
	}
	var output bytes.Buffer
	if err := run(root, []string{"add", "--id", "visible-fix", "--package", "@example/web:major", "--summary", "replace"}, &output); err == nil {
		t.Fatal("overwrote existing record")
	}
	after, _ = os.ReadFile(filepath.Join(root, ".changeset/visible-fix.json"))
	if !bytes.Equal(before, after) {
		t.Fatal("duplicate add changed record")
	}
}
func TestEmptyPublicSelection(t *testing.T) {
	root := fixture(t)
	if got := invoke(t, root, "status"); got != "[]\n" {
		t.Fatal(got)
	}
	if got := invoke(t, root, "changelog"); got != "" {
		t.Fatal(got)
	}
	invoke(t, root, "add", "--id", "internal", "--package", "repo:patch", "--summary", "secret implementation", "--public", " \n ")
	if got := invoke(t, root, "changelog"); got != "" {
		t.Fatal(got)
	}
}
func TestInvalidRecords(t *testing.T) {
	for name, body := range map[string]string{
		"version":           `{"schemaVersion":2,"packages":{"repo":"patch"},"summary":"x"}`,
		"unknown-package":   `{"schemaVersion":1,"packages":{"unknown":"patch"},"summary":"x"}`,
		"bump":              `{"schemaVersion":1,"packages":{"repo":"huge"},"summary":"x"}`,
		"empty-packages":    `{"schemaVersion":1,"packages":{},"summary":"x"}`,
		"empty-summary":     `{"schemaVersion":1,"packages":{"repo":"patch"},"summary":" "}`,
		"unknown-field":     `{"schemaVersion":1,"packages":{"repo":"patch"},"summary":"x","publc":"typo"}`,
		"duplicate-package": `{"schemaVersion":1,"packages":{"repo":"patch","repo":"major"},"summary":"x"}`,
		"duplicate-public":  `{"schemaVersion":1,"packages":{"repo":"patch"},"summary":"x","public":"a","public":"b"}`,
		"trailing":          `{"schemaVersion":1,"packages":{"repo":"patch"},"summary":"x"} {}`,
		"malformed":         `{`,
	} {
		t.Run(name, func(t *testing.T) {
			root := fixture(t)
			put(t, root, ".changeset/bad.json", body)
			for _, cmd := range []string{"check", "status", "changelog"} {
				var output bytes.Buffer
				if err := run(root, []string{cmd}, &output); err == nil {
					t.Fatalf("%s accepted invalid record", cmd)
				}
				if output.Len() != 0 {
					t.Fatal("partial output before validation")
				}
			}
		})
	}
}
func TestInvalidInputs(t *testing.T) {
	root := fixture(t)
	for _, args := range [][]string{
		{"add", "--id", "../escape", "--package", "repo:patch", "--summary", "x"},
		{"add", "--id", "valid", "--package", "repo:patch", "--package", "repo:major", "--summary", "x"},
		{"add", "--id", "valid", "--package", "repo:patch"},
		{"check", "unexpected"}, {"changelog", "--public"}, {"delete"},
	} {
		var output bytes.Buffer
		if err := run(root, args, &output); err == nil {
			t.Fatalf("accepted %v", args)
		}
	}
	entries, _ := os.ReadDir(filepath.Join(root, ".changeset"))
	if len(entries) != 1 {
		t.Fatal("invalid add wrote files")
	}
}
func TestUnexpectedEntries(t *testing.T) {
	for _, name := range []string{"ignored.md", "INVALID.json", "nested"} {
		t.Run(name, func(t *testing.T) {
			root := fixture(t)
			put(t, root, filepath.Join(".changeset", name), "{}")
			var output bytes.Buffer
			if err := run(root, []string{"check"}, &output); err == nil {
				t.Fatal("accepted unexpected entry")
			}
		})
	}
	root := fixture(t)
	if err := os.Symlink(filepath.Join(root, "package.json"), filepath.Join(root, ".changeset/link.json")); err != nil {
		t.Skip(err)
	}
	var output bytes.Buffer
	if err := run(root, []string{"check"}, &output); err == nil {
		t.Fatal("accepted symlink")
	}
}
