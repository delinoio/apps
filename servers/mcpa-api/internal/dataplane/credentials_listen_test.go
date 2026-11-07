// SPDX-License-Identifier: FSL-1.1-ALv2

package dataplane

import (
	"context"
	"errors"
	"io"
	"log/slog"
	"testing"
	"time"
)

type listenStep struct {
	connected bool
}

func runScripted(t *testing.T, steps []listenStep) []time.Duration {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	var waits []time.Duration
	i := 0
	listen := func(context.Context) (bool, error) {
		step := steps[i]
		i++
		return step.connected, errors.New("dropped")
	}
	wait := func(_ context.Context, d time.Duration) bool {
		waits = append(waits, d)
		if i == len(steps) {
			cancel()
			return false
		}
		return true
	}
	log := slog.New(slog.NewTextHandler(io.Discard, nil))
	if err := runListenLoop(ctx, log, listen, wait); err != nil {
		t.Fatalf("runListenLoop = %v, want nil", err)
	}
	return waits
}

func assertWaits(t *testing.T, got []time.Duration, want ...time.Duration) {
	t.Helper()
	if len(got) != len(want) {
		t.Fatalf("waits = %v, want %v", got, want)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Fatalf("waits = %v, want %v", got, want)
		}
	}
}

func TestRunListenLoopBackoffBeforeListen(t *testing.T) {
	steps := make([]listenStep, 8)
	got := runScripted(t, steps)
	s := time.Second
	assertWaits(t, got, s, 2*s, 4*s, 8*s, 16*s, 32*s, 60*s, 60*s)
}

func TestRunListenLoopResetsAfterConnection(t *testing.T) {
	got := runScripted(t, []listenStep{{false}, {false}, {false}, {true}, {false}, {true}, {true}})
	s := time.Second
	assertWaits(t, got, s, 2*s, 4*s, s, 2*s, s, s)
}

func TestRunListenLoopCancelEndsWithoutWait(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	waited := false
	listen := func(context.Context) (bool, error) {
		cancel()
		return true, errors.New("canceled")
	}
	wait := func(context.Context, time.Duration) bool {
		waited = true
		return true
	}
	log := slog.New(slog.NewTextHandler(io.Discard, nil))
	if err := runListenLoop(ctx, log, listen, wait); err != nil {
		t.Fatalf("runListenLoop = %v, want nil", err)
	}
	if waited {
		t.Fatal("wait called after context cancellation")
	}
}

func TestSleepContext(t *testing.T) {
	if !sleepContext(context.Background(), time.Millisecond) {
		t.Fatal("sleepContext = false, want true")
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if sleepContext(ctx, time.Hour) {
		t.Fatal("sleepContext on canceled ctx = true, want false")
	}
}
