// SPDX-License-Identifier: FSL-1.1-ALv2

package dataplane

import (
	"bytes"
	"context"
	"errors"
	"io"
	"log/slog"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/proxy"
	"golang.org/x/net/dns/dnsmessage"
)

// fakeDNS answers A queries over UDP: names in private resolve to 10.0.0.5
// and every other name is NXDOMAIN.
func fakeDNS(t *testing.T, private ...string) string {
	t.Helper()
	pc, err := net.ListenPacket("udp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = pc.Close() })
	go func() {
		buf := make([]byte, 512)
		for {
			n, addr, err := pc.ReadFrom(buf)
			if err != nil {
				return
			}
			var query dnsmessage.Message
			if query.Unpack(buf[:n]) != nil || len(query.Questions) == 0 {
				continue
			}
			question := query.Questions[0]
			reply := dnsmessage.Message{
				Header:    dnsmessage.Header{ID: query.ID, Response: true, Authoritative: true, RCode: dnsmessage.RCodeNameError},
				Questions: query.Questions,
			}
			for _, name := range private {
				if question.Name.String() == name+"." && question.Type == dnsmessage.TypeA {
					reply.RCode = dnsmessage.RCodeSuccess
					reply.Answers = []dnsmessage.Resource{{
						Header: dnsmessage.ResourceHeader{Name: question.Name, Type: dnsmessage.TypeA, Class: dnsmessage.ClassINET, TTL: 60},
						Body:   &dnsmessage.AResource{A: [4]byte{10, 0, 0, 5}},
					}}
				} else if question.Name.String() == name+"." {
					// Known name without a record of the queried type.
					reply.RCode = dnsmessage.RCodeSuccess
				}
			}
			if out, err := reply.Pack(); err == nil {
				_, _ = pc.WriteTo(out, addr)
			}
		}
	}()
	return pc.LocalAddr().String()
}

func dialerWithResolver(dnsAddr string) *GuardedDialer {
	return &GuardedDialer{dialer: &net.Dialer{
		Timeout: 5 * time.Second,
		Control: publicOnly,
		Resolver: &net.Resolver{
			PreferGo: true,
			Dial: func(ctx context.Context, _, _ string) (net.Conn, error) {
				var d net.Dialer
				return d.DialContext(ctx, "udp", dnsAddr)
			},
		},
	}}
}

type logBuffer struct {
	mu  sync.Mutex
	buf bytes.Buffer
}

func (b *logBuffer) Write(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buf.Write(p)
}

func (b *logBuffer) String() string {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buf.String()
}

func captureDefaultLog(t *testing.T) *logBuffer {
	t.Helper()
	logs := &logBuffer{}
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewTextHandler(logs, nil)))
	t.Cleanup(func() { slog.SetDefault(previous) })
	return logs
}

func TestGuardedDialerHidesInternalAddressing(t *testing.T) {
	logs := captureDefaultLog(t)
	dns := fakeDNS(t, "internal.provider.test")
	dialer := dialerWithResolver(dns)
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	t.Run("non-public address", func(t *testing.T) {
		_, err := dialer.DialContext(ctx, "tcp", "internal.provider.test:443")
		if !errors.Is(err, errProviderAddressNotAllowed) || err.Error() != "provider address is not allowed" {
			t.Fatalf("error %v", err)
		}
		if strings.Contains(err.Error(), "10.0.0.5") {
			t.Fatalf("error discloses the address: %v", err)
		}
		// The original is kept for operators.
		if out := logs.String(); !strings.Contains(out, "10.0.0.5") || !strings.Contains(out, "level=WARN") {
			t.Fatalf("original error is not logged at warn level: %s", out)
		}
	})

	t.Run("address literal", func(t *testing.T) {
		_, err := dialer.DialContext(ctx, "tcp", "127.0.0.1:443")
		if !errors.Is(err, errProviderAddressNotAllowed) {
			t.Fatalf("error %v", err)
		}
	})

	t.Run("unresolvable host", func(t *testing.T) {
		_, err := dialer.DialContext(ctx, "tcp", "missing.provider.test:443")
		if !errors.Is(err, errProviderHostUnresolved) || err.Error() != "provider host could not be resolved" {
			t.Fatalf("error %v", err)
		}
		for _, leaked := range []string{dns, "lookup", "no such host", " on ", ":53"} {
			if strings.Contains(err.Error(), leaked) {
				t.Fatalf("error discloses %q: %v", leaked, err)
			}
		}
		if out := logs.String(); !strings.Contains(out, "missing.provider.test") || !strings.Contains(out, "no such host") {
			t.Fatalf("original lookup error is not logged: %s", out)
		}
	})
}

// Other dial errors keep their text: they carry no internal addressing that
// the guard hides, and operators and customers rely on them.
func TestGuardedDialerKeepsOtherDialErrors(t *testing.T) {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	addr := ln.Addr().String()
	_ = ln.Close()
	// Without the public-address Control hook the loopback test address is
	// reachable, so the failure is a plain refused connection.
	dialer := &GuardedDialer{dialer: &net.Dialer{Timeout: 2 * time.Second}}
	_, err = dialer.DialContext(context.Background(), "tcp", addr)
	if err == nil {
		t.Fatal("dial to a closed port succeeded")
	}
	var opErr *net.OpError
	if errors.Is(err, errProviderAddressNotAllowed) || errors.Is(err, errProviderHostUnresolved) || !errors.As(err, &opErr) {
		t.Fatalf("error %v was rewritten", err)
	}
	if !strings.Contains(err.Error(), "connection refused") {
		t.Fatalf("error %v", err)
	}
}

type e2eCredentials struct {
	mu    sync.Mutex
	creds map[string][]proxy.Credential
}

func (c *e2eCredentials) LoadResourceCredentials(_ context.Context, resourceID string) ([]proxy.Credential, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	return append([]proxy.Credential(nil), c.creds[resourceID]...), nil
}

type e2eKeys struct{}

func (e2eKeys) AuthenticateCallKey(_ context.Context, resourceID, _ string) (proxy.CallPrincipal, error) {
	return proxy.CallPrincipal{ResourceID: resourceID, TenantID: "tenant", CallKeyID: "key", IssuerUserID: "user"}, nil
}

type e2eAuthorizer struct{}

func (e2eAuthorizer) AuthorizeCall(context.Context, proxy.CallPrincipal) error { return nil }

type e2eRouting struct{}

func (e2eRouting) ModelAllowlist(context.Context, string) ([]string, error) { return nil, nil }

type e2eSink struct{}

func (e2eSink) Checkpoint(context.Context, string, int64, int64, bool, proxy.CallRecordUpdate) error {
	return nil
}

// Through the real PublicTransport and PublicDialer, a provider base URL
// whose name resolves to a non-public address must produce a call error
// without the address. localhost resolves to loopback through the system
// resolver, so no resolver setup is needed.
func TestProviderCallErrorsDoNotDiscloseAddresses(t *testing.T) {
	credentials := &e2eCredentials{creds: map[string][]proxy.Credential{}}
	engine, err := proxy.New(proxy.Options{
		Credentials:         credentials,
		Keys:                e2eKeys{},
		Authorizer:          e2eAuthorizer{},
		Routing:             e2eRouting{},
		Sink:                e2eSink{},
		Logger:              quietLog(),
		CheckpointInterval:  time.Hour,
		AutoRefreshInterval: -1,
		BaseTransport:       PublicTransport(),
		DialUpstream:        PublicDialer().DialContext,
	})
	if err != nil {
		t.Fatal(err)
	}
	defer engine.Close()
	srv := httptest.NewServer(engine)
	defer srv.Close()

	cases := []struct {
		name     string
		provider mcpav1.Provider
		baseURL  string
		path     string
		body     string
		// hidden lists text that must not reach the customer. The Claude
		// path reports the relay's own loopback address in the SOCKS error,
		// which is not provider addressing and is outside this guard.
		hidden []string
	}{
		{"openai-compatible", mcpav1.Provider_PROVIDER_OPENAI_COMPATIBLE, "https://localhost:9/v1", "/v1/chat/completions", `{"model":"m","messages":[{"role":"user","content":"hi"}]}`, []string{"127.0.0.1", "[::1]", "not a public address"}},
		{"claude", mcpav1.Provider_PROVIDER_CLAUDE, "https://localhost:9", "/v1/messages", `{"model":"m","max_tokens":8,"messages":[{"role":"user","content":"hi"}]}`, []string{"not a public address", "no such host"}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			resourceID := "res-" + tc.name
			cred := proxy.Credential{
				ID: "cred-" + tc.name, ResourceID: resourceID, Provider: tc.provider,
				Kind: mcpav1.CredentialKind_CREDENTIAL_KIND_API_KEY, Secret: []byte("sk-test"),
				BaseURL: tc.baseURL, Models: []string{"m"},
			}
			credentials.mu.Lock()
			credentials.creds[resourceID] = []proxy.Credential{cred}
			credentials.mu.Unlock()
			if err := engine.UpsertCredential(context.Background(), cred); err != nil {
				t.Fatal(err)
			}
			req, err := http.NewRequest(http.MethodPost, srv.URL+"/r/"+resourceID+tc.path, strings.NewReader(tc.body))
			if err != nil {
				t.Fatal(err)
			}
			req.Header.Set("Authorization", "Bearer key")
			req.Header.Set("Content-Type", "application/json")
			req.Header.Set("Anthropic-Version", "2023-06-01")
			resp, err := http.DefaultClient.Do(req)
			if err != nil {
				t.Fatal(err)
			}
			defer resp.Body.Close()
			body, _ := io.ReadAll(resp.Body)
			if resp.StatusCode == http.StatusOK {
				t.Fatalf("call to a non-public provider succeeded: %s", body)
			}
			t.Logf("status %d body %s", resp.StatusCode, body)
			for _, leaked := range tc.hidden {
				if strings.Contains(string(body), leaked) {
					t.Fatalf("response discloses %q: %s", leaked, body)
				}
			}
		})
	}
}
