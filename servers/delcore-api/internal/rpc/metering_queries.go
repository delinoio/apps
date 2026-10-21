// SPDX-License-Identifier: FSL-1.1-ALv2

package rpc

import (
	"context"
	"encoding/base64"
	"errors"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/delcore-api/internal/accounting"
	"github.com/delinoio/apps/servers/delcore-api/internal/events"
	"github.com/delinoio/apps/servers/delcore-api/internal/features"
	"github.com/delinoio/apps/servers/delcore-api/internal/postgres"
	"github.com/jackc/pgx/v5"
	"google.golang.org/protobuf/types/known/timestamppb"
)

// maxAggregateCursorBytes bounds a decoded group-key cursor. Group keys derive
// from at most the catalog group-by and attribute value limits.
const maxAggregateCursorBytes = 1 << 16

// aggregatePage applies the shared page size rules. Aggregates are keyed by
// their group key rather than a UUID, so the cursor is an opaque base64url
// encoding of the last returned group key.
func aggregatePage(p *pb.Page) (int, string, error) {
	n := 50
	cursor := ""
	if p != nil {
		if p.Size != 0 {
			n = int(p.Size)
		}
		cursor = p.AfterId
	}
	if n < 1 || n > 100 {
		return 0, "", invalid("page size must be 1 to 100")
	}
	if cursor == "" {
		return n, "", nil
	}
	raw, err := base64.RawURLEncoding.DecodeString(cursor)
	if err != nil || len(raw) == 0 || len(raw) > maxAggregateCursorBytes {
		return 0, "", invalid("invalid cursor")
	}
	return n, string(raw), nil
}

// ListMeterAggregates returns applied group aggregates of one tenant meter
// billing period. Results come from asynchronous processing: a successful
// ingestion or correction does not imply that a listed aggregate includes it.
func (s *Server) ListMeterAggregates(ctx context.Context, r *connect.Request[pb.ListMeterAggregatesRequest]) (*connect.Response[pb.ListMeterAggregatesResponse], error) {
	o := &pb.ListMeterAggregatesResponse{}
	return run(s, ctx, r, o, func(c *call) error {
		q := r.Msg
		service := q.ServiceId
		if c.service != "" {
			// Services read only their own meters, for tenants they report for.
			if e := c.serviceScope("usage:read"); e != nil {
				return e
			}
			if !canonical(q.TenantId) {
				return invalid("invalid tenant ID")
			}
			if service == "" {
				service = c.service
			}
			if service != c.service {
				return forbidden()
			}
		} else {
			if e := c.require(q.TenantId, pb.Role_ROLE_BILLING); e != nil {
				return e
			}
			if !canonical(service) {
				return invalid("invalid service ID")
			}
		}
		if !catalogKeyPattern.MatchString(q.MeterKey) {
			return invalid("invalid meter key")
		}
		n, after, e := aggregatePage(q.Page)
		if e != nil {
			return e
		}
		at := c.now
		if q.PeriodAt != nil {
			if q.PeriodAt.CheckValid() != nil {
				return invalid("invalid period instant")
			}
			at = q.PeriodAt.AsTime()
		}
		var raw []byte
		e = c.tx.QueryRow(c.ctx, `SELECT ce.id::text,cv.definition FROM catalog_entities ce LEFT JOIN catalog_versions cv ON cv.id=ce.active_version_id
 WHERE ce.kind='meter' AND ce.scope=$1 AND ce.key=$2`, service, q.MeterKey).Scan(&o.MeterId, &raw)
		if errors.Is(e, pgx.ErrNoRows) {
			return invalid("unknown meter")
		}
		if e != nil {
			return e
		}
		definition, e := features.DecodeMeter(raw)
		if e != nil {
			return e
		}
		o.Aggregation = definition.Aggregation
		start, end, e := postgres.BillingPeriod(c.ctx, c.tx, q.TenantId, at)
		if e != nil {
			return e
		}
		o.PeriodStart, o.PeriodEnd = timestamppb.New(start), timestamppb.New(end)
		rows, e := c.tx.Query(c.ctx, `SELECT group_key,group_values,value::text,event_count,sum::text,distinct_count,updated_at FROM metering_aggregates
 WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3 AND group_key>$4 ORDER BY group_key LIMIT $5`, q.TenantId, o.MeterId, start, after, n+1)
		if e != nil {
			return e
		}
		defer rows.Close()
		var last []string
		for rows.Next() {
			var group, value, sum string
			var values []byte
			var updated time.Time
			v := &pb.MeterAggregate{}
			if e = rows.Scan(&group, &values, &value, &v.EventCount, &sum, &v.DistinctCount, &updated); e != nil {
				return e
			}
			decoded, e := events.DecodeValues(values)
			if e != nil {
				return e
			}
			if v.Value, e = canonicalDecimal(value); e != nil {
				return e
			}
			if v.Sum, e = canonicalDecimal(sum); e != nil {
				return e
			}
			v.GroupValues, v.UpdatedAt = wireAttributes(decoded), timestamppb.New(updated)
			o.Aggregates = append(o.Aggregates, v)
			last = append(last, group)
		}
		if e = rows.Err(); e != nil {
			return e
		}
		if len(o.Aggregates) > n {
			o.Aggregates = o.Aggregates[:n]
			o.NextAfterId = base64.RawURLEncoding.EncodeToString([]byte(last[n-1]))
		}
		s.Log.InfoContext(c.ctx, "meter aggregates listed", "category", "metering_query", "correlation_id", c.correlation, "service_id", c.service, "meter_key", q.MeterKey, "aggregates", len(o.Aggregates))
		return nil
	})
}

// canonicalDecimal removes the trailing zeros PostgreSQL keeps for numeric
// scale so exposed values match ingestion's canonical decimals.
func canonicalDecimal(text string) (string, error) {
	d, err := accounting.Parse(text)
	if err != nil {
		return "", err
	}
	return d.String(), nil
}

// meteringFailureCategories maps the worker's bounded job error categories.
var meteringFailureCategories = map[string]pb.MeteringFailureCategory{
	"metering_transient": pb.MeteringFailureCategory_METERING_FAILURE_CATEGORY_TRANSIENT,
	"retry_exhausted":    pb.MeteringFailureCategory_METERING_FAILURE_CATEGORY_RETRY_EXHAUSTED,
	"metering_invalid":   pb.MeteringFailureCategory_METERING_FAILURE_CATEGORY_INVALID_STATE,
	"invalid_payload":    pb.MeteringFailureCategory_METERING_FAILURE_CATEGORY_INVALID_PAYLOAD,
}

// GetMeteringStatus reports processing lag and failure state from unfinished
// metering jobs. Tenant billing users read their tenant, services read the
// events they reported for a tenant, and platform operators read one tenant or
// the whole service. The call is read-only; recovery stays with RetryJob.
func (s *Server) GetMeteringStatus(ctx context.Context, r *connect.Request[pb.GetMeteringStatusRequest]) (*connect.Response[pb.GetMeteringStatusResponse], error) {
	o := &pb.GetMeteringStatusResponse{}
	return run(s, ctx, r, o, func(c *call) error {
		var tenant, service any
		scope := "tenant"
		switch {
		case c.service != "":
			if e := c.serviceScope("usage:read"); e != nil {
				return e
			}
			if !canonical(r.Msg.TenantId) {
				return invalid("invalid tenant ID")
			}
			tenant, service = r.Msg.TenantId, c.service
		case r.Msg.TenantId == "":
			if !c.identity.Operator {
				return forbidden()
			}
			scope = "service_wide"
		case c.identity.Operator:
			if !canonical(r.Msg.TenantId) {
				return invalid("invalid tenant ID")
			}
			tenant = r.Msg.TenantId
		default:
			if e := c.require(r.Msg.TenantId, pb.Role_ROLE_BILLING); e != nil {
				return e
			}
			tenant = r.Msg.TenantId
		}
		// Unfinished metering jobs use the jobs_metering_unfinished index. The
		// guarded cast lets the join use the event primary key while an
		// unreadable payload joins no event and still counts as failed work.
		const work = `FROM jobs j LEFT JOIN metering_events e ON e.id=CASE WHEN j.payload->>'event_id' ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$' THEN (j.payload->>'event_id')::uuid END
 WHERE j.kind='` + events.JobKind + `' AND j.state<>'done' AND ($1::uuid IS NULL OR j.tenant_id=$1) AND ($2::uuid IS NULL OR e.service_id=$2)`
		var oldest *time.Time
		if e := c.tx.QueryRow(c.ctx, `SELECT count(DISTINCT e.id) FILTER(WHERE e.applied_revision<e.latest_revision),
 count(*) FILTER(WHERE j.state IN ('pending','running')),min(j.created_at) FILTER(WHERE j.state IN ('pending','running')),
 count(*) FILTER(WHERE j.state='failed') `+work, tenant, service).Scan(&o.UnappliedEvents, &o.PendingJobs, &oldest, &o.FailedJobs); e != nil {
			return e
		}
		if oldest != nil {
			o.OldestPendingAt = timestamppb.New(*oldest)
			o.LagSeconds = max(int64(c.now.Sub(*oldest)/time.Second), 0)
		}
		rows, e := c.tx.Query(c.ctx, `SELECT j.error_category,count(*),max(j.updated_at) `+work+` AND j.error_category<>'' GROUP BY j.error_category ORDER BY j.error_category`, tenant, service)
		if e != nil {
			return e
		}
		defer rows.Close()
		counts := map[pb.MeteringFailureCategory]int64{}
		var latest time.Time
		for rows.Next() {
			var category string
			var n int64
			var at time.Time
			if e = rows.Scan(&category, &n, &at); e != nil {
				return e
			}
			typed := meteringFailureCategories[category]
			counts[typed] += n
			if at.After(latest) {
				latest, o.LastFailureCategory = at, typed
			}
		}
		if e = rows.Err(); e != nil {
			return e
		}
		for category := pb.MeteringFailureCategory_METERING_FAILURE_CATEGORY_UNSPECIFIED; category <= pb.MeteringFailureCategory_METERING_FAILURE_CATEGORY_INVALID_PAYLOAD; category++ {
			if n := counts[category]; n > 0 {
				o.Failures = append(o.Failures, &pb.MeteringFailureCount{Category: category, Jobs: n})
			}
		}
		if !latest.IsZero() {
			o.LastFailureAt = timestamppb.New(latest)
		}
		s.Log.InfoContext(c.ctx, "metering status read", "category", "metering_query", "correlation_id", c.correlation, "scope", scope, "service_id", c.service, "pending_jobs", o.PendingJobs, "failed_jobs", o.FailedJobs, "lag_seconds", o.LagSeconds)
		return nil
	})
}
