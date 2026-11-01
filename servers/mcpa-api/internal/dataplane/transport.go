// SPDX-License-Identifier: FSL-1.1-ALv2

package dataplane

import (
	"errors"
	"net"
	"net/http"
	"net/netip"
	"syscall"
	"time"

	"github.com/delinoio/apps/servers/mcpa-api/internal/credential"
)

// errNonPublicAddress is returned when a provider host resolves to a
// private, loopback, link-local or otherwise non-public address.
var errNonPublicAddress = errors.New("provider address is not a public address")

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

// PublicTransport returns the provider transport. It dials only public
// addresses and ignores proxy environment variables: an egress proxy would
// hide the destination from the address check.
func PublicTransport() *http.Transport {
	dialer := &net.Dialer{Timeout: 30 * time.Second, KeepAlive: 30 * time.Second, Control: publicOnly}
	return &http.Transport{
		Proxy:                 nil,
		DialContext:           dialer.DialContext,
		ForceAttemptHTTP2:     true,
		MaxIdleConns:          200,
		MaxIdleConnsPerHost:   32,
		IdleConnTimeout:       90 * time.Second,
		TLSHandshakeTimeout:   10 * time.Second,
		ExpectContinueTimeout: time.Second,
	}
}
