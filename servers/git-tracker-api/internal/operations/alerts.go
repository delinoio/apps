// SPDX-License-Identifier: FSL-1.1-ALv2

package operations

import (
	"errors"
	"fmt"
	"regexp"
	"strconv"
	"strings"
)

var windowPattern = regexp.MustCompile(`^[1-9][0-9]*[smhd]$`)

// AlertRules requires operator thresholds and windows; no numeric SLO is assumed.
func AlertRules(get func(string) string) (string, error) {
	var out strings.Builder
	out.WriteString("groups:\n  - name: git-tracker\n    rules:\n")
	for _, rule := range []struct{ name, key, expr string }{
		{"GitTrackerAPIErrors", "API_ERRORS", `sum(rate(git_tracker_http_requests_total{outcome="server_error"}[%s])) / clamp_min(sum(rate(git_tracker_http_requests_total[%s])), 0.000001)`},
		{"GitTrackerPersistenceFailures", "PERSISTENCE", `sum(increase(git_tracker_events_total{kind="persistence",category="failure"}[%s]))`},
		{"GitTrackerGitHubFailures", "GITHUB", `sum(increase(git_tracker_events_total{kind="github",category="failure"}[%s]))`},
		{"GitTrackerInterruptedExecutions", "INTERRUPTED", `sum(increase(git_tracker_events_total{kind="execution",category="interrupted"}[%s]))`},
	} {
		threshold, window, hold := get("GIT_TRACKER_ALERT_"+rule.key+"_THRESHOLD"), get("GIT_TRACKER_ALERT_"+rule.key+"_WINDOW"), get("GIT_TRACKER_ALERT_"+rule.key+"_FOR")
		value, e := strconv.ParseFloat(threshold, 64)
		if e != nil || value < 0 || !regexp.MustCompile(`^[0-9]+(\.[0-9]+)?$`).MatchString(threshold) || !windowPattern.MatchString(window) || !windowPattern.MatchString(hold) {
			return "", errors.New("valid threshold, observation window and hold duration required for " + rule.key)
		}
		expr := fmt.Sprintf(rule.expr, window)
		if rule.key == "API_ERRORS" {
			expr = fmt.Sprintf(rule.expr, window, window)
		}
		fmt.Fprintf(&out, "      - alert: %s\n        expr: '%s > %s'\n        for: %s\n        labels:\n          service: git-tracker\n        annotations:\n          summary: %s exceeded the configured threshold\n", rule.name, expr, threshold, hold, rule.name)
	}
	return out.String(), nil
}
