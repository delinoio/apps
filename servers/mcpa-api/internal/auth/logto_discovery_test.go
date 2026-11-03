// SPDX-License-Identifier: FSL-1.1-ALv2

package auth_test

import (
	"context"
	"errors"
	"net/http"
	"sync"
	"testing"
	"time"

	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth/authtest"
)

const discoveryTimeout = 300 * time.Millisecond

// newTimedLogto returns a Logto whose HTTP client, and so every discovery
// attempt, times out after discoveryTimeout.
func newTimedLogto(t *testing.T) (*auth.Logto, *authtest.Provider) {
	t.Helper()
	p := authtest.New(t)
	client := &http.Client{Timeout: discoveryTimeout, Transport: p.Server.Client().Transport}
	return auth.NewLogto(p.Issuer(), logtoClientID, "secret", client), p
}

func TestLogtoConcurrentDiscoveryDuringOutageSharesOneAttempt(t *testing.T) {
	l, p := newTimedLogto(t)
	p.SetDiscoveryHang(true)
	t.Cleanup(func() { p.SetDiscoveryHang(false) })

	const callers = 8
	errs := make([]error, callers)
	start := time.Now()
	var wg sync.WaitGroup
	for i := range callers {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_, _, _, errs[i] = l.Endpoints(context.Background())
		}()
	}
	wg.Wait()
	elapsed := time.Since(start)

	for i, err := range errs {
		if !errors.Is(err, auth.ErrLogtoUnavailable) {
			t.Fatalf("caller %d: %v, want ErrLogtoUnavailable", i, err)
		}
	}
	// Serialized attempts would take callers * discoveryTimeout.
	if elapsed > 3*discoveryTimeout {
		t.Fatalf("%d callers took %v, want about one discovery timeout (%v)", callers, elapsed, discoveryTimeout)
	}
	if n := p.DiscoveryRequests(); n != 1 {
		t.Fatalf("discovery attempted %d times for %d concurrent callers, want 1", n, callers)
	}
}

func TestLogtoDiscoveryWaiterCancellationDoesNotCancelSharedAttempt(t *testing.T) {
	l, p := newTimedLogto(t)
	p.SetDiscoveryHang(true)
	t.Cleanup(func() { p.SetDiscoveryHang(false) })

	// The first caller starts the shared attempt and then gives up.
	starter, cancelStarter := context.WithCancel(context.Background())
	starterDone := make(chan error, 1)
	go func() {
		_, _, _, err := l.Endpoints(starter)
		starterDone <- err
	}()
	waitFor(t, func() bool { return p.DiscoveryRequests() == 1 })

	// A second caller joins and survives the starter's cancellation.
	joinerDone := make(chan error, 1)
	go func() {
		_, _, _, err := l.Endpoints(context.Background())
		joinerDone <- err
	}()
	// Let the joiner reach the wait before the starter is cancelled.
	time.Sleep(50 * time.Millisecond)

	cancelStarter()
	select {
	case err := <-starterDone:
		if !errors.Is(err, context.Canceled) {
			t.Fatalf("cancelled waiter: %v, want context.Canceled", err)
		}
	case <-time.After(time.Second):
		t.Fatal("cancelled waiter did not return immediately")
	}

	// The shared attempt keeps running: releasing discovery now lets the
	// joiner succeed.
	p.SetDiscoveryHang(false)
	select {
	case err := <-joinerDone:
		if err != nil {
			t.Fatalf("joiner: %v, want success after the starter was cancelled", err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("joiner did not finish")
	}
	if n := p.DiscoveryRequests(); n != 1 {
		t.Fatalf("discovery attempted %d times, want 1", n)
	}
}

func TestLogtoDiscoveryWaiterDeadlineReturnsContextError(t *testing.T) {
	l, p := newTimedLogto(t)
	p.SetDiscoveryHang(true)
	t.Cleanup(func() { p.SetDiscoveryHang(false) })
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Millisecond)
	defer cancel()
	start := time.Now()
	_, _, _, err := l.Endpoints(ctx)
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("waiter deadline: %v, want context.DeadlineExceeded", err)
	}
	if time.Since(start) > discoveryTimeout/2 {
		t.Fatalf("waiter returned after %v, want immediately on its own deadline", time.Since(start))
	}
}

func TestLogtoDiscoveryRecoversAndCachesAfterOutage(t *testing.T) {
	l, p := newTimedLogto(t)
	p.SetDiscoveryHang(true)
	t.Cleanup(func() { p.SetDiscoveryHang(false) })
	if _, _, _, err := l.Endpoints(context.Background()); !errors.Is(err, auth.ErrLogtoUnavailable) {
		t.Fatalf("outage: %v, want ErrLogtoUnavailable", err)
	}

	// The failure is not cached: the next call starts a fresh attempt.
	p.SetDiscoveryHang(false)
	for range 3 {
		if _, _, _, err := l.Endpoints(context.Background()); err != nil {
			t.Fatalf("after recovery: %v", err)
		}
	}
	// One failed attempt plus one successful attempt; later calls hit the cache.
	if n := p.DiscoveryRequests(); n != 2 {
		t.Fatalf("discovery requests = %d, want 2", n)
	}
}

func waitFor(t *testing.T, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for !cond() {
		if time.Now().After(deadline) {
			t.Fatal("condition not reached in time")
		}
		time.Sleep(5 * time.Millisecond)
	}
}
