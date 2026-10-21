// SPDX-License-Identifier: FSL-1.1-ALv2

package worker

import (
	"context"
	"encoding/json"
	"errors"
	"time"

	"github.com/delinoio/apps/servers/delcore-api/internal/events"
	"github.com/jackc/pgx/v5"
)

// MeteringBatch bounds the metering jobs processed in one worker step so other
// durable work keeps running while a backlog drains.
const MeteringBatch = 100

var errNoMeteringWork = errors.New("no ready metering work")

// ProcessMetering materializes up to limit ready metering jobs. Each job claims
// its row, applies the event transition, runs the optional MeteringApplied hook,
// and completes in one transaction, so a crash before commit leaves the job
// pending and a crash after commit cannot apply it twice. A failed attempt is
// recorded in a separate transaction with bounded retries.
func (w *Worker) ProcessMetering(ctx context.Context, limit int) (int, error) {
	processed := 0
	for processed < limit {
		done, err := w.processMeteringJob(ctx)
		if err != nil || !done {
			return processed, err
		}
		processed++
	}
	return processed, nil
}

func (w *Worker) processMeteringJob(ctx context.Context) (bool, error) {
	var j job
	var applied events.Applied
	// Bound one transition; a timeout is a transient, retried failure.
	jobCtx, cancelJob := context.WithTimeout(ctx, w.Config.Timeout())
	defer cancelJob()
	err := w.DB.Transaction(jobCtx, func(tx pgx.Tx) error {
		ctx := jobCtx
		e := tx.QueryRow(ctx, `SELECT id::text,COALESCE(tenant_id::text,''),attempts,payload FROM jobs WHERE kind=$1 AND state='pending' AND available_at<=now() ORDER BY available_at,id LIMIT 1 FOR UPDATE SKIP LOCKED`, events.JobKind).Scan(&j.ID, &j.Tenant, &j.Attempts, &j.Payload)
		if errors.Is(e, pgx.ErrNoRows) {
			return errNoMeteringWork
		}
		if e != nil {
			return e
		}
		var p struct {
			EventID string `json:"event_id"`
		}
		if json.Unmarshal(j.Payload, &p) != nil || p.EventID == "" {
			return errInvalidMeteringPayload
		}
		if applied, e = events.Process(ctx, tx, time.Now().UTC(), p.EventID); e != nil {
			return e
		}
		if w.MeteringApplied != nil {
			if e = w.MeteringApplied(ctx, tx, applied); e != nil {
				return e
			}
		}
		_, e = tx.Exec(ctx, "UPDATE jobs SET state='done',error_category='',attempts=attempts+1,updated_at=now() WHERE id=$1", j.ID)
		return e
	})
	if errors.Is(err, errNoMeteringWork) {
		return false, nil
	}
	if err == nil {
		w.Log.DebugContext(ctx, "metering work applied", "category", "metering_processing", "job_id", j.ID, "from_revision", applied.FromRevision, "to_revision", applied.ToRevision, "changed_groups", len(applied.Changes))
		return true, nil
	}
	if j.ID == "" {
		return false, err
	}
	if ctx.Err() != nil {
		// Shutdown rolled back the attempt; the job stays pending without
		// consuming its retry budget.
		return false, ctx.Err()
	}
	state, category := "pending", "metering_transient"
	switch {
	case errors.Is(err, errInvalidMeteringPayload):
		state, category = "failed", "invalid_payload"
	case errors.Is(err, events.ErrPermanent):
		state, category = "failed", "metering_invalid"
	case j.Attempts+1 >= w.Config.RetryLimit:
		state, category = "failed", "retry_exhausted"
	}
	w.Log.WarnContext(ctx, "metering work deferred", "category", category, "state", state, "job_id", j.ID, "attempts", j.Attempts+1)
	// Record the attempt with a fresh bound after the processing transaction
	// rolled back, including after its own timeout.
	recordCtx, cancel := context.WithTimeout(context.WithoutCancel(ctx), w.Config.Timeout())
	defer cancel()
	if e := w.DB.Transaction(recordCtx, func(tx pgx.Tx) error {
		_, e := tx.Exec(recordCtx, "UPDATE jobs SET state=$2,error_category=$3,attempts=attempts+1,available_at=$4,updated_at=now() WHERE id=$1 AND state='pending'", j.ID, state, category, time.Now().Add(time.Duration(w.Config.RetrySeconds)*time.Second))
		return e
	}); e != nil {
		return false, e
	}
	return true, nil
}

var errInvalidMeteringPayload = errors.New("invalid metering job payload")
