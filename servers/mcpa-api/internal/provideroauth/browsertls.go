// SPDX-License-Identifier: FSL-1.1-ALv2

package provideroauth

import (
	"errors"
	"io"
	"net"
	"net/http"
	"sync"
	"time"

	utls "github.com/refraction-networking/utls"
	"golang.org/x/net/http2"
)

// Workaround: Anthropic's OAuth token endpoint sits behind Cloudflare bot
// detection that rejects Go's default TLS fingerprint. CLIProxyAPI v6.10.9
// (internal/auth/claude/utls_transport.go, MIT, see third-party/LICENSE)
// therefore sends its Claude token requests over HTTP/2 with a Chrome
// ClientHello from refraction-networking/utls; this transport does the same
// for the Claude token exchange only. Remove it when the endpoint accepts
// standard Go TLS clients.

// browserTLSTransport opens one HTTP/2 connection per request with a
// Chrome TLS fingerprint. Token exchanges are rare, so connections are not
// pooled.
type browserTLSTransport struct {
	dialer *net.Dialer
}

// NewBrowserTLSClient returns an HTTP client whose TLS handshake looks like
// Chrome's.
func NewBrowserTLSClient(timeout time.Duration) *http.Client {
	return &http.Client{
		Timeout:   timeout,
		Transport: &browserTLSTransport{dialer: &net.Dialer{Timeout: timeout, KeepAlive: 30 * time.Second}},
	}
}

func (t *browserTLSTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	if req.URL.Scheme != "https" {
		return nil, errors.New("browser TLS transport: https is required")
	}
	host := req.URL.Hostname()
	port := req.URL.Port()
	if port == "" {
		port = "443"
	}
	raw, err := t.dialer.DialContext(req.Context(), "tcp", net.JoinHostPort(host, port))
	if err != nil {
		return nil, err
	}
	conn := utls.UClient(raw, &utls.Config{ServerName: host}, utls.HelloChrome_Auto)
	if err := conn.HandshakeContext(req.Context()); err != nil {
		raw.Close()
		return nil, err
	}
	if conn.ConnectionState().NegotiatedProtocol != "h2" {
		conn.Close()
		return nil, errors.New("browser TLS transport: the server did not negotiate HTTP/2")
	}
	cc, err := (&http2.Transport{}).NewClientConn(conn)
	if err != nil {
		conn.Close()
		return nil, err
	}
	resp, err := cc.RoundTrip(req)
	if err != nil {
		cc.Close()
		return nil, err
	}
	resp.Body = &closingBody{ReadCloser: resp.Body, close: cc.Close}
	return resp, nil
}

// closingBody closes the connection with the response body.
type closingBody struct {
	io.ReadCloser
	once  sync.Once
	close func() error
}

func (b *closingBody) Close() error {
	err := b.ReadCloser.Close()
	b.once.Do(func() { _ = b.close() })
	return err
}
