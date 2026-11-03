// SPDX-License-Identifier: FSL-1.1-ALv2

package auth_test

import (
	"context"
	"errors"
	"net/http"
	"net/url"
	"testing"

	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
)

func TestLogtoTokenEndpointStatusClassification(t *testing.T) {
	cases := map[string]struct {
		status      int
		body        map[string]string
		unavailable bool
		invalid     bool
	}{
		"rate limited":      {http.StatusTooManyRequests, map[string]string{"error": "too_many_requests"}, true, false},
		"server error":      {http.StatusServiceUnavailable, map[string]string{"error": "server_error"}, true, false},
		"invalid grant":     {http.StatusBadRequest, map[string]string{"error": "invalid_grant"}, false, true},
		"other client fail": {http.StatusBadRequest, map[string]string{"error": "invalid_request"}, false, false},
	}
	for name, c := range cases {
		l, p := newLogto(t)
		p.SetTokenHandler(func(url.Values) (int, any) { return c.status, c.body })
		_, err := l.RefreshForResource(context.Background(), "rt", mcpAudience)
		if err == nil {
			t.Fatalf("%s: no error", name)
		}
		if got := errors.Is(err, auth.ErrLogtoUnavailable); got != c.unavailable {
			t.Fatalf("%s: unavailable = %v, want %v (%v)", name, got, c.unavailable, err)
		}
		if got := errors.Is(err, auth.ErrInvalidGrant); got != c.invalid {
			t.Fatalf("%s: invalid grant = %v, want %v (%v)", name, got, c.invalid, err)
		}
	}
}
