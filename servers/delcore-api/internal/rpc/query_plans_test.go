// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package rpc

import (
	"context"
	"encoding/json"
	"net/url"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/delinoio/apps/servers/delcore-api/internal/events"
	"github.com/delinoio/apps/servers/delcore-api/internal/postgres"
	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
)

// planNode is the subset of a PostgreSQL JSON plan node these tests inspect.
type planNode struct {
	NodeType  string     `json:"Node Type"`
	IndexName string     `json:"Index Name"`
	IndexCond string     `json:"Index Cond"`
	Plans     []planNode `json:"Plans"`
}

func (n planNode) walk(visit func(planNode)) {
	visit(n)
	for _, child := range n.Plans {
		child.walk(visit)
	}
}

// explain returns the plan of query in a fresh migrated schema. Sequential and
// bitmap scans are disabled so the plan shows which index conditions and order
// the query can use, independent of the planner's estimates for empty tables.
func explain(t *testing.T, query string, args ...any) planNode {
	t.Helper()
	dsn := os.Getenv("TEST_DATABASE_URL")
	if dsn == "" {
		t.Fatal("TEST_DATABASE_URL is required for integration tests")
	}
	ctx := context.Background()
	root, err := postgres.Open(ctx, dsn)
	if err != nil {
		t.Fatal(err)
	}
	schema := "test_" + strings.ReplaceAll(uuid.Must(uuid.NewV7()).String(), "-", "")
	if _, err = root.Pool.Exec(ctx, "CREATE SCHEMA "+schema); err != nil {
		t.Fatal(err)
	}
	u, err := url.Parse(dsn)
	if err != nil {
		t.Fatal(err)
	}
	q := u.Query()
	q.Set("search_path", schema)
	u.RawQuery = q.Encode()
	db, err := postgres.Open(ctx, u.String())
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		db.Pool.Close()
		root.Pool.Exec(context.Background(), "DROP SCHEMA "+schema+" CASCADE")
		root.Pool.Close()
	})
	if err = db.Migrate(ctx); err != nil {
		t.Fatal(err)
	}
	var plans []struct {
		Plan planNode `json:"Plan"`
	}
	err = db.Transaction(ctx, func(tx pgx.Tx) error {
		if _, err := tx.Exec(ctx, "SET LOCAL enable_seqscan=off; SET LOCAL enable_bitmapscan=off"); err != nil {
			return err
		}
		var raw []byte
		if err := tx.QueryRow(ctx, "EXPLAIN (FORMAT JSON) "+query, args...).Scan(&raw); err != nil {
			return err
		}
		return json.Unmarshal(raw, &plans)
	})
	if err != nil {
		t.Fatal(err)
	}
	if len(plans) != 1 {
		t.Fatalf("plans = %v", plans)
	}
	return plans[0].Plan
}

// A metered feature check reads one group through the aggregate primary key
// instead of filtering every group of the period by its unindexed group key.
func TestFeatureUsageQueryUsesGroupDigestIndex(t *testing.T) {
	groupKey := `[{"name":"model","string":"large"}]`
	plan := explain(t, featureUsageQuery, uuid.NewString(), uuid.NewString(), time.Now().UTC(), events.GroupDigest(groupKey), groupKey)
	found := false
	plan.walk(func(n planNode) {
		if n.IndexName == "metering_aggregates_pkey" && strings.Contains(n.IndexCond, "group_digest") {
			found = true
		}
	})
	if !found {
		t.Fatalf("feature usage lookup does not use the group digest index condition: %+v", plan)
	}
}

// An aggregate page, including a later cursor page, reads the aggregate primary
// key in order and stops at the page limit instead of sorting every group of
// the period by its unindexed group key.
func TestAggregatePageQueryReadsGroupDigestIndexInOrder(t *testing.T) {
	for label, after := range map[string][]byte{"first page": {}, "cursor page": events.GroupDigest("[]")} {
		plan := explain(t, aggregatePageQuery, uuid.NewString(), uuid.NewString(), time.Now().UTC(), after, 51)
		indexed, sorted := false, false
		plan.walk(func(n planNode) {
			if n.NodeType == "Sort" || n.NodeType == "Incremental Sort" {
				sorted = true
			}
			if n.IndexName == "metering_aggregates_pkey" && strings.Contains(n.IndexCond, "group_digest") {
				indexed = true
			}
		})
		if sorted || !indexed {
			t.Fatalf("%s does not page through the group digest index: %+v", label, plan)
		}
	}
}
