// SPDX-License-Identifier: FSL-1.1-ALv2

package dataplane

import (
	"context"
	"errors"
	"log/slog"
	"net"
	"net/http"
	"net/netip"
	"syscall"
	"time"

	"github.com/delinoio/apps/servers/mcpa-api/internal/credential"
	"github.com/delinoio/apps/servers/mcpa-api/internal/proxy"
)

// errNonPublicAddress is returned when a provider host resolves to a
// private, loopback, link-local or otherwise non-public address.
var errNonPublicAddress = errors.New("provider address is not a public address")

// The errors below are the only dial failures whose text reaches customers.
// The SDK copies provider dial errors into its response body, and the raw
// errors name the resolved address and the resolver
// ("dial tcp 10.0.0.5:443: ...", "lookup host on 10.0.0.2:53: no such host"),
// which would let a customer map the internal network through a crafted
// base URL.
var (
	errProviderAddressNotAllowed = errors.New("provider address is not allowed")
	errProviderHostUnresolved    = errors.New("provider host could not be resolved")
)

// publicOnly is a net.Dialer Control function that refuses every
// non-public destination after DNS resolution, so customer-supplied
// provider base URLs cannot reach internal services.
func publicOnly(_, address string, _ syscall.RawConn) error {
	addrPort, err := netip.ParseAddrPort(address)
	if err != nil {
		return errNonPublicAddress
	}
	if !credential.PublicAddr(addrPort.Addr()) {
		return errNonPublicAddress
	}
	return nil
}

// GuardedDialer dials provider connections. The embedded dialer refuses every
// non-public destination after DNS resolution; DialContext replaces the two
// failures that disclose internal addressing (a refused destination and a
// failed lookup) with fixed errors and logs the original at warn level.
// Other dial errors, such as a refused connection or a timeout, pass through
// unchanged.
type GuardedDialer struct {
	dialer *net.Dialer
}

// DialContext implements proxy.DialFunc.
func (g *GuardedDialer) DialContext(ctx context.Context, network, address string) (net.Conn, error) {
	conn, err := g.dialer.DialContext(ctx, network, address)
	if err == nil {
		return conn, nil
	}
	var public error
	var dnsErr *net.DNSError
	switch {
	case errors.Is(err, errNonPublicAddress):
		public = errProviderAddressNotAllowed
	case errors.As(err, &dnsErr):
		public = errProviderHostUnresolved
	default:
		return nil, err
	}
	// The original error and the address are internal; only the fixed error
	// is returned to the caller.
	slog.Default().Warn("provider dial refused",
		slog.String("component", "dataplane"),
		slog.String("call_id", proxy.CallIDFromContext(ctx)),
		slog.String("address", address),
		slog.String("reason", public.Error()),
		slog.String("error", err.Error()))
	return nil, public
}

// PublicDialer returns the provider dialer. It refuses every non-public
// destination after DNS resolution and hides internal addressing from dial
// errors (see GuardedDialer). The engine uses it for the SDK Claude client,
// which does not use PublicTransport.
func PublicDialer() *GuardedDialer {
	return &GuardedDialer{dialer: &net.Dialer{Timeout: 30 * time.Second, KeepAlive: 30 * time.Second, Control: publicOnly}}
}

// PublicTransport returns the provider transport. It dials only public
// addresses and ignores proxy environment variables: an egress proxy would
// hide the destination from the address check.
func PublicTransport() *http.Transport {
	return &http.Transport{
		Proxy:                 nil,
		DialContext:           PublicDialer().DialContext,
		ForceAttemptHTTP2:     true,
		MaxIdleConns:          200,
		MaxIdleConnsPerHost:   32,
		IdleConnTimeout:       90 * time.Second,
		TLSHandshakeTimeout:   10 * time.Second,
		ExpectContinueTimeout: time.Second,
	}
}
