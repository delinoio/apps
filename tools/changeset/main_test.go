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
	before, err := os.ReadFile(filepath.Join(root, ".changeset/visible-fix.md"))
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
	after, _ := os.ReadFile(filepath.Join(root, ".changeset/visible-fix.md"))
	if !bytes.Equal(before, after) {
		t.Fatal("read commands mutated record")
	}
	var output bytes.Buffer
	if err := run(root, []string{"add", "--id", "visible-fix", "--package", "@example/web:major", "--summary", "replace"}, &output); err == nil {
		t.Fatal("overwrote existing record")
	}
	after, _ = os.ReadFile(filepath.Join(root, ".changeset/visible-fix.md"))
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
		"unknown-package":      "---\nunknown: patch\n---\nx",
		"bump":                 "---\nrepo: huge\n---\nx",
		"non-string":           "---\nrepo: 1\n---\nx",
		"duplicate-package":    "---\nrepo: patch\nrepo: major\n---\nx",
		"nested-map":           "---\nrepo: {type: patch}\n---\nx",
		"sequence":             "---\n- repo\n---\nx",
		"missing-frontmatter":  "summary",
		"unclosed-frontmatter": "---\nrepo: patch",
		"malformed-yaml":       "---\n[\n---\nx",
		"unclosed-public":      "---\nrepo: patch\n---\nx\n" + publicStart + "\nnote",
		"reversed-public":      "---\nrepo: patch\n---\n" + publicEnd + "\n" + publicStart,
		"duplicate-public":     "---\nrepo: patch\n---\n" + publicStart + "\n" + publicStart + "\n" + publicEnd,
		"trailing-private":     "---\nrepo: patch\n---\nx\n" + publicStart + "\nnote\n" + publicEnd + "\nprivate",
		"inline-public":        "---\nrepo: patch\n---\nx " + publicStart + "\nnote\n" + publicEnd,
		"fenced-public":        "---\nrepo: patch\n---\n```md\n" + publicStart + "\nnote\n" + publicEnd,
	} {
		t.Run(name, func(t *testing.T) {
			root := fixture(t)
			put(t, root, ".changeset/bad.md", body)
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
		{"add", "--id", "readme", "--package", "repo:patch", "--summary", "x"},
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
	if err := os.Symlink(filepath.Join(root, "package.json"), filepath.Join(root, ".changeset/link.md")); err != nil {
		t.Skip(err)
	}
	var output bytes.Buffer
	if err := run(root, []string{"check"}, &output); err == nil {
		t.Fatal("accepted symlink")
	}
}

func TestChangesetsMarkdown(t *testing.T) {
	for _, body := range []string{
		"---\n\"@example/web\": minor\nrepo: none\n---\n\nPlain Markdown summary.\n\n## Details\n\n- First item\n- Second item\n",
		"---\r\n\"@example/web\": patch\r\n---\r\n\r\nWindows line endings.\r\n",
		"---\n---\n",
		"---\n{}\n---\n\nNo release needed.\n",
		"---\nrepo: patch\n---\n",
	} {
		t.Run(body, func(t *testing.T) {
			root := fixture(t)
			put(t, root, ".changeset/standard.md", body)
			put(t, root, ".changeset/config.json", `{"baseBranch":"main"}`)
			invoke(t, root, "check")
			if output := invoke(t, root, "changelog"); output != "" {
				t.Fatalf("unmarked body leaked to public output: %s", output)
			}
		})
	}
	root := fixture(t)
	invoke(t, root, "add", "--id", "generated", "--package", "@example/web:minor", "--summary", "Internal detail.", "--public", "User-facing note.")
	data, err := os.ReadFile(filepath.Join(root, ".changeset/generated.md"))
	if err != nil {
		t.Fatal(err)
	}
	want := "---\n\"@example/web\": minor\n---\n\nInternal detail.\n\n" + publicStart + "\nUser-facing note.\n" + publicEnd + "\n"
	if string(data) != want {
		t.Fatalf("unexpected Markdown:\n%s", data)
	}
}

func TestReservedMarkersCannotBeInjected(t *testing.T) {
	for _, field := range []string{"--summary", "--public"} {
		t.Run(field, func(t *testing.T) {
			root := fixture(t)
			args := []string{"add", "--id", "injected", "--package", "repo:patch", "--summary", "internal", field, publicStart + "\nprivate\n" + publicEnd}
			var output bytes.Buffer
			if err := run(root, args, &output); err == nil {
				t.Fatal("accepted reserved markers")
			}
			if _, err := os.Stat(filepath.Join(root, ".changeset/injected.md")); !os.IsNotExist(err) {
				t.Fatal("invalid add created a file")
			}
		})
	}
}
