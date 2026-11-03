// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"context"
	"log/slog"
	"sync"
	"time"
)

// checkpointState tracks what a call has billed. The sink receives each
// checkpoint with a stable sequence number; a checkpoint whose commit failed
// is resent unchanged before any newer bytes, so an ambiguous failure (the
// sink committed but reported an error) cannot bill the same bytes under two
// event keys.
type checkpointState struct {
	mu             sync.Mutex
	seq            int64
	billedReq      int64
	billedResp     int64
	pending        *pendingCheckpoint
	finalCommitted bool
}

type pendingCheckpoint struct {
	deltaReq  int64
	deltaResp int64
	final     bool
	record    CallRecordUpdate
}

// checkpointer drives the metering checkpoints of one call.
type checkpointer struct {
	sink     MeterSink
	metrics  *Metrics
	logger   *slog.Logger
	interval time.Duration
	timeout  time.Duration
	// finalAttempts bounds commit attempts for the final checkpoint.
	finalAttempts int
	retryDelay    time.Duration
}

// run commits periodic checkpoints until stop is closed.
func (cp *checkpointer) run(ctx context.Context, call *callState, record func(final bool) CallRecordUpdate, stop <-chan struct{}) {
	if cp.interval <= 0 {
		<-stop
		return
	}
	ticker := time.NewTicker(cp.interval)
	defer ticker.Stop()
	for {
		select {
		case <-stop:
			return
		case <-ticker.C:
			cp.commit(ctx, call, record, false)
		}
	}
}

// commit sends the pending checkpoint, if any, and then a checkpoint for the
// unbilled bytes. Periodic checkpoints without new bytes are skipped; the
// final checkpoint is always sent so the sink can close the call record.
func (cp *checkpointer) commit(ctx context.Context, call *callState, record func(final bool) CallRecordUpdate, final bool) bool {
	state := &call.checkpoint
	state.mu.Lock()
	defer state.mu.Unlock()
	if state.finalCommitted {
		return true
	}
	attempts := 1
	if final {
		attempts = max(cp.finalAttempts, 1)
	}
	if state.pending != nil {
		if !cp.send(ctx, call, state, state.pending, attempts) {
			return false
		}
		state.pending = nil
	}
	req := call.requestBytes.Load()
	resp := call.responseBytes.Load()
	deltaReq, deltaResp := req-state.billedReq, resp-state.billedResp
	if !final && deltaReq == 0 && deltaResp == 0 {
		return true
	}
	// An invalid_call_key rejection is never recorded or billed, and the 401
	// body counts toward responseBytes as soon as it is written. The
	// rejection is set before the body is written and the byte counters are
	// read above, so a periodic checkpoint that observes those bytes also
	// observes the rejection. The handler skips the final checkpoint for the
	// same rejection.
	if !final && call.invalidCallKeyRejected() {
		return true
	}
	state.seq++
	update := record(final)
	update.CheckpointSeq = state.seq
	update.RequestBytes = req
	update.ResponseBytes = resp
	next := &pendingCheckpoint{deltaReq: deltaReq, deltaResp: deltaResp, final: final, record: update}
	// The checkpoint is billed state from here on: on failure it is resent
	// unchanged with the same sequence number.
	state.billedReq, state.billedResp = req, resp
	if !cp.send(ctx, call, state, next, attempts) {
		state.pending = next
		return false
	}
	if final {
		state.finalCommitted = true
	}
	return true
}

func (cp *checkpointer) send(ctx context.Context, call *callState, state *checkpointState, p *pendingCheckpoint, attempts int) bool {
	for attempt := 1; attempt <= attempts; attempt++ {
		sinkCtx, cancel := context.WithTimeout(context.WithoutCancel(ctx), cp.timeout)
		err := cp.sink.Checkpoint(sinkCtx, call.id, p.deltaReq, p.deltaResp, p.final, p.record)
		cancel()
		if err == nil {
			cp.metrics.billed(p.deltaReq, p.deltaResp)
			if p.final {
				state.finalCommitted = true
			}
			return true
		}
		cp.metrics.checkpointFailure()
		// Uncommitted bytes are lost if the process stops before a later
		// checkpoint commits them, so every failure alerts.
		cp.logger.Error("metering checkpoint failed",
			slog.Bool("alert", true),
			slog.String("call_id", call.id),
			slog.Int64("checkpoint_seq", p.record.CheckpointSeq),
			slog.Bool("final", p.final),
			slog.Int("attempt", attempt),
			slog.String("error", err.Error()),
		)
		if attempt < attempts && cp.retryDelay > 0 {
			time.Sleep(cp.retryDelay * time.Duration(attempt))
		}
	}
	return false
}
