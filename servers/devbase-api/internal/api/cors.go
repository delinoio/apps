// SPDX-License-Identifier: FSL-1.1-ALv2

package api

import "net/http"

// CORS limits browser access to one exact origin, the DevBase console. A request
// without an Origin header comes from a server or a command-line client and is
// unaffected. Any other origin is rejected before authentication, and an empty
// allowed origin therefore rejects every browser origin. The server never allows
// credentials because authentication uses bearer tokens only.
func CORS(allowed string, next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		origin := r.Header.Get("Origin")
		if origin == "" {
			next.ServeHTTP(w, r)
			return
		}
		if allowed == "" || origin != allowed {
			w.WriteHeader(http.StatusForbidden)
			return
		}
		h := w.Header()
		h.Set("Access-Control-Allow-Origin", origin)
		h.Add("Vary", "Origin")
		if r.Method == http.MethodOptions && r.Header.Get("Access-Control-Request-Method") != "" {
			h.Set("Access-Control-Allow-Methods", "POST")
			h.Set("Access-Control-Allow-Headers", "Authorization, Content-Type, Connect-Protocol-Version, Connect-Timeout-Ms")
			h.Set("Access-Control-Max-Age", "600")
			w.WriteHeader(http.StatusNoContent)
			return
		}
		h.Set("Access-Control-Expose-Headers", "X-Devbase-Trace-Id, Retry-After")
		next.ServeHTTP(w, r)
	})
}
