// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"context"
	"crypto/rand"
	"crypto/subtle"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net"
	"net/url"
	"strconv"
	"sync"
	"time"
)

// DialFunc dials a provider connection. address is "host:port" with the
// host as the provider URL names it; the function resolves it. The error
// text may reach customers (the SDK copies it into its response), so it must
// not disclose resolved addresses or resolver details; the context carries
// the call (see CallIDFromContext) for internal logging.
type DialFunc func(ctx context.Context, network, address string) (net.Conn, error)

// upstreamRelay applies Options.DialUpstream to the SDK Claude client.
//
// Workaround: the SDK Claude executor builds a private client per request
// with helps.NewUtlsHTTPClient (internal/runtime/executor/claude_executor.go
// lines 126, 228, 402 and 637 of SDK v6.10.9). That client ignores the
// context RoundTripper that carries BaseTransport and dials with its own
// net.Dialer (utls_client.go: proxy.Direct for api.anthropic.com and a plain
// http.Transport for every other host), so a customer base URL whose name
// resolves to a private address would bypass the provider address guard.
// The only dial hook the client honours is the auth's ProxyURL, which it
// passes to golang.org/x/net/proxy (uTLS path) and to
// sdk/proxyutil.BuildHTTPTransport (other hosts). Both accept socks5 URLs and
// send the destination host name unresolved (x/net internal/socks client.go),
// so this loopback SOCKS5 relay receives the name and performs the dial,
// including DNS resolution, through DialUpstream. resourceExecutor sets the
// relay as the ProxyURL of every Claude credential before the SDK executor
// runs.
//
// Scope: SDK Claude executor calls only (Execute, ExecuteStream, CountTokens
// and HttpRequest). Remove the relay when the SDK Claude client accepts the
// context RoundTripper or another injectable transport or dialer.
//
// The relay listens on loopback only and requires SOCKS5 username/password
// authentication (RFC 1929) with a random secret generated per relay. The
// secret exists only in this process (in the relay URL that resourceExecutor
// hands to the SDK), so other local processes cannot use the relay as an
// egress proxy. It only implements CONNECT and only reaches destinations
// that DialUpstream permits.
type upstreamRelay struct {
	ln       net.Listener
	dial     DialFunc
	logger   *slog.Logger
	host     string
	password []byte
	done     chan struct{}
	// closing is closed by Close so the accept loop stops promptly even
	// while it sleeps in an accept-error backoff.
	closing   chan struct{}
	closeOnce sync.Once

	// idleTimeout closes a tunnel without traffic in either direction.
	idleTimeout time.Duration
	mu          sync.Mutex
	// calls holds the open tunnels of each call that connected with a call
	// identity (see CloseCall). open counts every open tunnel, with or
	// without identity.
	calls map[string]map[*tunnel]struct{}
	open  int
}

const (
	socksVersion5        = 0x05
	socksMethodNoAuth    = 0x00
	socksMethodUserPass  = 0x02
	socksMethodNone      = 0xff
	socksUserPassVersion = 0x01
	socksUserPassOK      = 0x00
	socksUserPassFailed  = 0x01
	socksCmdConnect      = 0x01
	socksAddrIPv4        = 0x01
	socksAddrDomain      = 0x03
	socksAddrIPv6        = 0x04
	socksReplySucceeded  = 0x00
	socksReplyFailure    = 0x01
	socksReplyCmdUnknown = 0x07
	socksReplyAddrType   = 0x08

	// relayHandshakeTimeout bounds the SOCKS handshake and the upstream
	// dial. The dialer has its own, shorter timeout in production.
	relayHandshakeTimeout = 45 * time.Second

	// The accept loop backs off after a transient accept error such as
	// EMFILE, ENFILE or ENOMEM: it starts at relayAcceptBackoffMin, doubles
	// per consecutive failure up to relayAcceptBackoffMax and resets after
	// a successful accept.
	relayAcceptBackoffMin = 5 * time.Millisecond
	relayAcceptBackoffMax = time.Second

	// relayIdleTimeout is the default time after which a tunnel without
	// traffic in either direction is closed. It is a backstop for tunnels
	// that CloseCall does not reach.
	relayIdleTimeout = 5 * time.Minute

	// relaySharedUsername is the SOCKS username of connections that carry
	// no call identity. The password is always the per-relay secret.
	relaySharedUsername = "mcpa"
)

func startUpstreamRelay(dial DialFunc, logger *slog.Logger) (*upstreamRelay, error) {
	secret := make([]byte, 32)
	if _, err := rand.Read(secret); err != nil {
		return nil, fmt.Errorf("generate upstream relay secret: %w", err)
	}
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return nil, fmt.Errorf("listen upstream relay: %w", err)
	}
	return newUpstreamRelay(ln, secret, dial, logger, relayIdleTimeout), nil
}

// newUpstreamRelay serves the relay on ln. It is separate from
// startUpstreamRelay so tests can inject a listener that fails on demand.
func newUpstreamRelay(ln net.Listener, secret []byte, dial DialFunc, logger *slog.Logger, idleTimeout time.Duration) *upstreamRelay {
	password := hex.EncodeToString(secret)
	r := &upstreamRelay{
		ln:          ln,
		dial:        dial,
		logger:      logger,
		host:        ln.Addr().String(),
		password:    []byte(password),
		done:        make(chan struct{}),
		closing:     make(chan struct{}),
		idleTimeout: idleTimeout,
		calls:       map[string]map[*tunnel]struct{}{},
	}
	go r.serve()
	return r
}

// nextAcceptBackoff returns the delay after a failed accept given the previous
// delay, zero meaning the previous accept succeeded.
func nextAcceptBackoff(previous time.Duration) time.Duration {
	if previous <= 0 {
		return relayAcceptBackoffMin
	}
	return min(previous*2, relayAcceptBackoffMax)
}

func sleepUnlessClosing(d time.Duration, closing <-chan struct{}) {
	timer := time.NewTimer(d)
	defer timer.Stop()
	select {
	case <-timer.C:
	case <-closing:
	}
}

// URL returns the SOCKS5 proxy URL of the relay for connections without a
// call identity, including its credentials. Never log it. Such tunnels end
// only when their endpoints close or after the idle timeout.
func (r *upstreamRelay) URL() string { return r.urlFor("") }

// URLFor returns the SOCKS5 proxy URL for one call, including its
// credentials. The SOCKS username is the call ID, which is an identifier and
// not a secret; the password is the per-relay secret. Never log the URL.
// Tunnels opened through it close with CloseCall(callID).
func (r *upstreamRelay) URLFor(callID string) string { return r.urlFor(callID) }

func (r *upstreamRelay) urlFor(callID string) string {
	username := callID
	if username == "" {
		username = relaySharedUsername
	}
	u := url.URL{Scheme: "socks5", User: url.UserPassword(username, string(r.password)), Host: r.host}
	return u.String()
}

// CloseCall closes every open tunnel of the call. Engine.ServeHTTP calls it
// when the call ends: the SDK Claude client builds a new client for every
// request and never closes its idle provider connections, so without it each
// call would leave its connection, two descriptors and two goroutines
// behind until the provider closed it.
func (r *upstreamRelay) CloseCall(callID string) {
	r.mu.Lock()
	set := r.calls[callID]
	delete(r.calls, callID)
	tunnels := make([]*tunnel, 0, len(set))
	for t := range set {
		tunnels = append(tunnels, t)
	}
	r.mu.Unlock()
	for _, t := range tunnels {
		t.close()
	}
}

// openTunnels returns the number of open tunnels.
func (r *upstreamRelay) openTunnels() int {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.open
}

// Close stops accepting connections. Established tunnels finish with their
// calls, matching Engine.Close, which does not interrupt in-flight calls.
func (r *upstreamRelay) Close() error {
	r.closeOnce.Do(func() { close(r.closing) })
	err := r.ln.Close()
	<-r.done
	return err
}

// serve accepts connections until the listener is closed. A transient accept
// error (descriptor or memory exhaustion) must not stop the relay for good:
// the kernel keeps queueing connections in the backlog while nothing accepts
// them, and every Claude call would hang. The loop backs off and retries;
// only net.ErrClosed ends it.
func (r *upstreamRelay) serve() {
	defer close(r.done)
	var delay time.Duration
	for {
		conn, err := r.ln.Accept()
		if err != nil {
			if errors.Is(err, net.ErrClosed) {
				return
			}
			delay = nextAcceptBackoff(delay)
			r.logger.Warn("upstream relay accept failed; retrying",
				slog.String("error", err.Error()),
				slog.Duration("delay", delay))
			sleepUnlessClosing(delay, r.closing)
			select {
			case <-r.closing:
				return
			default:
			}
			continue
		}
		delay = 0
		go r.handle(conn)
	}
}

func (r *upstreamRelay) handle(client net.Conn) {
	ctx, cancel := context.WithTimeout(context.Background(), relayHandshakeTimeout)
	defer cancel()
	_ = client.SetDeadline(time.Now().Add(relayHandshakeTimeout))
	address, callID, reply, err := r.readSocksConnect(client)
	if err != nil {
		if reply != 0 {
			_, _ = client.Write(socksReply(reply))
		}
		r.logger.Warn("upstream relay rejected a request", slog.String("error", err.Error()))
		_ = client.Close()
		return
	}
	if callID != "" {
		ctx = context.WithValue(ctx, relayCallIDKey{}, callID)
	}
	upstream, err := r.dial(ctx, "tcp", address)
	if err != nil {
		_, _ = client.Write(socksReply(socksReplyFailure))
		r.logger.Warn("upstream relay dial refused or failed",
			slog.String("call_id", callID),
			slog.String("address", address),
			slog.String("error", err.Error()))
		_ = client.Close()
		return
	}
	if _, err := client.Write(socksReply(socksReplySucceeded)); err != nil {
		_ = client.Close()
		_ = upstream.Close()
		return
	}
	_ = client.SetDeadline(time.Time{})
	t := r.openTunnel(callID, client, upstream)
	t.run()
}

// readSocksConnect reads a SOCKS5 greeting, the username/password
// sub-negotiation and a CONNECT request (RFC 1928 and RFC 1929) and returns
// the destination and the call ID of the SOCKS username, empty for the
// shared username. On failure it returns the reply code to send, or zero when
// no reply is possible.
func (r *upstreamRelay) readSocksConnect(conn net.Conn) (string, string, byte, error) {
	var head [2]byte
	if _, err := io.ReadFull(conn, head[:]); err != nil {
		return "", "", 0, fmt.Errorf("read greeting: %w", err)
	}
	if head[0] != socksVersion5 {
		return "", "", 0, fmt.Errorf("unsupported SOCKS version %d", head[0])
	}
	methods := make([]byte, head[1])
	if _, err := io.ReadFull(conn, methods); err != nil {
		return "", "", 0, fmt.Errorf("read methods: %w", err)
	}
	userPass := false
	for _, method := range methods {
		if method == socksMethodUserPass {
			userPass = true
		}
	}
	if !userPass {
		_, _ = conn.Write([]byte{socksVersion5, socksMethodNone})
		return "", "", 0, errors.New("client offered no supported authentication method")
	}
	if _, err := conn.Write([]byte{socksVersion5, socksMethodUserPass}); err != nil {
		return "", "", 0, fmt.Errorf("write method: %w", err)
	}
	callID, err := r.authenticate(conn)
	if err != nil {
		return "", "", 0, err
	}
	var req [4]byte
	if _, err := io.ReadFull(conn, req[:]); err != nil {
		return "", "", 0, fmt.Errorf("read request: %w", err)
	}
	if req[0] != socksVersion5 {
		return "", "", 0, fmt.Errorf("unsupported SOCKS version %d", req[0])
	}
	if req[1] != socksCmdConnect {
		return "", "", socksReplyCmdUnknown, fmt.Errorf("unsupported SOCKS command %d", req[1])
	}
	var host string
	switch req[3] {
	case socksAddrIPv4, socksAddrIPv6:
		size := net.IPv4len
		if req[3] == socksAddrIPv6 {
			size = net.IPv6len
		}
		ip := make(net.IP, size)
		if _, err := io.ReadFull(conn, ip); err != nil {
			return "", "", 0, fmt.Errorf("read address: %w", err)
		}
		host = ip.String()
	case socksAddrDomain:
		var size [1]byte
		if _, err := io.ReadFull(conn, size[:]); err != nil {
			return "", "", 0, fmt.Errorf("read address: %w", err)
		}
		name := make([]byte, size[0])
		if _, err := io.ReadFull(conn, name); err != nil {
			return "", "", 0, fmt.Errorf("read address: %w", err)
		}
		host = string(name)
	default:
		return "", "", socksReplyAddrType, fmt.Errorf("unsupported SOCKS address type %d", req[3])
	}
	var port [2]byte
	if _, err := io.ReadFull(conn, port[:]); err != nil {
		return "", "", 0, fmt.Errorf("read port: %w", err)
	}
	return net.JoinHostPort(host, strconv.Itoa(int(binary.BigEndian.Uint16(port[:])))), callID, 0, nil
}

// authenticate runs the RFC 1929 username/password sub-negotiation and
// compares the password in constant time. It returns the call ID the
// username carries, empty for the shared username. The username is an
// identifier; only the password authenticates the client.
func (r *upstreamRelay) authenticate(conn net.Conn) (string, error) {
	var head [2]byte
	if _, err := io.ReadFull(conn, head[:]); err != nil {
		return "", fmt.Errorf("read authentication: %w", err)
	}
	if head[0] != socksUserPassVersion {
		return "", fmt.Errorf("unsupported authentication version %d", head[0])
	}
	username := make([]byte, head[1])
	if _, err := io.ReadFull(conn, username); err != nil {
		return "", fmt.Errorf("read authentication: %w", err)
	}
	var size [1]byte
	if _, err := io.ReadFull(conn, size[:]); err != nil {
		return "", fmt.Errorf("read authentication: %w", err)
	}
	password := make([]byte, size[0])
	if _, err := io.ReadFull(conn, password); err != nil {
		return "", fmt.Errorf("read authentication: %w", err)
	}
	if subtle.ConstantTimeCompare(password, r.password) != 1 || len(username) == 0 {
		_, _ = conn.Write([]byte{socksUserPassVersion, socksUserPassFailed})
		return "", errors.New("client credentials do not match the relay")
	}
	if _, err := conn.Write([]byte{socksUserPassVersion, socksUserPassOK}); err != nil {
		return "", fmt.Errorf("write authentication: %w", err)
	}
	if string(username) == relaySharedUsername {
		return "", nil
	}
	return string(username), nil
}

// socksReply builds a reply with an unspecified bound address; the SDK
// clients do not use it.
func socksReply(code byte) []byte {
	return []byte{socksVersion5, code, 0x00, socksAddrIPv4, 0, 0, 0, 0, 0, 0}
}

// tunnel is one established relay connection pair.
type tunnel struct {
	relay    *upstreamRelay
	callID   string
	client   net.Conn
	upstream net.Conn

	idle      time.Duration
	timer     *time.Timer
	closeOnce sync.Once
}

// openTunnel registers a tunnel for callID (empty for none) and starts its
// idle timer.
func (r *upstreamRelay) openTunnel(callID string, client, upstream net.Conn) *tunnel {
	t := &tunnel{relay: r, callID: callID, client: client, upstream: upstream, idle: r.idleTimeout}
	r.mu.Lock()
	r.open++
	if callID != "" {
		set := r.calls[callID]
		if set == nil {
			set = map[*tunnel]struct{}{}
			r.calls[callID] = set
		}
		set[t] = struct{}{}
	}
	r.mu.Unlock()
	t.timer = time.AfterFunc(t.idle, func() {
		r.logger.Debug("upstream relay tunnel idle timeout", slog.String("call_id", callID))
		t.close()
	})
	return t
}

// touch restarts the idle countdown after traffic in either direction.
func (t *tunnel) touch() { t.timer.Reset(t.idle) }

// close closes both connections and unregisters the tunnel. It is safe to
// call more than once and concurrently.
func (t *tunnel) close() {
	t.closeOnce.Do(func() {
		t.timer.Stop()
		_ = t.client.Close()
		_ = t.upstream.Close()
		r := t.relay
		r.mu.Lock()
		r.open--
		if set := r.calls[t.callID]; set != nil {
			delete(set, t)
			if len(set) == 0 {
				delete(r.calls, t.callID)
			}
		}
		r.mu.Unlock()
	})
}

// activityConn reports traffic to its tunnel.
type activityConn struct {
	net.Conn
	t *tunnel
}

func (c activityConn) Read(p []byte) (int, error) {
	n, err := c.Conn.Read(p)
	if n > 0 {
		c.t.touch()
	}
	return n, err
}

func (c activityConn) Write(p []byte) (int, error) {
	n, err := c.Conn.Write(p)
	if n > 0 {
		c.t.touch()
	}
	return n, err
}

// run copies both directions until both end or the tunnel is closed, then
// closes the connections.
func (t *tunnel) run() {
	a := activityConn{Conn: t.client, t: t}
	b := activityConn{Conn: t.upstream, t: t}
	var wg sync.WaitGroup
	wg.Add(2)
	pipe := func(dst, src activityConn) {
		defer wg.Done()
		_, _ = io.Copy(dst, src)
		if tcp, ok := dst.Conn.(interface{ CloseWrite() error }); ok {
			_ = tcp.CloseWrite()
		} else {
			_ = dst.Close()
		}
	}
	go pipe(a, b)
	go pipe(b, a)
	wg.Wait()
	t.close()
}
