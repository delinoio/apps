// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"context"
	"errors"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"runtime"
	"strings"
	"sync"
	"syscall"
	"testing"
	"time"

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

// flakyListener fails the first failures Accept calls with err, then serves
// connections from the wrapped listener. It records when each Accept call
// started so tests can observe the backoff delays.
type flakyListener struct {
	net.Listener
	err      error
	failures int
	mu       sync.Mutex
	calls    []time.Time
}

func (l *flakyListener) Accept() (net.Conn, error) {
	l.mu.Lock()
	l.calls = append(l.calls, time.Now())
	fail := len(l.calls) <= l.failures
	l.mu.Unlock()
	if fail {
		return nil, l.err
	}
	return l.Listener.Accept()
}

func (l *flakyListener) acceptTimes() []time.Time {
	l.mu.Lock()
	defer l.mu.Unlock()
	return append([]time.Time(nil), l.calls...)
}

func newTestRelay(t *testing.T, ln net.Listener, dial DialFunc) *upstreamRelay {
	t.Helper()
	return newUpstreamRelay(ln, []byte("0123456789abcdef0123456789abcdef"), dial, discardLogger(), relayIdleTimeout)
}

// A transient accept error such as EMFILE must not stop the relay: the loop
// backs off, keeps accepting and serves the next connection.
func TestUpstreamRelaySurvivesTransientAcceptErrors(t *testing.T) {
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
	inner, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	ln := &flakyListener{Listener: inner, err: syscall.EMFILE, failures: 2}
	d := &recordingDial{target: echo.Addr().String(), allow: true}
	relay := newTestRelay(t, ln, d.dial)
	defer relay.Close()

	conn, err := relayDialer(t, relay).Dial("tcp", "provider.example.test:443")
	if err != nil {
		t.Fatalf("relay did not recover from accept errors: %v", err)
	}
	defer conn.Close()
	if _, err := conn.Write([]byte("ping")); err != nil {
		t.Fatal(err)
	}
	buf := make([]byte, 4)
	if _, err := io.ReadFull(conn, buf); err != nil || string(buf) != "ping" {
		t.Fatalf("echo %q %v", buf, err)
	}
	times := ln.acceptTimes()
	if len(times) < 3 {
		t.Fatalf("accept calls %d", len(times))
	}
	if gap := times[1].Sub(times[0]); gap < relayAcceptBackoffMin {
		t.Fatalf("first backoff %v is shorter than %v", gap, relayAcceptBackoffMin)
	}
	if gap := times[2].Sub(times[1]); gap < 2*relayAcceptBackoffMin {
		t.Fatalf("second backoff %v is shorter than %v", gap, 2*relayAcceptBackoffMin)
	}
}

// Close must return promptly even while the accept loop sleeps in its
// longest backoff.
func TestUpstreamRelayCloseInterruptsBackoff(t *testing.T) {
	inner, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	ln := &flakyListener{Listener: inner, err: syscall.EMFILE, failures: 1 << 30}
	relay := newTestRelay(t, ln, (&recordingDial{allow: true}).dial)
	// Let the backoff grow towards its maximum.
	deadline := time.Now().Add(5 * time.Second)
	for len(ln.acceptTimes()) < 8 && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	if got := len(ln.acceptTimes()); got < 8 {
		t.Fatalf("accept calls %d", got)
	}
	start := time.Now()
	_ = relay.Close()
	if elapsed := time.Since(start); elapsed > 100*time.Millisecond {
		t.Fatalf("Close took %v during backoff", elapsed)
	}
}

func TestUpstreamRelayBackoffDelayIsCappedAndResets(t *testing.T) {
	var delays []time.Duration
	delay := time.Duration(0)
	for range 12 {
		delay = nextAcceptBackoff(delay)
		delays = append(delays, delay)
	}
	if delays[0] != 5*time.Millisecond || delays[1] != 10*time.Millisecond || delays[len(delays)-1] != time.Second {
		t.Fatalf("unexpected backoff sequence %v", delays)
	}
	// A successful accept resets the delay to zero, so the next failure
	// starts at the minimum again.
	if got := nextAcceptBackoff(0); got != relayAcceptBackoffMin {
		t.Fatalf("backoff after reset %v", got)
	}
}

// echoServer accepts connections and echoes until they close. It returns its
// address.
func echoServer(t *testing.T) string {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = ln.Close() })
	go func() {
		for {
			conn, err := ln.Accept()
			if err != nil {
				return
			}
			go func() {
				defer conn.Close()
				_, _ = io.Copy(conn, conn)
			}()
		}
	}()
	return ln.Addr().String()
}

func dialRelayURL(t *testing.T, rawURL, address string) net.Conn {
	t.Helper()
	u, err := url.Parse(rawURL)
	if err != nil {
		t.Fatal(err)
	}
	dialer, err := proxy.FromURL(u, proxy.Direct)
	if err != nil {
		t.Fatal(err)
	}
	conn, err := dialer.Dial("tcp", address)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = conn.Close() })
	return conn
}

func waitFor(t *testing.T, timeout time.Duration, what string, ok func() bool) {
	t.Helper()
	deadline := time.Now().Add(timeout)
	for !ok() {
		if time.Now().After(deadline) {
			t.Fatalf("timed out waiting for %s", what)
		}
		time.Sleep(5 * time.Millisecond)
	}
}

func expectEcho(t *testing.T, conn net.Conn) {
	t.Helper()
	if _, err := conn.Write([]byte("ping")); err != nil {
		t.Fatal(err)
	}
	buf := make([]byte, 4)
	if _, err := io.ReadFull(conn, buf); err != nil || string(buf) != "ping" {
		t.Fatalf("echo %q %v", buf, err)
	}
}

// CloseCall closes the tunnels of one call and no others.
func TestUpstreamRelayCloseCallClosesOnlyThatCall(t *testing.T) {
	target := echoServer(t)
	d := &recordingDial{target: target, allow: true}
	relay, err := startUpstreamRelay(d.dial, discardLogger())
	if err != nil {
		t.Fatal(err)
	}
	defer relay.Close()

	a1 := dialRelayURL(t, relay.URLFor("call-a"), "provider.example.test:443")
	a2 := dialRelayURL(t, relay.URLFor("call-a"), "provider.example.test:443")
	b := dialRelayURL(t, relay.URLFor("call-b"), "provider.example.test:443")
	shared := dialRelayURL(t, relay.URL(), "provider.example.test:443")
	for _, conn := range []net.Conn{a1, a2, b, shared} {
		expectEcho(t, conn)
	}
	if got := relay.openTunnels(); got != 4 {
		t.Fatalf("open tunnels %d, want 4", got)
	}

	relay.CloseCall("call-a")
	waitFor(t, 2*time.Second, "call-a tunnels to close", func() bool { return relay.openTunnels() == 2 })
	for name, conn := range map[string]net.Conn{"a1": a1, "a2": a2} {
		_ = conn.SetReadDeadline(time.Now().Add(2 * time.Second))
		if _, err := conn.Read(make([]byte, 1)); err == nil {
			t.Fatalf("tunnel %s is still open", name)
		}
	}
	expectEcho(t, b)
	expectEcho(t, shared)

	relay.CloseCall("call-b")
	relay.CloseCall("call-unknown")
	waitFor(t, 2*time.Second, "call-b tunnel to close", func() bool { return relay.openTunnels() == 1 })
	expectEcho(t, shared)
}

// The call username is an identifier: a wrong password is refused whatever
// the username is.
func TestUpstreamRelayRefusesCallIdentityWithWrongSecret(t *testing.T) {
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
	forged := &url.URL{Scheme: "socks5", User: url.UserPassword("call-a", "not-the-secret"), Host: u.Host}
	dialer, err := proxy.FromURL(forged, proxy.Direct)
	if err != nil {
		t.Fatal(err)
	}
	if conn, err := dialer.Dial("tcp", "provider.example.test:443"); err == nil {
		conn.Close()
		t.Fatal("forged call identity tunneled through the relay")
	}
	if got := d.addresses(); len(got) != 0 {
		t.Fatalf("forged client dialed %v", got)
	}
}

// A tunnel without traffic in either direction is closed after the idle
// timeout; traffic restarts the countdown.
func TestUpstreamRelayClosesIdleTunnels(t *testing.T) {
	target := echoServer(t)
	inner, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	const idle = 300 * time.Millisecond
	d := &recordingDial{target: target, allow: true}
	relay := newUpstreamRelay(inner, []byte("0123456789abcdef0123456789abcdef"), d.dial, discardLogger(), idle)
	defer relay.Close()

	conn := dialRelayURL(t, relay.URLFor("call-idle"), "provider.example.test:443")
	// Traffic every 100 ms keeps the tunnel open for longer than the idle
	// timeout.
	for range 6 {
		expectEcho(t, conn)
		time.Sleep(100 * time.Millisecond)
	}
	if got := relay.openTunnels(); got != 1 {
		t.Fatalf("active tunnel was closed: open %d", got)
	}
	waitFor(t, 3*time.Second, "idle tunnel to close", func() bool { return relay.openTunnels() == 0 })
	_ = conn.SetReadDeadline(time.Now().Add(2 * time.Second))
	if _, err := conn.Read(make([]byte, 1)); err == nil {
		t.Fatal("idle tunnel is still open")
	}
}

func openDescriptors(t *testing.T) int {
	t.Helper()
	for _, dir := range []string{"/proc/self/fd", "/dev/fd"} {
		if entries, err := os.ReadDir(dir); err == nil {
			return len(entries)
		}
	}
	t.Skip("descriptor listing is unavailable on this platform")
	return 0
}

// The SDK Claude client leaves its provider connection open after every
// call. Ending the call must close the tunnel, so sequential calls do not
// accumulate goroutines or descriptors.
func TestClaudeCallsDoNotLeakRelayTunnels(t *testing.T) {
	const calls = 50
	up := claudeUpstream(t, "hello")
	d := &recordingDial{target: strings.TrimPrefix(up.URL(), "http://"), allow: true}
	h := newHarness(t, func(o *Options) { o.DialUpstream = d.dial })
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	h.addCredential(apiKeyCredential(resourceID, mcpav1.Provider_PROVIDER_CLAUDE, "http://claude-upstream.example.test:80", "sk-ant-test", "claude-test"))
	body := `{"model":"claude-test","max_tokens":16,"messages":[{"role":"user","content":"hi"}]}`
	header := map[string]string{"Anthropic-Version": "2023-06-01"}

	run := func() {
		res := h.call(http.MethodPost, "/r/"+resourceID+"/v1/messages", key, body, header)
		if res.resp.StatusCode != http.StatusOK {
			t.Fatalf("status %d body %s", res.resp.StatusCode, res.body)
		}
	}
	// Warm up lazily created goroutines and descriptors before the baseline.
	run()
	waitFor(t, 2*time.Second, "warm-up tunnel to close", func() bool { return h.engine.relay.openTunnels() == 0 })
	h.client.CloseIdleConnections()
	time.Sleep(100 * time.Millisecond)
	goroutines := runtime.NumGoroutine()
	descriptors := openDescriptors(t)

	for range calls {
		run()
	}
	h.client.CloseIdleConnections()
	waitFor(t, 2*time.Second, "relay tunnels to close", func() bool { return h.engine.relay.openTunnels() == 0 })
	waitFor(t, 2*time.Second, "goroutines and descriptors to return to baseline", func() bool {
		return runtime.NumGoroutine() <= goroutines+5 && openDescriptors(t) <= descriptors+5
	})
	if got := len(d.addresses()); got < calls {
		t.Fatalf("only %d dials for %d calls: the relay was not used", got, calls)
	}
}

// A call cancelled mid-stream closes its tunnel when the call ends, and an
// in-flight stream is not interrupted before that.
func TestClaudeStreamingCallClosesTunnelWhenCancelled(t *testing.T) {
	release := make(chan struct{})
	started := make(chan struct{}, 1)
	up := newUpstream(t, func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.ReadAll(r.Body)
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "event: ping\ndata: {\"type\":\"ping\"}\n\n")
		w.(http.Flusher).Flush()
		started <- struct{}{}
		select {
		case <-release:
		case <-r.Context().Done():
		}
	})
	defer close(release)
	d := &recordingDial{target: strings.TrimPrefix(up.URL(), "http://"), allow: true}
	h := newHarness(t, func(o *Options) { o.DialUpstream = d.dial })
	resourceID := newResourceID()
	key := h.addKey(resourceID)
	h.addCredential(apiKeyCredential(resourceID, mcpav1.Provider_PROVIDER_CLAUDE, "http://claude-upstream.example.test:80", "sk-ant-test", "claude-test"))

	ctx, cancel := context.WithCancel(context.Background())
	body := `{"model":"claude-test","max_tokens":16,"stream":true,"messages":[{"role":"user","content":"hi"}]}`
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, h.srv.URL+"/r/"+resourceID+"/v1/messages", strings.NewReader(body))
	if err != nil {
		t.Fatal(err)
	}
	req.Header.Set("Authorization", "Bearer "+key)
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Anthropic-Version", "2023-06-01")
	resp, err := h.client.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	select {
	case <-started:
	case <-time.After(10 * time.Second):
		t.Fatal("upstream stream did not start")
	}
	// The stream is in flight: its tunnel stays open.
	time.Sleep(200 * time.Millisecond)
	if got := h.engine.relay.openTunnels(); got != 1 {
		t.Fatalf("open tunnels during the stream: %d, want 1", got)
	}
	cancel()
	waitFor(t, 5*time.Second, "tunnel to close after the call ended", func() bool { return h.engine.relay.openTunnels() == 0 })
}

// The dial function learns the call of a tunnel from its context, so the
// guarded dialer can log the call ID of a refused destination.
func TestUpstreamRelayDialContextCarriesCallID(t *testing.T) {
	var mu sync.Mutex
	var ids []string
	dial := func(ctx context.Context, _, _ string) (net.Conn, error) {
		mu.Lock()
		ids = append(ids, CallIDFromContext(ctx))
		mu.Unlock()
		return nil, errDialRefused
	}
	relay, err := startUpstreamRelay(dial, discardLogger())
	if err != nil {
		t.Fatal(err)
	}
	defer relay.Close()
	for _, rawURL := range []string{relay.URLFor("call-xyz"), relay.URL()} {
		u, err := url.Parse(rawURL)
		if err != nil {
			t.Fatal(err)
		}
		dialer, err := proxy.FromURL(u, proxy.Direct)
		if err != nil {
			t.Fatal(err)
		}
		if conn, err := dialer.Dial("tcp", "10.0.0.1:443"); err == nil {
			conn.Close()
			t.Fatal("refused destination was connected")
		}
	}
	mu.Lock()
	defer mu.Unlock()
	if len(ids) != 2 || ids[0] != "call-xyz" || ids[1] != "" {
		t.Fatalf("call IDs seen by the dial function: %q", ids)
	}
}
