// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package main

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/delinoio/apps/servers/devbase-api/internal/store"
	"github.com/google/uuid"
)

func TestAuditQueryProcedure(t *testing.T) {
	url := os.Getenv("TEST_DATABASE_URL")
	if url == "" {
		t.Skip("TEST_DATABASE_URL is required")
	}
	ctx := context.Background()
	db, e := store.Open(ctx, url)
	if e != nil {
		t.Fatal(e)
	}
	defer db.Close()
	if _, e = db.Pool.Exec(ctx, "DROP SCHEMA public CASCADE; CREATE SCHEMA public"); e != nil {
		t.Fatal(e)
	}
	if e = db.Migrate(ctx); e != nil {
		t.Fatal(e)
	}
	t.Setenv("DATABASE_URL", url)
	target := uuid.NewString()
	now := time.Now().UTC().Truncate(time.Microsecond)
	for i, row := range []struct {
		age    time.Duration
		action string
		actor  string
		target string
	}{
		{time.Hour, "project.create", "user:a", target},
		{2 * time.Hour, "api_key.issue", "user:a", uuid.NewString()},
		{3 * time.Hour, "end_user.create", "key:b", uuid.NewString()},
		{72 * time.Hour, "project.delete", "user:a", uuid.NewString()},
	} {
		if _, e = db.Pool.Exec(ctx, "INSERT INTO audit_events VALUES($1,$2,$3,$4,$5,$6)", uuid.NewString(), now.Add(-row.age), row.action, row.actor, row.target, "trace"+string(rune('a'+i))); e != nil {
			t.Fatal(e)
		}
	}
	query := func(args ...string) []map[string]any {
		var out bytes.Buffer
		if e := auditQuery(ctx, args, &out); e != nil {
			t.Fatal(e)
		}
		var rows []map[string]any
		for _, line := range strings.Split(strings.TrimSpace(out.String()), "\n") {
			if line == "" {
				continue
			}
			var row map[string]any
			if e := json.Unmarshal([]byte(line), &row); e != nil {
				t.Fatal(e)
			}
			rows = append(rows, row)
		}
		return rows
	}
	if rows := query(); len(rows) != 3 || rows[0]["action"] != "project.create" {
		t.Fatalf("default window should list the last day newest first: %v", rows)
	}
	if rows := query("--since", now.Add(-96*time.Hour).Format(time.RFC3339)); len(rows) != 4 {
		t.Fatal("--since", rows)
	}
	if rows := query("--target", target); len(rows) != 1 {
		t.Fatal("--target", rows)
	}
	if rows := query("--actor", "key:b"); len(rows) != 1 || rows[0]["action"] != "end_user.create" {
		t.Fatal("--actor", rows)
	}
	if rows := query("--limit", "2"); len(rows) != 2 {
		t.Fatal("--limit", rows)
	}
	for _, bad := range [][]string{{"--since", "yesterday"}, {"--limit", "0"}, {"--limit", "10001"}, {"--target", "nope"}} {
		if e := auditQuery(ctx, bad, &bytes.Buffer{}); e == nil {
			t.Fatalf("%v accepted", bad)
		}
	}
	for _, row := range query("--since", now.Add(-96*time.Hour).Format(time.RFC3339)) {
		for key := range row {
			switch key {
			case "occurred_at", "action", "actor", "target_id", "trace_id":
			default:
				t.Fatal("unexpected audit field", key)
			}
		}
	}
}
