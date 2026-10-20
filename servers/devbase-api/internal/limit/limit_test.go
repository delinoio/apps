// SPDX-License-Identifier: FSL-1.1-ALv2

package limit

import (
	"sync"
	"testing"
	"time"
)

func TestBurstRefillAndRetryTiming(t *testing.T) {
	now := time.Unix(1000, 0)
	l := New(10, 20, func() time.Time { return now })
	for i := 0; i < 20; i++ {
		if ok, _ := l.Allow("a"); !ok {
			t.Fatalf("burst request %d rejected", i)
		}
	}
	ok, wait := l.Allow("a")
	if ok || wait != 100*time.Millisecond {
		t.Fatalf("expected rejection with 100ms retry, got %v %v", ok, wait)
	}
	if ok, _ := l.Allow("b"); !ok {
		t.Fatal("another key shares the bucket")
	}
	now = now.Add(time.Second)
	for i := 0; i < 10; i++ {
		if ok, _ := l.Allow("a"); !ok {
			t.Fatalf("refilled request %d rejected", i)
		}
	}
	if ok, _ := l.Allow("a"); ok {
		t.Fatal("refill exceeded the rate")
	}
}

func TestEvictionAndConcurrency(t *testing.T) {
	now := time.Unix(0, 0)
	var mu sync.Mutex
	clock := func() time.Time { mu.Lock(); defer mu.Unlock(); return now }
	l := New(1, 5, clock)
	for _, key := range []string{"x", "y", "z"} {
		l.Allow(key)
	}
	mu.Lock()
	now = now.Add(2 * time.Minute)
	mu.Unlock()
	l.Allow("fresh")
	if l.Size() != 1 {
		t.Fatalf("idle buckets were not evicted: %d", l.Size())
	}
	var wg sync.WaitGroup
	allowed := make(chan bool, 100)
	for i := 0; i < 100; i++ {
		wg.Add(1)
		go func() { defer wg.Done(); ok, _ := l.Allow("hot"); allowed <- ok }()
	}
	wg.Wait()
	close(allowed)
	n := 0
	for ok := range allowed {
		if ok {
			n++
		}
	}
	if n != 5 {
		t.Fatalf("expected exactly the burst of 5 under concurrency, got %d", n)
	}
}
