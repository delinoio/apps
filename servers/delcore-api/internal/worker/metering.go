// SPDX-License-Identifier: FSL-1.1-ALv2

package worker

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"time"

	"github.com/delinoio/apps/servers/delcore-api/internal/events"
	"github.com/delinoio/apps/servers/delcore-api/internal/postgres"
	"github.com/delinoio/apps/servers/delcore-api/internal/telemetry"
	"github.com/jackc/pgx/v5"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	"go.opentelemetry.io/otel/trace"
)

// MeteringBatch bounds the metering jobs processed in one worker step so other
// durable work keeps running while a backlog drains.
const MeteringBatch = 100

var errNoMeteringWork = errors.New("no ready metering work")

// ProcessMetering materializes up to limit ready metering jobs. Each job claims
// its row, applies the event transition, rates the changed meter periods and
// appends their ledger differences, runs the optional MeteringApplied hook, and
// completes in one transaction, so a crash before commit leaves the job pending
// with no aggregate or ledger effect and a crash after commit cannot apply or
// rate it twice. A failed attempt is recorded in a separate transaction with
// bounded retries.
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
	var entries int
	started := time.Now()
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
		now := time.Now().UTC()
		if applied, e = events.Process(ctx, tx, now, p.EventID); e != nil {
			return e
		}
		if entries, e = rateMetering(ctx, tx, now, applied); e != nil {
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
	// The span starts at the claim attempt but is created only for claimed
	// work, so an idle worker emits no spans. It carries the job ID and
	// bounded values only, never tenant or event IDs or attributes.
	_, span := telemetry.Tracer().Start(ctx, telemetry.SpanMeteringProcess, trace.WithTimestamp(started))
	defer span.End()
	if j.ID != "" {
		span.SetAttributes(attribute.String("delcore.job_id", j.ID), attribute.Int("delcore.job.attempt", j.Attempts+1))
	}
	if err == nil {
		w.Metrics.MeteringJob(telemetry.OutcomeApplied, "", time.Since(started))
		span.SetAttributes(attribute.String("delcore.outcome", string(telemetry.OutcomeApplied)), attribute.Int("delcore.metering.changed_groups", len(applied.Changes)), attribute.Int("delcore.metering.rating_differences", entries))
		w.Log.DebugContext(ctx, "metering work applied", "category", "metering_processing", "job_id", j.ID, "from_revision", applied.FromRevision, "to_revision", applied.ToRevision, "changed_groups", len(applied.Changes), "rating_differences", entries)
		return true, nil
	}
	if j.ID == "" {
		// No job was claimed, for example during a database outage. Annotate the
		// span with the same bounded attributes as the other failure paths so
		// outcome-based trace queries still find it.
		span.SetAttributes(attribute.String("delcore.outcome", string(telemetry.OutcomeFailed)), attribute.String("delcore.failure_category", "database"))
		span.SetStatus(codes.Error, "database")
		return false, err
	}
	if ctx.Err() != nil {
		// Shutdown rolled back the attempt; the job stays pending without
		// consuming its retry budget. The span still carries bounded outcome
		// and category values so outcome-based trace queries find it.
		span.SetAttributes(attribute.String("delcore.outcome", string(telemetry.OutcomeDeferred)), attribute.String("delcore.failure_category", "shutdown"))
		span.SetStatus(codes.Error, "shutdown")
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
	outcome := telemetry.OutcomeDeferred
	if state == "failed" {
		outcome = telemetry.OutcomeFailed
	}
	w.Metrics.MeteringJob(outcome, category, time.Since(started))
	span.SetAttributes(attribute.String("delcore.outcome", string(outcome)), attribute.String("delcore.failure_category", category))
	span.SetStatus(codes.Error, category)
	w.Log.WarnContext(ctx, "metering work deferred", "category", category, "state", state, "job_id", j.ID, "attempts", j.Attempts+1)
	// Record the attempt with a fresh bound after the processing transaction
	// rolled back, including after its own timeout.
	recordCtx, cancel := context.WithTimeout(context.WithoutCancel(ctx), w.Config.Timeout())
	defer cancel()
	if e := w.DB.Transaction(recordCtx, func(tx pgx.Tx) error {
		tag, e := tx.Exec(recordCtx, "UPDATE jobs SET state=$2,error_category=$3,attempts=attempts+1,available_at=$4,updated_at=now(),failed_at=now() WHERE id=$1 AND state='pending'", j.ID, state, category, time.Now().Add(time.Duration(w.Config.RetrySeconds)*time.Second))
		if e != nil || state != "failed" || tag.RowsAffected() != 1 {
			return e
		}
		// A permanent failure is a state change that waits for an operator;
		// audit it as system work. Transient deferrals are not audited.
		return postgres.Audit(recordCtx, tx, postgres.ActorMetering, j.Tenant, "metering.failed", fmt.Sprintf("category=%s attempts=%d", category, j.Attempts+1), id(), time.Now().UTC())
	}); e != nil {
		return false, e
	}
	return true, nil
}

var errInvalidMeteringPayload = errors.New("invalid metering job payload")
