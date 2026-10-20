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
	out, e := AlertRules(func(key string) string {
		if strings.HasSuffix(key, "THRESHOLD") {
			return "1"
		}
		return "5m"
	})
	if e != nil || strings.Count(out, "- alert:") != 4 || strings.Contains(out, "%!") {
		t.Fatal(out, e)
	}
}
