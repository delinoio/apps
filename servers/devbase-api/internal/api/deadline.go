// SPDX-License-Identifier: FSL-1.1-ALv2

package api

import (
	"context"
	"net/http"
	"time"
)

// Deadline gives each request one deadline that starts when its headers arrive and
// covers reading the body as well as execution. Connect decodes the body before it
// calls a service method, so a timeout that starts inside the method would let a slow
// sender use the whole budget while reading and then receive a second one. The same
// instant bounds the body read on the connection and the request context.
func Deadline(timeout time.Duration, next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		deadline := time.Now().Add(timeout)
		// A writer that cannot move the read deadline keeps the server-wide ReadTimeout.
		_ = http.NewResponseController(w).SetReadDeadline(deadline)
		ctx, cancel := context.WithDeadline(r.Context(), deadline)
		defer cancel()
		next.ServeHTTP(w, r.WithContext(ctx))
	})
}
