// SPDX-License-Identifier: FSL-1.1-ALv2
package main

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

func TestGatewayTransportUsesEnvironmentProxy(t *testing.T) {
	t.Setenv("HTTPS_PROXY", "http://127.0.0.1:9182")
	client := newGatewayClient()
	transport := client.Transport.(*http.Transport)
	req, _ := http.NewRequest("GET", "https://ai-gateway.vercel.sh/v1/models", nil)
	proxy, err := transport.Proxy(req)
	if err != nil || proxy == nil || proxy.String() != "http://127.0.0.1:9182" {
		t.Fatalf("proxy: %v %v", proxy, err)
	}
	if client.Timeout != 0 || transport.ResponseHeaderTimeout != 0 {
		t.Fatal("transport timeout changed")
	}
	if transport == http.DefaultTransport {
		t.Fatal("shared default transport mutated")
	}
}

func TestStageFailureDoesNotExposeDependencySecrets(t *testing.T) {
	cause := errors.New("postgres://user:secret@host/database")
	err := &stageFailure{stage: "database_connect", cause: cause}
	if err.Error() != "database_connect" || strings.Contains(err.Error(), "secret") {
		t.Fatal("unsafe diagnostic")
	}
	if !errors.Is(err, cause) {
		t.Fatal("lost error identity")
	}
}

func TestGatewayHeadersUseRequestDeadline(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		select {
		case <-time.After(50 * time.Millisecond):
			w.WriteHeader(http.StatusOK)
		case <-r.Context().Done():
		}
	}))
	defer server.Close()
	client := newGatewayClient()
	for _, tc := range []struct {
		timeout time.Duration
		pass    bool
	}{{time.Second, true}, {time.Millisecond, false}} {
		ctx, cancel := context.WithTimeout(context.Background(), tc.timeout)
		req, _ := http.NewRequestWithContext(ctx, "POST", server.URL, nil)
		response, err := client.Do(req)
		if response != nil {
			response.Body.Close()
		}
		cancel()
		if tc.pass && err != nil {
			t.Fatal("headers truncated before execution deadline", err)
		}
		if !tc.pass && !errors.Is(err, context.DeadlineExceeded) {
			t.Fatal("request deadline not enforced", err)
		}
	}
}
