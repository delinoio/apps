// SPDX-License-Identifier: FSL-1.1-ALv2

package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"io"
	"os"
	"time"

	"github.com/delinoio/apps/servers/devbase-api/internal/config"
	"github.com/delinoio/apps/servers/devbase-api/internal/store"
	"github.com/google/uuid"
)

// auditQuery is the operator audit procedure. It prints matching records as JSON
// lines, newest first. Records hold no names, email addresses, metadata, raw keys
// or external user IDs.
func auditQuery(ctx context.Context, args []string, out io.Writer) error {
	flags := flag.NewFlagSet("audit", flag.ContinueOnError)
	flags.SetOutput(os.Stderr)
	since := flags.String("since", "", "earliest record time (RFC 3339); default is 24 hours ago")
	target := flags.String("target", "", "only records for this internal target UUID")
	actor := flags.String("actor", "", "only records by this actor, such as user:<id> or key:<id>")
	limit := flags.Int("limit", 100, "maximum records, 1 to 10000")
	if e := flags.Parse(args); e != nil {
		return e
	}
	from := time.Now().UTC().Add(-24 * time.Hour)
	if *since != "" {
		t, e := time.Parse(time.RFC3339, *since)
		if e != nil {
			return errors.New("--since must be an RFC 3339 time")
		}
		from = t
	}
	if *limit < 1 || *limit > 10000 {
		return errors.New("--limit must be 1 to 10000")
	}
	if _, e := uuid.Parse(*target); *target != "" && e != nil {
		return errors.New("--target must be a UUID")
	}
	cfg, e := config.Database(os.Getenv)
	if e != nil {
		return e
	}
	db, e := store.Open(ctx, cfg.DatabaseURL)
	if e != nil {
		return e
	}
	defer db.Close()
	rows, e := db.Pool.Query(ctx, `SELECT occurred_at,action,actor,target_id::text,trace_id FROM audit_events
 WHERE occurred_at>=$1 AND ($2='' OR target_id=NULLIF($2,'')::uuid) AND ($3='' OR actor=$3)
 ORDER BY occurred_at DESC,id DESC LIMIT $4`, from, *target, *actor, *limit)
	if e != nil {
		return e
	}
	defer rows.Close()
	encoder := json.NewEncoder(out)
	for rows.Next() {
		var record struct {
			OccurredAt time.Time `json:"occurred_at"`
			Action     string    `json:"action"`
			Actor      string    `json:"actor"`
			TargetID   string    `json:"target_id"`
			TraceID    string    `json:"trace_id"`
		}
		if e = rows.Scan(&record.OccurredAt, &record.Action, &record.Actor, &record.TargetID, &record.TraceID); e != nil {
			return e
		}
		if e = encoder.Encode(record); e != nil {
			return e
		}
	}
	return rows.Err()
}
