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
		name, operator, body string
		valid                bool
	}{
		{"text", "equal", "values: [\"success\"]", true},
		{"number-equal", "equal", "values: [5]", true},
		{"boolean-not-equal", "not_equal", "values: [true]", true},
		{"mixed-membership", "in", "values: [\"a\", 2.5, false]", true},
		{"noncanonical-number-equal", "equal", "values: [2.50]", false},
		{"exponent-number", "in", "values: [1e3]", false},
		{"null-value", "equal", "values: [null]", false},
		{"nested-value", "equal", "values: [[1]]", false},
		{"numeric", "greater_than", "values: [\"2.5\"]", true},
		{"unquoted-numeric", "greater_than", "values: [2.5]", true},
		{"noncanonical", "greater_than", "values: [\"02.50\"]", false},
		{"boolean-comparison", "less_than", "values: [true]", false},
		{"text-comparison", "less_than", "values: [\"high\"]", false},
		{"exists", "exists", "values: []", true},
		{"exists-value", "exists", "values: [\"present\"]", false},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			dir := t.TempDir()
			body := "kind: meter\nkey: requests\nservice: api\nevent_type: request\naggregation: count\nfilters:\n  - attribute: value\n    operator: " + tc.operator + "\n    " + tc.body + "\n"
			put(t, dir, "meter.yaml", body)
			_, err := load(dir)
			if (err == nil) != tc.valid {
				t.Fatalf("literal validation: %v", err)
			}
		})
	}
}

// The committed sample is the documented format reference. Keep one example of
// every supported declaration so it cannot silently lose coverage.
func TestExampleCatalogCoversTheDocumentedFormat(t *testing.T) {
	root := "../../catalog"
	items, err := load(root)
	if err != nil {
		t.Fatal(err)
	}
	has := map[string]bool{}
	planFeatures := map[string]bool{}
	var addonFeatures []string
	prices := map[string]map[string]bool{}
	for _, item := range items {
		relative, err := filepath.Rel(root, item.Path)
		if err != nil {
			t.Fatal(err)
		}
		has["ext:"+filepath.Ext(item.Path)] = true
		if strings.Contains(relative, string(filepath.Separator)) {
			has["nested"] = true
		}
		has["kind:"+item.Kind] = true
		switch item.Kind {
		case "meter":
			has["aggregation:"+item.Aggregation] = true
			if item.Aggregation == "sum" && len(item.Filters) > 0 && len(item.GroupBy) > 0 {
				has["sum-with-filters-and-group-by"] = true
			}
			for _, f := range item.Filters {
				has["operator:"+f.Operator] = true
			}
		case "feature":
			has["feature:"+item.Type+":"+item.ValueType] = true
		case "plan":
			for _, g := range item.Features {
				planFeatures[g.Service+":"+g.Key] = true
			}
		case "addon":
			for _, g := range item.Features {
				addonFeatures = append(addonFeatures, g.Service+":"+g.Key)
			}
		case "price":
			owner := item.Plan
			if item.AddOn != nil {
				owner = "addon:" + item.AddOn.Key
			}
			if prices[owner] == nil {
				prices[owner] = map[string]bool{}
			}
			prices[owner][item.PriceKind] = true
			if item.PriceKind == "usage" && item.Included != "0" {
				prices[owner]["included"] = true
			}
		}
	}
	for _, want := range []string{
		"ext:.yaml", "ext:.yml", "nested",
		"kind:service", "kind:meter", "kind:feature", "kind:plan", "kind:addon", "kind:price",
		"aggregation:count", "aggregation:sum", "aggregation:unique_count", "sum-with-filters-and-group-by",
		"operator:exists", "operator:in", "operator:not_equal", "operator:less_than",
		"feature:access:", "feature:metered:",
		"feature:configuration:boolean", "feature:configuration:integer", "feature:configuration:decimal",
		"feature:configuration:string", "feature:configuration:string_list",
	} {
		if !has[want] {
			t.Errorf("sample catalog lacks %s", want)
		}
	}
	paid := false
	for _, kinds := range prices {
		paid = paid || kinds["recurring"] && kinds["usage"] && kinds["included"]
	}
	if !paid {
		t.Error("sample catalog lacks a plan with a fixed fee, a usage price, and included usage")
	}
	if len(addonFeatures) == 0 {
		t.Error("sample add-on grants no feature")
	}
	for _, feature := range addonFeatures {
		if planFeatures[feature] {
			t.Errorf("sample add-on feature %s conflicts with a plan grant", feature)
		}
	}
}
