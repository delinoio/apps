// SPDX-License-Identifier: FSL-1.1-ALv2

package catalog

import (
	"fmt"
	"testing"

	"go.yaml.in/yaml/v4"
)

func grantFromYAML(t *testing.T, body string) grant {
	t.Helper()
	var value grant
	if err := yaml.Unmarshal([]byte(body), &value); err != nil {
		t.Fatal(err)
	}
	return value
}

func TestNormalizeFeatureGrantTypes(t *testing.T) {
	for _, tc := range []struct {
		name, body      string
		kind, valueType int
		expected        string
		valid           bool
	}{
		{"access", "enabled: true", 1, 0, `map[enabled:true]`, true},
		{"metered", `limit: "10.00"`, 2, 0, `map[limit:10]`, true},
		{"boolean", "configuration: false", 3, 1, `map[boolean:false]`, true},
		{"integer", "configuration: 42", 3, 2, `map[integer:42]`, true},
		{"decimal", `configuration: "1.50"`, 3, 3, `map[decimal:1.5]`, true},
		{"string", `configuration: "eu-west"`, 3, 4, `map[text:eu-west]`, true},
		{"string-list", `configuration: ["eu", "us"]`, 3, 5, `map[string_list:[eu us]]`, true},
		{"wrong-boolean", `configuration: "true"`, 3, 1, "", false},
		{"wrong-integer", `configuration: 3.5`, 3, 2, "", false},
		{"wrong-decimal", `configuration: 1.5`, 3, 3, "", false},
		{"negative-limit", `limit: "-1"`, 2, 0, "", false},
		{"wrong-list", `configuration: ["eu", 3]`, 3, 5, "", false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			value, err := normalizeGrant(grantFromYAML(t, tc.body), tc.kind, tc.valueType)
			if (err == nil) != tc.valid {
				t.Fatalf("type validation: %v", err)
			}
			if tc.valid && fmt.Sprint(value) != tc.expected {
				t.Fatalf("normalized %v, want %s", value, tc.expected)
			}
		})
	}
}
