// SPDX-License-Identifier: FSL-1.1-ALv2

package api

import (
	"io"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

func TestDeadlineCoversReadingTheBody(t *testing.T) {
	const budget = 300 * time.Millisecond
	type result struct {
		readErr  error
		deadline time.Time
		hasDL    bool
		start    time.Time
		elapsed  time.Duration
	}
	done := make(chan result, 1)
	ts := httptest.NewServer(Deadline(budget, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		start := time.Now()
		_, e := io.ReadAll(r.Body)
		dl, has := r.Context().Deadline()
		done <- result{readErr: e, deadline: dl, hasDL: has, start: start, elapsed: time.Since(start)}
	})))
	defer ts.Close()

	// The sender delivers one byte and then stalls, like a slow client.
	pr, pw := io.Pipe()
	defer pw.Close()
	go pw.Write([]byte("x"))
	req, _ := http.NewRequest(http.MethodPost, ts.URL, pr)
	go func() {
		resp, e := ts.Client().Do(req)
		if e == nil {
			resp.Body.Close()
		}
	}()
	select {
	case got := <-done:
		if got.readErr == nil || !got.hasDL {
			t.Fatalf("body read was not bounded: %+v", got)
		}
		if got.elapsed > 3*budget {
			t.Fatalf("body read outlived the shared deadline: %v", got.elapsed)
		}
		// The context deadline was fixed when the request arrived, not when the handler ran.
		if got.deadline.After(got.start.Add(budget)) {
			t.Fatalf("context deadline starts after the body was read: %v", got.deadline.Sub(got.start))
		}
	case <-time.After(5 * time.Second):
		t.Fatal("stalled body was never cut off")
	}
}
