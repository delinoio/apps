// SPDX-License-Identifier: FSL-1.1-ALv2

package catalog

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func put(t *testing.T, dir, name, body string) {
	t.Helper()
	path := filepath.Join(dir, name)
	if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(body), 0600); err != nil {
		t.Fatal(err)
	}
}

func TestLoadCatalogFileSet(t *testing.T) {
	dir := t.TempDir()
	if _, err := load(dir); err == nil {
		t.Fatal("empty directory accepted")
	}
	if _, err := load(filepath.Join(dir, "missing")); err == nil {
		t.Fatal("missing directory accepted")
	}
	put(t, dir, "nested/service.yml", "kind: service\nkey: api\nname: API\nclient_id: api\ncapabilities: [execute]\nscopes: [authorize]\n")
	put(t, dir, "plan.yaml", "kind: plan\nkey: free\nname: Free\nrank: 0\nfree: true\ndefault_free: true\n")
	items, err := load(dir)
	if err != nil || len(items) != 2 {
		t.Fatalf("nested split files: %v, %d", err, len(items))
	}
	for name, body := range map[string]string{
		"duplicate.yaml":        "kind: service\nkey: api\nname: API\nclient_id: api\ncapabilities: [execute]\nscopes: [authorize]\n",
		"unknown.yaml":          "kind: plan\nkey: free\nname: Free\nrank: 0\nfree: true\nunknown: 1\n",
		"nested-unknown.yaml":   "kind: plan\nkey: free\nname: Free\nrank: 0\nfree: true\nfeatures: [{service: api, key: access, enabled: true, surprise: true}]\n",
		"nested-duplicate.yaml": "kind: plan\nkey: free\nname: Free\nrank: 0\nfree: true\nfeatures: [{service: api, service: api, key: access, enabled: true}]\n",
		"bad-type.yaml":         "kind: plan\nkey: free\nname: Free\nrank: invalid\nfree: true\n",
		"two.yaml":              "kind: plan\nkey: first\nname: First\nrank: 1\nfree: true\n---\nkind: plan\nkey: second\nname: Second\nrank: 2\nfree: true\n",
		"malformed.yaml":        "kind: [",
	} {
		t.Run(name, func(t *testing.T) {
			path := filepath.Join(dir, name)
			put(t, dir, name, body)
			if _, err := load(dir); err == nil {
				t.Fatal("invalid file accepted")
			}
			if err := os.Remove(path); err != nil {
				t.Fatal(err)
			}
		})
	}
}

func TestCountMeterRejectsValueAttribute(t *testing.T) {
	dir := t.TempDir()
	const head = "kind: meter\nservice: api\nkey: requests\nevent_type: request\n"
	put(t, dir, "meter.yaml", head+"aggregation: count\nvalue_attribute: bytes\n")
	if _, err := load(dir); err == nil || !strings.Contains(err.Error(), "count aggregation cannot have a value attribute") {
		t.Fatalf("count meter with a value attribute accepted: %v", err)
	}
	put(t, dir, "meter.yaml", head+"aggregation: count\n")
	if _, err := load(dir); err != nil {
		t.Fatalf("count meter without a value attribute rejected: %v", err)
	}
	put(t, dir, "meter.yaml", head+"aggregation: sum\nvalue_attribute: bytes\n")
	if _, err := load(dir); err != nil {
		t.Fatalf("sum meter with a value attribute rejected: %v", err)
	}
}

func TestFilterLiteralValidation(t *testing.T) {
	cases := []struct {
		name, body string
		valid      bool
	}{
		{"text", "values: [\"success\"]", true},
		{"numeric", "values: [\"2.5\"]", true},
		{"noncanonical", "values: [\"02.50\"]", false},
		{"unquoted", "values: [2.5]", false},
		{"exists-value", "values: [\"present\"]", false},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			dir := t.TempDir()
			operator := "greater_than"
			if tc.name == "text" {
				operator = "equal"
			}
			if tc.name == "exists-value" {
				operator = "exists"
			}
			body := "kind: meter\nkey: requests\nservice: api\nevent_type: request\naggregation: count\nfilters:\n  - attribute: value\n    operator: " + operator + "\n    " + tc.body + "\n"
			put(t, dir, "meter.yaml", body)
			_, err := load(dir)
			if (err == nil) != tc.valid {
				t.Fatalf("literal validation: %v", err)
			}
		})
	}
}

func TestExampleCatalogParses(t *testing.T) {
	items, err := load("../../catalog")
	if err != nil {
		t.Fatal(err)
	}
	if len(items) != 8 {
		t.Fatalf("example has %d definitions", len(items))
	}
	for _, item := range items {
		if !strings.Contains(item.Path, "catalog") {
			t.Fatal("lost source path")
		}
	}
}
