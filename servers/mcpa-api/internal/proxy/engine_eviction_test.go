// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"context"
	"runtime"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/testutil"
)

// fakeClock is a manually advanced clock for the engine's Now option.
type fakeClock struct {
	mu  sync.Mutex
	now time.Time
}

func newFakeClock() *fakeClock { return &fakeClock{now: time.Unix(1_700_000_000, 0)} }

func (c *fakeClock) Now() time.Time {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.now
}

func (c *fakeClock) Advance(d time.Duration) {
	c.mu.Lock()
	c.now = c.now.Add(d)
	c.mu.Unlock()
}

// evictionHarness builds a harness whose janitor is driven by hand through
// Engine.evictIdle and whose resources each hold one credential.
func newEvictionHarness(t *testing.T, clock *fakeClock, options ...harnessOption) *harness {
	t.Helper()
	opts := append([]harnessOption{func(o *Options) {
		o.Now = clock.Now
		o.JanitorInterval = -1
		// A positive interval makes every runtime start SDK refresh
		// goroutines, which the eviction must stop.
		o.AutoRefreshInterval = time.Hour
	}}, options...)
	return newHarness(t, opts...)
}

// seedResources registers n resources with one credential each in the
// credential source and returns their IDs.
func seedResources(h *harness, n int) []string {
	ids := make([]string, n)
	for i := range ids {
		ids[i] = newResourceID()
		h.creds.put(compatCredential(ids[i], "https://provider.example", "sk-test", "gpt-test"))
	}
	return ids
}

// touch loads the resource's runtime and releases it at once.
func touch(t *testing.T, h *harness, resourceID string) *resourceRuntime {
	t.Helper()
	rt, release, err := h.engine.runtime(context.Background(), resourceID)
	if err != nil {
		t.Fatal(err)
	}
	release()
	return rt
}

func loadedCount(e *Engine) int {
	e.mu.Lock()
	defer e.mu.Unlock()
	return len(e.runtimes)
}

func isLoaded(e *Engine, resourceID string) bool {
	e.mu.Lock()
	defer e.mu.Unlock()
	return e.runtimes[resourceID] != nil
}

// waitGoroutines waits until the goroutine count is at most limit.
func waitGoroutines(t *testing.T, limit int) int {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for {
		n := runtime.NumGoroutine()
		if n <= limit || time.Now().After(deadline) {
			return n
		}
		time.Sleep(20 * time.Millisecond)
	}
}

func TestIdleRuntimesAreEvictedAndGoroutinesReturn(t *testing.T) {
	clock := newFakeClock()
	reg := prometheus.NewRegistry()
	metrics, err := NewMetrics(reg, nil)
	if err != nil {
		t.Fatal(err)
	}
	h := newEvictionHarness(t, clock, func(o *Options) { o.Metrics = metrics })
	ids := seedResources(h, 100)
	baseline := runtime.NumGoroutine()

	for _, id := range ids {
		touch(t, h, id)
	}
	if got := loadedCount(h.engine); got != 100 {
		t.Fatalf("loaded %d runtimes, want 100", got)
	}
	if got := testutil.ToFloat64(metrics.runtimesLoaded); got != 100 {
		t.Fatalf("mcpa_runtimes_loaded = %v, want 100", got)
	}
	if peak := runtime.NumGoroutine(); peak <= baseline+len(ids) {
		t.Fatalf("goroutines at peak %d, baseline %d: runtimes should hold refresh goroutines", peak, baseline)
	}

	// Before the TTL nothing is evicted.
	clock.Advance(defaultRuntimeIdleTTL - time.Second)
	if evicted := h.engine.evictIdle(); evicted != 0 || loadedCount(h.engine) != 100 {
		t.Fatalf("evicted %d before the idle TTL, %d loaded", evicted, loadedCount(h.engine))
	}
	clock.Advance(time.Second)
	if evicted := h.engine.evictIdle(); evicted != 100 {
		t.Fatalf("evicted %d runtimes, want 100", evicted)
	}
	if got := loadedCount(h.engine); got != 0 {
		t.Fatalf("%d runtimes still loaded", got)
	}
	if got := testutil.ToFloat64(metrics.runtimesLoaded); got != 0 {
		t.Fatalf("mcpa_runtimes_loaded = %v after eviction, want 0", got)
	}
	// Allow a small tolerance for unrelated runtime goroutines.
	if n := waitGoroutines(t, baseline+5); n > baseline+5 {
		t.Fatalf("goroutines after eviction %d, baseline %d", n, baseline)
	}

	// A later call rebuilds the runtime from the credential source.
	loads := h.creds.loads.Load()
	rt := touch(t, h, ids[0])
	if h.creds.loads.Load() != loads+1 {
		t.Fatalf("credential source loads %d, want %d", h.creds.loads.Load(), loads+1)
	}
	if !rt.serves("gpt-test") {
		t.Fatal("rebuilt runtime does not serve the credential's model")
	}
}

func TestInFlightRuntimeIsNeverEvicted(t *testing.T) {
	clock := newFakeClock()
	h := newEvictionHarness(t, clock)
	ids := seedResources(h, 2)

	rt, release, err := h.engine.runtime(context.Background(), ids[0])
	if err != nil {
		t.Fatal(err)
	}
	touch(t, h, ids[1])

	clock.Advance(10 * defaultRuntimeIdleTTL)
	if evicted := h.engine.evictIdle(); evicted != 1 {
		t.Fatalf("evicted %d runtimes, want only the idle one", evicted)
	}
	if !isLoaded(h.engine, ids[0]) || isLoaded(h.engine, ids[1]) {
		t.Fatal("the in-flight runtime must stay loaded and the idle one must go")
	}
	if !rt.serves("gpt-test") {
		t.Fatal("the in-flight runtime was shut down")
	}

	// Finishing the call restarts the idle period.
	release()
	if evicted := h.engine.evictIdle(); evicted != 0 {
		t.Fatalf("evicted %d runtimes right after the call finished", evicted)
	}
	clock.Advance(defaultRuntimeIdleTTL)
	if evicted := h.engine.evictIdle(); evicted != 1 {
		t.Fatalf("evicted %d runtimes after the idle TTL, want 1", evicted)
	}
	// Releasing twice must not corrupt the lease count.
	release()
	rt2, release2, err := h.engine.runtime(context.Background(), ids[0])
	if err != nil {
		t.Fatal(err)
	}
	defer release2()
	clock.Advance(10 * defaultRuntimeIdleTTL)
	if evicted := h.engine.evictIdle(); evicted != 0 || !rt2.serves("gpt-test") {
		t.Fatalf("a double release let a leased runtime be evicted (evicted %d)", evicted)
	}
}

func TestLeaseHeldByCredentialChangeBlocksEviction(t *testing.T) {
	clock := newFakeClock()
	h := newEvictionHarness(t, clock)
	ids := seedResources(h, 1)
	touch(t, h, ids[0])

	rt, release, err := h.engine.loadedRuntime(context.Background(), ids[0])
	if err != nil || rt == nil {
		t.Fatalf("loadedRuntime = %v, %v", rt, err)
	}
	clock.Advance(10 * defaultRuntimeIdleTTL)
	if evicted := h.engine.evictIdle(); evicted != 0 {
		t.Fatalf("evicted %d runtimes while a credential change held a lease", evicted)
	}
	release()
	if evicted := h.engine.evictIdle(); evicted != 0 {
		t.Fatalf("evicted %d runtimes right after the lease ended", evicted)
	}
	// An unloaded resource has no lease to take.
	if rt, release, err := h.engine.loadedRuntime(context.Background(), newResourceID()); rt != nil || release != nil || err != nil {
		t.Fatalf("loadedRuntime of an unloaded resource = %v, %v", rt, err)
	}
}

func TestOverCapacityEvictsLeastRecentlyUsedIdleRuntimes(t *testing.T) {
	clock := newFakeClock()
	h := newEvictionHarness(t, clock, func(o *Options) { o.MaxRuntimes = 10 })
	ids := seedResources(h, 15)
	for _, id := range ids {
		touch(t, h, id)
		clock.Advance(time.Second)
	}
	// Use the oldest runtime again so it is the most recently used.
	touch(t, h, ids[0])

	// ids[14] stays leased: it is among the most recent, but leased runtimes
	// count toward the cap and are never candidates.
	_, release, err := h.engine.runtime(context.Background(), ids[14])
	if err != nil {
		t.Fatal(err)
	}
	defer release()

	if evicted := h.engine.evictIdle(); evicted != 5 {
		t.Fatalf("evicted %d runtimes, want 5", evicted)
	}
	if got := loadedCount(h.engine); got != 10 {
		t.Fatalf("%d runtimes loaded, want the cap of 10", got)
	}
	// ids[1]..ids[5] are the five least recently used.
	for i, id := range ids {
		wantLoaded := i == 0 || i > 5
		if isLoaded(h.engine, id) != wantLoaded {
			t.Errorf("resource %d loaded = %v, want %v", i, !wantLoaded, wantLoaded)
		}
	}
}

func TestCallsDuringEvictionRebuildTheRuntime(t *testing.T) {
	clock := newFakeClock()
	h := newEvictionHarness(t, clock, func(o *Options) { o.AutoRefreshInterval = -1 })
	ids := seedResources(h, 4)

	var stop atomic.Bool
	var wg sync.WaitGroup
	var calls atomic.Int64
	for i := 0; i < 8; i++ {
		wg.Add(1)
		go func(id string) {
			defer wg.Done()
			for !stop.Load() {
				rt, release, err := h.engine.runtime(context.Background(), id)
				if err != nil {
					t.Errorf("runtime: %v", err)
					return
				}
				// A leased runtime must never be shut down underneath its user.
				if !rt.serves("gpt-test") {
					t.Error("a leased runtime lost its credentials")
				}
				calls.Add(1)
				release()
			}
		}(ids[i%len(ids)])
	}
	rebuilds := h.creds.loads.Load()
	deadline := time.Now().Add(10 * time.Second)
	for h.creds.loads.Load() < rebuilds+20 && time.Now().Before(deadline) {
		clock.Advance(2 * defaultRuntimeIdleTTL)
		h.engine.evictIdle()
		time.Sleep(time.Millisecond)
	}
	stop.Store(true)
	wg.Wait()
	if h.creds.loads.Load() < rebuilds+20 {
		t.Fatalf("only %d rebuilds in %d calls", h.creds.loads.Load()-rebuilds, calls.Load())
	}
}

func TestJanitorEvictsAndStopsOnClose(t *testing.T) {
	h := newHarness(t, func(o *Options) {
		o.RuntimeIdleTTL = 20 * time.Millisecond
		o.JanitorInterval = 5 * time.Millisecond
		o.AutoRefreshInterval = time.Hour
	})
	ids := seedResources(h, 3)
	for _, id := range ids {
		touch(t, h, id)
	}
	deadline := time.Now().Add(10 * time.Second)
	for loadedCount(h.engine) != 0 {
		if time.Now().After(deadline) {
			t.Fatalf("janitor left %d runtimes loaded", loadedCount(h.engine))
		}
		time.Sleep(5 * time.Millisecond)
	}
	if h.engine.janitorDone == nil {
		t.Fatal("the janitor is not running")
	}
	if err := h.engine.Close(); err != nil {
		t.Fatal(err)
	}
	select {
	case <-h.engine.janitorDone:
	default:
		t.Fatal("Close returned before the janitor stopped")
	}
}

func TestDefaultRuntimeLimits(t *testing.T) {
	h := newHarness(t)
	if h.engine.opts.RuntimeIdleTTL != 30*time.Minute || h.engine.opts.MaxRuntimes != 5000 || h.engine.opts.JanitorInterval != time.Minute {
		t.Fatalf("defaults: ttl %v, max %d, janitor %v", h.engine.opts.RuntimeIdleTTL, h.engine.opts.MaxRuntimes, h.engine.opts.JanitorInterval)
	}
}
