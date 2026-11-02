// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"context"
	"errors"
	"io"
	"net"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"testing"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"golang.org/x/net/proxy"
)

// recordingDial records requested addresses and connects every allowed one
// to target.
type recordingDial struct {
	target string
	allow  bool
	mu     sync.Mutex
	seen   []string
}

var errDialRefused = errors.New("destination refused by test policy")

func (d *recordingDial) dial(ctx context.Context, network, address string) (net.Conn, error) {
	d.mu.Lock()
	d.seen = append(d.seen, address)
	d.mu.Unlock()
	if !d.allow {
		return nil, errDialRefused
	}
	var dialer net.Dialer
	return dialer.DialContext(ctx, network, d.target)
}

func (d *recordingDial) addresses() []string {
	d.mu.Lock()
	defer d.mu.Unlock()
	return append([]string(nil), d.seen...)
}

func relayDialer(t *testing.T, relay *upstreamRelay) proxy.Dialer {
	t.Helper()
	u, err := url.Parse(relay.URL())
	if err != nil {
		t.Fatal(err)
	}
	dialer, err := proxy.FromURL(u, proxy.Direct)
	if err != nil {
		t.Fatal(err)
	}
	return dialer
}

func TestUpstreamRelayPassesNamesToDialFunc(t *testing.T) {
	echo, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer echo.Close()
	go func() {
		conn, err := echo.Accept()
		if err != nil {
			return
		}
		defer conn.Close()
		_, _ = io.Copy(conn, conn)
	}()
	d := &recordingDial{target: echo.Addr().String(), allow: true}
	relay, err := startUpstreamRelay(d.dial, discardLogger())
	if err != nil {
		t.Fatal(err)
	}
	defer relay.Close()
	if u, err := url.Parse(relay.URL()); err != nil || u.Scheme != "socks5" || u.Hostname() != "127.0.0.1" {
		t.Fatalf("relay URL is not a loopback SOCKS5 URL: %v", err)
	}
	conn, err := relayDialer(t, relay).Dial("tcp", "provider.example.test:443")
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	if _, err := conn.Write([]byte("ping")); err != nil {
		t.Fatal(err)
	}
	buf := make([]byte, 4)
	if _, err := io.ReadFull(conn, buf); err != nil || string(buf) != "ping" {
		t.Fatalf("echo %q %v", buf, err)
	}
	// The relay must not resolve the name itself: the dial function owns
	// resolution and the destination check.
	if got := d.addresses(); len(got) != 1 || got[0] != "provider.example.test:443" {
		t.Fatalf("dialed %v", got)
	}
}

func TestUpstreamRelayFailsClosedWhenDialIsRefused(t *testing.T) {
	d := &recordingDial{allow: false}
	relay, err := startUpstreamRelay(d.dial, discardLogger())
	if err != nil {
		t.Fatal(err)
	}
	defer relay.Close()
	if conn, err := relayDialer(t, relay).Dial("tcp", "10.0.0.1:443"); err == nil {
		conn.Close()
		t.Fatal("refused destination was connected")
	}
	if got := d.addresses(); len(got) != 1 || got[0] != "10.0.0.1:443" {
		t.Fatalf("dialed %v", got)
	}
}

func TestUpstreamRelayRejectsUnsupportedRequests(t *testing.T) {
	d := &recordingDial{allow: true}
	relay, err := startUpstreamRelay(d.dial, discardLogger())
	if err != nil {
		t.Fatal(err)
	}
	defer relay.Close()
	u, err := url.Parse(relay.URL())
	if err != nil {
		t.Fatal(err)
	}
	addr := u.Host
	password, _ := u.User.Password()
	reply := make([]byte, 2)
	connect := func() net.Conn {
		t.Helper()
		conn, err := net.Dial("tcp", addr)
		if err != nil {
			t.Fatal(err)
		}
		return conn
	}
	authenticate := func(conn net.Conn, username, password string) byte {
		t.Helper()
		_, _ = conn.Write([]byte{socksVersion5, 1, socksMethodUserPass})
		if _, err := io.ReadFull(conn, reply); err != nil || reply[1] != socksMethodUserPass {
			t.Fatalf("method reply %v %v", reply, err)
		}
		msg := append([]byte{socksUserPassVersion, byte(len(username))}, username...)
		msg = append(append(msg, byte(len(password))), password...)
		_, _ = conn.Write(msg)
		if _, err := io.ReadFull(conn, reply); err != nil {
			t.Fatalf("authentication reply: %v", err)
		}
		return reply[1]
	}

	// No authentication only: the relay offers no acceptable method.
	conn := connect()
	_, _ = conn.Write([]byte{socksVersion5, 1, socksMethodNoAuth})
	if _, err := io.ReadFull(conn, reply); err != nil || reply[1] != socksMethodNone {
		t.Fatalf("method reply %v %v", reply, err)
	}
	conn.Close()

	// A wrong password is refused before any request is read.
	conn = connect()
	if status := authenticate(conn, "mcpa", strings.Repeat("0", len(password))); status != socksUserPassFailed {
		t.Fatalf("wrong password status %d", status)
	}
	conn.Close()

	// BIND is not supported.
	conn = connect()
	defer conn.Close()
	if status := authenticate(conn, u.User.Username(), password); status != socksUserPassOK {
		t.Fatalf("authentication status %d", status)
	}
	_, _ = conn.Write([]byte{socksVersion5, 0x02, 0x00, socksAddrIPv4, 8, 8, 8, 8, 0x01, 0xbb})
	resp := make([]byte, 10)
	if _, err := io.ReadFull(conn, resp); err != nil || resp[1] != socksReplyCmdUnknown {
		t.Fatalf("bind reply %v %v", resp, err)
	}
	if got := d.addresses(); len(got) != 0 {
		t.Fatalf("rejected requests dialed %v", got)
	}
}

// Only holders of the per-relay secret can use the relay: a local process
// without it, or with the secret of another relay, cannot tunnel through it.
func TestUpstreamRelayRequiresItsSecret(t *testing.T) {
	d := &recordingDial{allow: true}
	relay, err := startUpstreamRelay(d.dial, discardLogger())
	if err != nil {
		t.Fatal(err)
	}
	defer relay.Close()
	other, err := startUpstreamRelay(d.dial, discardLogger())
	if err != nil {
		t.Fatal(err)
	}
	defer other.Close()
	if relay.URL() == other.URL() {
		t.Fatal("two relays share a URL")
	}
	u, err := url.Parse(relay.URL())
	if err != nil {
		t.Fatal(err)
	}
	if u.User == nil {
		t.Fatal("relay URL carries no credentials")
	}
	otherURL, err := url.Parse(other.URL())
	if err != nil {
		t.Fatal(err)
	}
	anonymous := &url.URL{Scheme: "socks5", Host: u.Host}
	foreign := &url.URL{Scheme: "socks5", User: otherURL.User, Host: u.Host}
	for name, target := range map[string]*url.URL{"anonymous": anonymous, "foreign secret": foreign} {
		dialer, err := proxy.FromURL(target, proxy.Direct)
		if err != nil {
			t.Fatal(err)
		}
		if conn, err := dialer.Dial("tcp", "provider.example.test:443"); err == nil {
			conn.Close()
			t.Fatalf("%s client tunneled through the relay", name)
		}
	}
	if got := d.addresses(); len(got) != 0 {
		t.Fatalf("unauthenticated clients dialed %v", got)
	}
}

// The SDK Claude client ignores BaseTransport; its dials must go through
// DialUpstream so the provider address guard applies to customer base URLs.
func TestClaudeUpstreamDialsThroughDialUpstream(t *testing.T) {
	up := claudeUpstream(t, "hello guarded claude")
	_, port, err := net.SplitHostPort(strings.TrimPrefix(up.URL(), "http://"))
	if err != nil {
		t.Fatal(err)
	}
	baseURL := "http://claude-upstream.example.test:" + port
	body := `{"model":"claude-test","max_tokens":16,"messages":[{"role":"user","content":"hi"}]}`
	header := map[string]string{"Anthropic-Version": "2023-06-01"}

	t.Run("allowed", func(t *testing.T) {
		d := &recordingDial{target: strings.TrimPrefix(up.URL(), "http://"), allow: true}
		h := newHarness(t, func(o *Options) { o.DialUpstream = d.dial })
		resourceID := newResourceID()
		key := h.addKey(resourceID)
		h.addCredential(apiKeyCredential(resourceID, mcpav1.Provider_PROVIDER_CLAUDE, baseURL, "sk-ant-test", "claude-test"))
		res := h.call(http.MethodPost, "/r/"+resourceID+"/v1/messages", key, body, header)
		if res.resp.StatusCode != http.StatusOK || !strings.Contains(string(res.body), "hello guarded claude") {
			t.Fatalf("status %d body %s", res.resp.StatusCode, res.body)
		}
		if got := d.addresses(); len(got) == 0 || got[0] != "claude-upstream.example.test:"+port {
			t.Fatalf("dialed %v", got)
		}
	})

	t.Run("refused", func(t *testing.T) {
		before := up.hits.Load()
		d := &recordingDial{allow: false}
		h := newHarness(t, func(o *Options) { o.DialUpstream = d.dial })
		resourceID := newResourceID()
		key := h.addKey(resourceID)
		h.addCredential(apiKeyCredential(resourceID, mcpav1.Provider_PROVIDER_CLAUDE, baseURL, "sk-ant-test", "claude-test"))
		res := h.call(http.MethodPost, "/r/"+resourceID+"/v1/messages", key, body, header)
		if res.resp.StatusCode == http.StatusOK {
			t.Fatalf("refused destination served the call: %s", res.body)
		}
		if got := up.hits.Load() - before; got != 0 {
			t.Fatalf("upstream received %d requests", got)
		}
		if got := d.addresses(); len(got) == 0 || got[0] != "claude-upstream.example.test:"+port {
			t.Fatalf("dialed %v", got)
		}
		record := h.sink.waitFinal(t, res.callID)
		if record.Status == mcpav1.CallStatus_CALL_STATUS_SUCCEEDED {
			t.Fatalf("record %+v", record)
		}
	})
}
