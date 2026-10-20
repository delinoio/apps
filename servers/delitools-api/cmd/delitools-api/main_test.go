// SPDX-License-Identifier: FSL-1.1-ALv2
package main

import (
	"errors"
	"net/http"
	"strings"
	"testing"
	"time"
)

func TestGatewayTransportUsesEnvironmentProxy(t *testing.T) {
	t.Setenv("HTTPS_PROXY", "http://127.0.0.1:9182")
	client := newGatewayClient(3 * time.Second)
	transport := client.Transport.(*http.Transport)
	req, _ := http.NewRequest("GET", "https://ai-gateway.vercel.sh/v1/models", nil)
	proxy, err := transport.Proxy(req)
	if err != nil || proxy == nil || proxy.String() != "http://127.0.0.1:9182" {
		t.Fatalf("proxy: %v %v", proxy, err)
	}
	if client.Timeout != 0 || transport.ResponseHeaderTimeout != 3*time.Second {
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
