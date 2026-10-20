// SPDX-License-Identifier: FSL-1.1-ALv2

package api

import (
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestCORSAllowsOnlyTheConfiguredOrigin(t *testing.T) {
	const console = "http://127.0.0.1:46313"
	reached := 0
	next := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { reached++; w.WriteHeader(http.StatusOK) })
	do := func(allowed, method, origin string, extra map[string]string) *httptest.ResponseRecorder {
		r := httptest.NewRequest(method, "/devbase.v1.DevBaseService/GetProject", nil)
		if origin != "" {
			r.Header.Set("Origin", origin)
		}
		for k, v := range extra {
			r.Header.Set(k, v)
		}
		w := httptest.NewRecorder()
		CORS(allowed, next).ServeHTTP(w, r)
		return w
	}

	reached = 0
	w := do(console, http.MethodOptions, console, map[string]string{"Access-Control-Request-Method": "POST"})
	if w.Code != http.StatusNoContent || reached != 0 {
		t.Fatalf("preflight not answered by the middleware: %d reached=%d", w.Code, reached)
	}
	h := w.Header()
	if h.Get("Access-Control-Allow-Origin") != console || h.Get("Vary") != "Origin" || h.Get("Access-Control-Allow-Methods") != "POST" {
		t.Fatalf("unexpected preflight headers %v", h)
	}
	if h.Get("Access-Control-Allow-Headers") != "Authorization, Content-Type, Connect-Protocol-Version, Connect-Timeout-Ms" || h.Get("Access-Control-Allow-Credentials") != "" {
		t.Fatalf("unexpected allowed headers or credentials %v", h)
	}

	w = do(console, http.MethodPost, console, nil)
	if w.Code != http.StatusOK || reached != 1 {
		t.Fatalf("allowed origin did not reach the handler: %d reached=%d", w.Code, reached)
	}
	h = w.Header()
	if h.Get("Access-Control-Allow-Origin") != console || h.Get("Access-Control-Expose-Headers") != "X-Devbase-Trace-Id, Retry-After" || h.Get("Access-Control-Allow-Credentials") != "" {
		t.Fatalf("unexpected response headers %v", h)
	}

	reached = 0
	for name, tc := range map[string]struct{ allowed, origin string }{
		"other origin":             {console, "https://example.com"},
		"scheme differs":           {console, "https://127.0.0.1:46313"},
		"port differs":             {console, "http://127.0.0.1:46314"},
		"nothing configured":       {"", console},
		"null origin":              {console, "null"},
		"trailing slash in origin": {console, console + "/"},
	} {
		for _, method := range []string{http.MethodPost, http.MethodOptions} {
			w = do(tc.allowed, method, tc.origin, map[string]string{"Access-Control-Request-Method": "POST"})
			if w.Code != http.StatusForbidden || w.Header().Get("Access-Control-Allow-Origin") != "" {
				t.Errorf("%s %s: status %d headers %v", name, method, w.Code, w.Header())
			}
		}
	}
	if reached != 0 {
		t.Fatalf("rejected origins reached the handler %d times", reached)
	}

	// Servers and command-line clients send no Origin header.
	for _, allowed := range []string{"", console} {
		if w = do(allowed, http.MethodPost, "", nil); w.Code != http.StatusOK || w.Header().Get("Access-Control-Allow-Origin") != "" {
			t.Fatalf("request without origin changed: %d %v", w.Code, w.Header())
		}
	}
}
