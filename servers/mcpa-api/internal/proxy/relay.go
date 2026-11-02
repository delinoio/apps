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
// host as the provider URL names it; the function resolves it.
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
	url      string
	username []byte
	password []byte
	done     chan struct{}
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
	const username = "mcpa"
	password := hex.EncodeToString(secret)
	u := url.URL{Scheme: "socks5", User: url.UserPassword(username, password), Host: ln.Addr().String()}
	r := &upstreamRelay{
		ln:       ln,
		dial:     dial,
		logger:   logger,
		url:      u.String(),
		username: []byte(username),
		password: []byte(password),
		done:     make(chan struct{}),
	}
	go r.serve()
	return r, nil
}

// URL returns the SOCKS5 proxy URL of the relay, including its
// credentials. Never log it.
func (r *upstreamRelay) URL() string { return r.url }

// Close stops accepting connections. Established tunnels finish with their
// calls, matching Engine.Close, which does not interrupt in-flight calls.
func (r *upstreamRelay) Close() error {
	err := r.ln.Close()
	<-r.done
	return err
}

func (r *upstreamRelay) serve() {
	defer close(r.done)
	for {
		conn, err := r.ln.Accept()
		if err != nil {
			if !errors.Is(err, net.ErrClosed) {
				r.logger.Error("upstream relay accept failed", slog.String("error", err.Error()))
			}
			return
		}
		go r.handle(conn)
	}
}

func (r *upstreamRelay) handle(client net.Conn) {
	ctx, cancel := context.WithTimeout(context.Background(), relayHandshakeTimeout)
	defer cancel()
	_ = client.SetDeadline(time.Now().Add(relayHandshakeTimeout))
	address, reply, err := r.readSocksConnect(client)
	if err != nil {
		if reply != 0 {
			_, _ = client.Write(socksReply(reply))
		}
		r.logger.Warn("upstream relay rejected a request", slog.String("error", err.Error()))
		_ = client.Close()
		return
	}
	upstream, err := r.dial(ctx, "tcp", address)
	if err != nil {
		_, _ = client.Write(socksReply(socksReplyFailure))
		r.logger.Warn("upstream relay dial refused or failed",
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
	splice(client, upstream)
}

// readSocksConnect reads a SOCKS5 greeting, the username/password
// sub-negotiation and a CONNECT request (RFC 1928 and RFC 1929) and returns
// the destination. On failure it returns the reply code to send, or zero
// when no reply is possible.
func (r *upstreamRelay) readSocksConnect(conn net.Conn) (string, byte, error) {
	var head [2]byte
	if _, err := io.ReadFull(conn, head[:]); err != nil {
		return "", 0, fmt.Errorf("read greeting: %w", err)
	}
	if head[0] != socksVersion5 {
		return "", 0, fmt.Errorf("unsupported SOCKS version %d", head[0])
	}
	methods := make([]byte, head[1])
	if _, err := io.ReadFull(conn, methods); err != nil {
		return "", 0, fmt.Errorf("read methods: %w", err)
	}
	userPass := false
	for _, method := range methods {
		if method == socksMethodUserPass {
			userPass = true
		}
	}
	if !userPass {
		_, _ = conn.Write([]byte{socksVersion5, socksMethodNone})
		return "", 0, errors.New("client offered no supported authentication method")
	}
	if _, err := conn.Write([]byte{socksVersion5, socksMethodUserPass}); err != nil {
		return "", 0, fmt.Errorf("write method: %w", err)
	}
	if err := r.authenticate(conn); err != nil {
		return "", 0, err
	}
	var req [4]byte
	if _, err := io.ReadFull(conn, req[:]); err != nil {
		return "", 0, fmt.Errorf("read request: %w", err)
	}
	if req[0] != socksVersion5 {
		return "", 0, fmt.Errorf("unsupported SOCKS version %d", req[0])
	}
	if req[1] != socksCmdConnect {
		return "", socksReplyCmdUnknown, fmt.Errorf("unsupported SOCKS command %d", req[1])
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
			return "", 0, fmt.Errorf("read address: %w", err)
		}
		host = ip.String()
	case socksAddrDomain:
		var size [1]byte
		if _, err := io.ReadFull(conn, size[:]); err != nil {
			return "", 0, fmt.Errorf("read address: %w", err)
		}
		name := make([]byte, size[0])
		if _, err := io.ReadFull(conn, name); err != nil {
			return "", 0, fmt.Errorf("read address: %w", err)
		}
		host = string(name)
	default:
		return "", socksReplyAddrType, fmt.Errorf("unsupported SOCKS address type %d", req[3])
	}
	var port [2]byte
	if _, err := io.ReadFull(conn, port[:]); err != nil {
		return "", 0, fmt.Errorf("read port: %w", err)
	}
	return net.JoinHostPort(host, strconv.Itoa(int(binary.BigEndian.Uint16(port[:])))), 0, nil
}

// authenticate runs the RFC 1929 username/password sub-negotiation and
// compares both values in constant time.
func (r *upstreamRelay) authenticate(conn net.Conn) error {
	var head [2]byte
	if _, err := io.ReadFull(conn, head[:]); err != nil {
		return fmt.Errorf("read authentication: %w", err)
	}
	if head[0] != socksUserPassVersion {
		return fmt.Errorf("unsupported authentication version %d", head[0])
	}
	username := make([]byte, head[1])
	if _, err := io.ReadFull(conn, username); err != nil {
		return fmt.Errorf("read authentication: %w", err)
	}
	var size [1]byte
	if _, err := io.ReadFull(conn, size[:]); err != nil {
		return fmt.Errorf("read authentication: %w", err)
	}
	password := make([]byte, size[0])
	if _, err := io.ReadFull(conn, password); err != nil {
		return fmt.Errorf("read authentication: %w", err)
	}
	userOK := subtle.ConstantTimeCompare(username, r.username)
	passOK := subtle.ConstantTimeCompare(password, r.password)
	if userOK&passOK != 1 {
		_, _ = conn.Write([]byte{socksUserPassVersion, socksUserPassFailed})
		return errors.New("client credentials do not match the relay")
	}
	if _, err := conn.Write([]byte{socksUserPassVersion, socksUserPassOK}); err != nil {
		return fmt.Errorf("write authentication: %w", err)
	}
	return nil
}

// socksReply builds a reply with an unspecified bound address; the SDK
// clients do not use it.
func socksReply(code byte) []byte {
	return []byte{socksVersion5, code, 0x00, socksAddrIPv4, 0, 0, 0, 0, 0, 0}
}

// splice copies both directions until both end, then closes the
// connections.
func splice(a, b net.Conn) {
	var wg sync.WaitGroup
	wg.Add(2)
	pipe := func(dst, src net.Conn) {
		defer wg.Done()
		_, _ = io.Copy(dst, src)
		if tcp, ok := dst.(interface{ CloseWrite() error }); ok {
			_ = tcp.CloseWrite()
		} else {
			_ = dst.Close()
		}
	}
	go pipe(a, b)
	go pipe(b, a)
	wg.Wait()
	_ = a.Close()
	_ = b.Close()
}
