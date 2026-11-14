// SPDX-License-Identifier: FSL-1.1-ALv2

package main

import (
	"context"
	"fmt"
	"log/slog"
	"net"
	"net/http"
	"os"
	"os/exec"
	"runtime"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"
)

// The tests in this file cover the signal wiring of the real process. Signal
// registration is process-global, so they re-execute the test binary as a helper
// process that runs the production shutdownContext, newServer and serve, and
// signal that process from the parent.

const (
	// signalHelperEnv switches the test binary into helper-process mode.
	signalHelperEnv = "ACCOUNTS_API_SIGNAL_HELPER"
	// helperDrainTimeout is the drain timeout of the helper. It is far above
	// every limit the parent test waits for, so an exit that waits for the drain
	// to time out is distinguishable from a prompt one.
	helperDrainTimeout = 30 * time.Second
)

// TestSignalHelperProcess is not a test. The signal tests re-execute the test
// binary into it. It serves /ping and /slow?ms=<n> (a handler that blocks for n
// milliseconds), prints "listening <addr>" and "slow_started" lines, and exits
// with serve's result.
func TestSignalHelperProcess(t *testing.T) {
	if os.Getenv(signalHelperEnv) != "1" {
		t.Skip("helper process for the signal tests")
	}

	logger := slog.New(slog.NewJSONHandler(os.Stdout, nil))
	mux := http.NewServeMux()
	mux.HandleFunc("GET /ping", func(w http.ResponseWriter, r *http.Request) {})
	mux.HandleFunc("GET /slow", func(w http.ResponseWriter, r *http.Request) {
		ms, _ := strconv.Atoi(r.URL.Query().Get("ms"))
		fmt.Println("slow_started")
		time.Sleep(time.Duration(ms) * time.Millisecond)
	})

	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	ctx, stop := shutdownContext()
	defer stop()
	fmt.Println("listening", listener.Addr().String())
	if err := serve(ctx, stop, newServer(mux, logger), listener, logger, helperDrainTimeout); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	os.Exit(0)
}

// helperProcess is a running helper process and the output it printed.
type helperProcess struct {
	t      *testing.T
	cmd    *exec.Cmd
	addr   string
	exited chan struct{}
	out    *syncBuffer
}

func startHelperProcess(t *testing.T) *helperProcess {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("the helper process is signalled with POSIX signals")
	}
	out := &syncBuffer{}
	cmd := exec.Command(os.Args[0], "-test.run=^TestSignalHelperProcess$")
	cmd.Env = append(os.Environ(), signalHelperEnv+"=1")
	cmd.Stdout = out
	cmd.Stderr = out
	if err := cmd.Start(); err != nil {
		t.Fatalf("start helper: %v", err)
	}
	h := &helperProcess{t: t, cmd: cmd, exited: make(chan struct{}), out: out}
	// Wait runs once, here: it reaps the process and flushes the output copy.
	go func() {
		_ = cmd.Wait()
		close(h.exited)
	}()
	t.Cleanup(func() {
		_ = cmd.Process.Kill()
		<-h.exited
	})

	line := h.waitForLine("listening ")
	h.addr = strings.TrimSpace(line[strings.Index(line, "listening ")+len("listening "):])
	return h
}

// waitForLine returns the first output line that contains substr.
func (h *helperProcess) waitForLine(substr string) string {
	h.t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for {
		for _, line := range strings.Split(h.out.String(), "\n") {
			if strings.Contains(line, substr) {
				return line
			}
		}
		select {
		case <-h.exited:
			// The output is complete once the process has exited.
			for _, line := range strings.Split(h.out.String(), "\n") {
				if strings.Contains(line, substr) {
					return line
				}
			}
			h.t.Fatalf("helper exited before printing a line containing %q; output:\n%s", substr, h.out.String())
		case <-time.After(10 * time.Millisecond):
		}
		if time.Now().After(deadline) {
			h.t.Fatalf("timed out waiting for a line containing %q; output so far:\n%s", substr, h.out.String())
		}
	}
}

func (h *helperProcess) signal(sig syscall.Signal) {
	h.t.Helper()
	if err := h.cmd.Process.Signal(sig); err != nil {
		h.t.Fatalf("signal %v: %v", sig, err)
	}
}

// wait waits for the helper to exit within limit and returns its wait status
// and how long the exit took.
func (h *helperProcess) wait(limit time.Duration) (syscall.WaitStatus, time.Duration) {
	h.t.Helper()
	start := time.Now()
	select {
	case <-h.exited:
	case <-time.After(limit):
		h.t.Fatalf("helper still running %v after the signal; output:\n%s", limit, h.out.String())
	}
	return h.cmd.ProcessState.Sys().(syscall.WaitStatus), time.Since(start)
}

// getAsync starts a GET and delivers the status code, or -1 on a client error.
func getAsync(url string) <-chan int {
	status := make(chan int, 1)
	go func() {
		resp, err := http.Get(url)
		if err != nil {
			status <- -1
			return
		}
		_ = resp.Body.Close()
		status <- resp.StatusCode
	}()
	return status
}

func TestSecondTerminationSignalTerminatesProcessDuringDrain(t *testing.T) {
	for _, sig := range []syscall.Signal{syscall.SIGTERM, syscall.SIGINT} {
		t.Run(sig.String(), func(t *testing.T) {
			h := startHelperProcess(t)
			// The request outlives the whole test, so only the second signal can
			// end the process before helperDrainTimeout.
			getAsync("http://" + h.addr + "/slow?ms=60000")
			h.waitForLine("slow_started")

			h.signal(sig)
			h.waitForLine("server_shutdown_started")
			h.signal(sig)

			status, elapsed := h.wait(5 * time.Second)
			if !status.Signaled() || status.Signal() != sig {
				t.Errorf("wait status = %v, want terminated by %v", status, sig)
			}
			if elapsed >= 5*time.Second {
				t.Errorf("exit took %v after the second signal, want a prompt exit", elapsed)
			}
			if strings.Contains(h.out.String(), "server_shutdown_completed") {
				t.Errorf("output reports a completed shutdown:\n%s", h.out.String())
			}
		})
	}
}

func TestSingleTerminationSignalDrainsInFlightRequestAndExitsZero(t *testing.T) {
	h := startHelperProcess(t)
	status := getAsync("http://" + h.addr + "/slow?ms=500")
	h.waitForLine("slow_started")

	h.signal(syscall.SIGINT)
	h.waitForLine("server_shutdown_started")

	if got := <-status; got != http.StatusOK {
		t.Errorf("in-flight request status = %d, want %d", got, http.StatusOK)
	}
	ws, _ := h.wait(5 * time.Second)
	if !ws.Exited() || ws.ExitStatus() != 0 {
		t.Errorf("wait status = %v, want exit status 0", ws)
	}
	h.waitForLine("server_shutdown_completed")
}

func TestSingleTerminationSignalWithoutTrafficExitsZero(t *testing.T) {
	h := startHelperProcess(t)

	resp, err := http.Get("http://" + h.addr + "/ping")
	if err != nil {
		t.Fatalf("GET /ping before any signal: %v", err)
	}
	_ = resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Errorf("GET /ping status = %d, want %d", resp.StatusCode, http.StatusOK)
	}

	h.signal(syscall.SIGTERM)
	h.waitForLine("server_shutdown_completed")
	ws, _ := h.wait(5 * time.Second)
	if !ws.Exited() || ws.ExitStatus() != 0 {
		t.Errorf("wait status = %v, want exit status 0", ws)
	}
}

func TestAwaitServeCallsStopBeforeLoggingShutdownStarted(t *testing.T) {
	logs := &syncBuffer{}
	logger := slog.New(slog.NewJSONHandler(logs, nil))
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	// serveErr only becomes ready inside stop. That keeps the select in
	// awaitServe from racing between a ready ctx and a ready serveErr, and it
	// models Serve returning once the drain has started.
	serveErr := make(chan error, 1)
	var logsAtStop string
	stopCalls := 0
	stop := func() {
		stopCalls++
		logsAtStop = logs.String()
		serveErr <- http.ErrServerClosed
	}
	if err := awaitServe(ctx, stop, &http.Server{}, serveErr, logger, time.Second); err != nil {
		t.Fatalf("awaitServe = %v, want nil", err)
	}
	if stopCalls != 1 {
		t.Errorf("stop called %d times, want 1", stopCalls)
	}
	if strings.Contains(logsAtStop, "server_shutdown_started") {
		t.Errorf("stop ran after server_shutdown_started was logged:\n%s", logsAtStop)
	}
	if !strings.Contains(logs.String(), "server_shutdown_started") {
		t.Errorf("logs lack server_shutdown_started:\n%s", logs.String())
	}
}

func TestAwaitServeDoesNotCallStopWhenServeFailsFirst(t *testing.T) {
	serveErr := make(chan error, 1)
	serveErr <- fmt.Errorf("accept failed")
	stopCalled := false
	err := awaitServe(context.Background(), func() { stopCalled = true }, &http.Server{}, serveErr, newTestLogger(), time.Second)
	if err == nil {
		t.Fatal("awaitServe = nil, want the serve error")
	}
	if stopCalled {
		t.Error("stop called on the serve-error path, want it left to the caller's defer")
	}
}
