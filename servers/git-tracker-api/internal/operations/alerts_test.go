// SPDX-License-Identifier: FSL-1.1-ALv2

package operations

import (
	"strings"
	"testing"
)

func TestOperatorThresholds(t *testing.T) {
	if _, e := AlertRules(func(string) string { return "" }); e == nil {
		t.Fatal("accepted missing thresholds")
	}
	settings := map[string]string{
		"ALERT_API_ERRORS_THRESHOLD":  "1",
		"ALERT_API_ERRORS_WINDOW":     "5m",
		"ALERT_API_ERRORS_FOR":        "5m",
		"ALERT_PERSISTENCE_THRESHOLD": "1",
		"ALERT_PERSISTENCE_WINDOW":    "5m",
		"ALERT_PERSISTENCE_FOR":       "5m",
		"ALERT_GITHUB_THRESHOLD":      "1",
		"ALERT_GITHUB_WINDOW":         "5m",
		"ALERT_GITHUB_FOR":            "5m",
		"ALERT_INTERRUPTED_THRESHOLD": "1",
		"ALERT_INTERRUPTED_WINDOW":    "5m",
		"ALERT_INTERRUPTED_FOR":       "5m",
	}
	out, e := AlertRules(func(key string) string { return settings[key] })
	if e != nil || strings.Count(out, "- alert:") != 4 || strings.Contains(out, "%!") {
		t.Fatal(out, e)
	}
}
