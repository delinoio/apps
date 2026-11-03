// SPDX-License-Identifier: FSL-1.1-ALv2

package dataplane

import (
	"context"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"connectrpc.com/connect"

	delcorev1 "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/proxy"
)

func quietLog() *slog.Logger { return slog.New(slog.NewTextHandler(io.Discard, nil)) }

func TestBackoffDoublesUpToOneHour(t *testing.T) {
	cases := []struct {
		attempts int
		want     time.Duration
	}{
		{1, 10 * time.Second}, {2, 20 * time.Second}, {3, 40 * time.Second}, {9, 2560 * time.Second},
		{10, time.Hour}, {50, time.Hour}, {10_000, time.Hour},
	}
	for _, tc := range cases {
		if got := Backoff(10*time.Second, tc.attempts); got != tc.want {
			t.Fatalf("attempt %d: %v, want %v", tc.attempts, got, tc.want)
		}
	}
}

// TestClassifyDeliveryReadsDelcoreReasons sends IngestEvent to a Delcore
// mock and classifies each failure from its ErrorDetail.
func TestClassifyDeliveryReadsDelcoreReasons(t *testing.T) {
	fake := newFakeDelcore(t)
	client := fake.client(t, time.Minute, time.Now)
	cases := []struct {
		err      error
		category string
		poison   bool
	}{
		{reasonError(connect.CodeInvalidArgument, delcorev1.ErrorReason_ERROR_REASON_INVALID_INPUT), "invalid_input", true},
		{reasonError(connect.CodeInvalidArgument, delcorev1.ErrorReason_ERROR_REASON_INVALID_ATTRIBUTE), "invalid_attribute", true},
		{reasonError(connect.CodeAlreadyExists, delcorev1.ErrorReason_ERROR_REASON_EVENT_IDENTITY_CONFLICT), "event_identity_conflict", true},
		{reasonError(connect.CodeFailedPrecondition, delcorev1.ErrorReason_ERROR_REASON_REPORTING_WINDOW_CLOSED), "reporting_window_closed", true},
		{reasonError(connect.CodeFailedPrecondition, delcorev1.ErrorReason_ERROR_REASON_NO_MATCHING_METER), "no_matching_meter", false},
		{reasonError(connect.CodeResourceExhausted, delcorev1.ErrorReason_ERROR_REASON_CARDINALITY_EXCEEDED), "cardinality_exceeded", false},
		{connect.NewError(connect.CodeUnavailable, errors.New("down")), "unavailable", false},
		{connect.NewError(connect.CodeUnauthenticated, errors.New("bad token")), "unauthenticated", false},
		{connect.NewError(connect.CodePermissionDenied, errors.New("no scope")), "rejected", false},
	}
	for _, tc := range cases {
		fake.set(func(f *fakeDelcore) { f.ingestErr = func(*delcorev1.Event) error { return tc.err } })
		_, err := client.IngestEvent(context.Background(), "019a0000-0000-7000-8000-0000000000aa", &delcorev1.Event{TenantId: "t", EventKey: "k"})
		if err == nil {
			t.Fatalf("%v: no error", tc.err)
		}
		category, poison := classifyDelivery(err)
		if category != tc.category || poison != tc.poison {
			t.Fatalf("%v: category %q poison %v, want %q %v", tc.err, category, poison, tc.category, tc.poison)
		}
	}
	// A transport failure without a Delcore answer retries.
	fake.srv.Close()
	_, err := client.IngestEvent(context.Background(), "019a0000-0000-7000-8000-0000000000ab", &delcorev1.Event{TenantId: "t"})
	if category, poison := classifyDelivery(err); category != "unavailable" || poison {
		t.Fatalf("transport failure: %q %v", category, poison)
	}
}

func TestCallAuthorizerTenantAndBillingGates(t *testing.T) {
	fake := newFakeDelcore(t)
	now := time.Now()
	clock := func() time.Time { return now }
	client := fake.client(t, 30*time.Second, clock)
	a := NewCallAuthorizer(nil, client, quietLog(), clock, time.Second)
	p := proxy.CallPrincipal{ResourceID: "019a0000-0000-7000-8000-000000000010", TenantID: "tenant-1", CallKeyID: "k", IssuerUserID: "user-1"}
	code := func(err error) proxy.RejectionCode {
		var r *proxy.Rejection
		if !errors.As(err, &r) {
			t.Fatalf("not a rejection: %v", err)
		}
		return r.Code
	}

	for _, state := range []delcorev1.TenantState{delcorev1.TenantState_TENANT_STATE_BLOCKED, delcorev1.TenantState_TENANT_STATE_DELETING,
		delcorev1.TenantState_TENANT_STATE_DELETED, delcorev1.TenantState_TENANT_STATE_UNKNOWN} {
		fake.set(func(f *fakeDelcore) { f.states["tenant-1"] = state })
		if got := code(a.AuthorizeCall(context.Background(), p)); got != proxy.RejectionTenantInactive {
			t.Fatalf("%s: %v", state, got)
		}
	}
	fake.set(func(f *fakeDelcore) { delete(f.states, "tenant-1") })

	// A permission denial (issuer left the tenant) invalidates the key; a
	// billing denial is tenant_inactive. Denials are never cached.
	fake.set(func(f *fakeDelcore) {
		f.denied["tenant-1/user-1/invoke_models"] = []delcorev1.BlockReason{delcorev1.BlockReason_BLOCK_REASON_PERMISSION}
	})
	if got := code(a.AuthorizeCall(context.Background(), p)); got != proxy.RejectionInvalidCallKey {
		t.Fatalf("permission denial: %v", got)
	}
	fake.set(func(f *fakeDelcore) {
		f.denied["tenant-1/user-1/invoke_models"] = []delcorev1.BlockReason{delcorev1.BlockReason_BLOCK_REASON_BALANCE}
	})
	if got := code(a.AuthorizeCall(context.Background(), p)); got != proxy.RejectionTenantInactive {
		t.Fatalf("billing denial: %v", got)
	}

	// Delcore down without a cached allowed result fails closed.
	fake.set(func(f *fakeDelcore) { delete(f.denied, "tenant-1/user-1/invoke_models"); f.down = true })
	if got := code(a.AuthorizeCall(context.Background(), p)); got != proxy.RejectionDependencyUnavailable {
		t.Fatalf("delcore down: %v", got)
	}
	// An allowed tenant passes without a store: budgets are a separate gate
	// (CheckBudgets), applied by the engine after the model allowlist. A fresh
	// client avoids the allowed-only cache of the cases above.
	fresh := NewCallAuthorizer(nil, fake.client(t, 30*time.Second, clock), quietLog(), clock, time.Second)
	fake.set(func(f *fakeDelcore) { f.down = false })
	if err := fresh.AuthorizeCall(context.Background(), p); err != nil {
		t.Fatalf("allowed tenant: %v", err)
	}
	if proxy.RejectionDependencyUnavailable.HTTPStatus() != http.StatusServiceUnavailable ||
		proxy.RejectionBudgetExhausted.HTTPStatus() != http.StatusTooManyRequests ||
		proxy.RejectionNoCredential.HTTPStatus() != http.StatusBadRequest ||
		proxy.RejectionTenantInactive.HTTPStatus() != http.StatusForbidden ||
		proxy.RejectionModelNotAllowed.HTTPStatus() != http.StatusForbidden ||
		proxy.RejectionInvalidCallKey.HTTPStatus() != http.StatusUnauthorized {
		t.Fatal("rejection statuses differ from the contract")
	}
}

func TestPublicTransportRefusesPrivateAddresses(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {}))
	defer srv.Close()
	client := &http.Client{Transport: PublicTransport(), Timeout: 2 * time.Second}
	_, err := client.Get(srv.URL)
	if err == nil || !errors.Is(err, errNonPublicAddress) {
		t.Fatalf("loopback dial: %v", err)
	}
	for _, addr := range []string{"10.0.0.1:443", "192.168.1.1:443", "169.254.169.254:80", "[::1]:443", "[fd00::1]:443", "100.64.0.1:443"} {
		if err := publicOnly("tcp", addr, nil); err == nil {
			t.Fatalf("%s accepted", addr)
		}
	}
	for _, addr := range []string{"8.8.8.8:443", "[2001:4860:4860::8888]:443"} {
		if err := publicOnly("tcp", addr, nil); err != nil {
			t.Fatalf("%s refused: %v", addr, err)
		}
	}
	if PublicTransport().Proxy != nil {
		t.Fatal("proxy environment variables must be ignored")
	}
}

// PublicDialer guards the SDK Claude client, which dials through the engine's
// upstream relay instead of PublicTransport.
func TestPublicDialerRefusesPrivateAddresses(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {}))
	defer srv.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	conn, err := PublicDialer().DialContext(ctx, "tcp", srv.Listener.Addr().String())
	if err == nil {
		conn.Close()
		t.Fatal("loopback dial succeeded")
	}
	if !errors.Is(err, errNonPublicAddress) {
		t.Fatalf("loopback dial: %v", err)
	}
	if conn, err := PublicDialer().DialContext(ctx, "tcp", "localhost:"+strings.TrimPrefix(srv.URL, "http://127.0.0.1:")); err == nil {
		conn.Close()
		t.Fatal("a name that resolves to loopback was dialed")
	}
}

func TestEveryRunsImmediatelyAndStops(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	runs := make(chan struct{}, 4)
	done := make(chan error, 1)
	go func() {
		done <- every(ctx, time.Hour, quietLog(), "test", func(context.Context) error {
			runs <- struct{}{}
			return errors.New("logged, not fatal")
		})
	}()
	select {
	case <-runs:
	case <-time.After(5 * time.Second):
		t.Fatal("the first cycle did not run immediately")
	}
	cancel()
	if err := <-done; err != nil {
		t.Fatalf("every returned %v", err)
	}
}
