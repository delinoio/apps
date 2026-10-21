// SPDX-License-Identifier: FSL-1.1-ALv2

package rpc

import (
	"context"
	"github.com/delinoio/apps/servers/delcore-api/internal/postgres"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"google.golang.org/protobuf/types/known/timestamppb"
)

func (s *Server) Authorize(ctx context.Context, r *connect.Request[pb.AuthorizeRequest]) (*connect.Response[pb.AuthorizeResponse], error) {
	o := &pb.AuthorizeResponse{}
	return run(s, ctx, r, o, func(c *call) error {
		q := r.Msg
		if e := c.serviceScope("authorize"); e != nil {
			return e
		}
		if !canonical(q.TenantId) || !canonical(q.UserId) {
			return invalid("invalid tenant or user ID")
		}
		// Feature authorization is additive: an empty feature key leaves the
		// permission and billing result below unchanged.
		feature, fe := c.featureRequest(q)
		if fe != nil {
			return fe
		}
		var allowed bool
		e := c.tx.QueryRow(c.ctx, `SELECT EXISTS(SELECT 1 FROM members m JOIN tenants t ON t.id=m.tenant_id JOIN users u ON u.id=m.user_id JOIN services s ON s.id=$3
 WHERE m.tenant_id=$1 AND m.user_id=$2 AND u.withdrawing_at IS NULL AND t.deleted_at IS NULL AND $4=ANY(s.capabilities)
 AND (1=ANY(m.roles) OR EXISTS(SELECT 1 FROM group_members gm JOIN group_permissions gp ON gp.group_id=gm.group_id WHERE gm.tenant_id=m.tenant_id AND gm.user_id=m.user_id AND gp.service_id=$3 AND gp.capability=$4)))`, q.TenantId, q.UserId, c.service, q.Capability).Scan(&allowed)
		if e != nil {
			return e
		}
		if !allowed {
			o.Reasons = append(o.Reasons, pb.BlockReason_BLOCK_REASON_PERMISSION)
			return nil
		}
		if q.Paid {
			// A paid tenant is renewal-blocked when the provider failed a renewal
			// or when the confirmed paid period has ended without a matching
			// renewal advancing it. Free plans without a subscription roll over by
			// calendar month in maintenance and never expire here; a subscription, even a recurring
			// add-on on a free base plan, expires. The check uses the request clock
			// and does not mutate state or call the provider.
			var operator, renewal, deleting, dispute, negative, depleted, entitled bool
			e = c.tx.QueryRow(c.ctx, `SELECT t.operator_block,(t.renewal_failed OR `+postgres.PeriodExpiredSQL("t", "p", "$3")+`),t.deleting_at IS NOT NULL,
   EXISTS(SELECT 1 FROM disputes WHERE tenant_id=t.id AND active),
   COALESCE((SELECT sum(amount) FROM ledger WHERE tenant_id=t.id AND kind IN ('credit','usage','refund','correction')),0)<0,
   (NOT `+postgres.SelectedPlanBoolSQL("t", "p", "postpaid")+` AND COALESCE((SELECT sum(amount) FROM ledger WHERE tenant_id=t.id AND kind IN ('credit','usage','refund','correction')),0)<=0),
   EXISTS(SELECT 1 FROM entitlements en JOIN prices pr ON pr.id=en.price_id WHERE en.tenant_id=t.id AND pr.service_id=$2 AND en.starts_at<=$3 AND (en.ends_at IS NULL OR en.ends_at>$3))
   FROM tenants t JOIN plans p ON p.id=t.plan_id WHERE t.id=$1`, q.TenantId, c.service, c.now).Scan(&operator, &renewal, &deleting, &dispute, &negative, &depleted, &entitled)
			if e != nil {
				return e
			}
			if depleted {
				var periodStart time.Time
				if e = c.tx.QueryRow(c.ctx, "SELECT period_start FROM tenants WHERE id=$1", q.TenantId).Scan(&periodStart); e != nil {
					return e
				}
				remaining, e := postgres.HasAllowance(c.ctx, c.tx, q.TenantId, c.service, periodStart)
				if e != nil {
					return e
				}
				depleted = !remaining
			}
			for _, v := range []struct {
				blocked bool
				reason  pb.BlockReason
			}{{operator, pb.BlockReason_BLOCK_REASON_OPERATOR}, {renewal, pb.BlockReason_BLOCK_REASON_RENEWAL}, {deleting, pb.BlockReason_BLOCK_REASON_DELETING}, {dispute, pb.BlockReason_BLOCK_REASON_DISPUTE}, {negative || depleted, pb.BlockReason_BLOCK_REASON_BALANCE}, {!entitled, pb.BlockReason_BLOCK_REASON_PERMISSION}} {
				if v.blocked {
					o.Reasons = append(o.Reasons, v.reason)
				}
			}
		}
		if e = c.authorizeFeature(feature, q.TenantId, o); e != nil {
			return e
		}
		o.Allowed = len(o.Reasons) == 0
		return nil
	})
}

// ReportUsage is the retired quantity-reporting call. It stays declared for
// wire compatibility, never bills, and directs callers to event ingestion.
func (s *Server) ReportUsage(ctx context.Context, r *connect.Request[pb.ReportUsageRequest]) (*connect.Response[pb.ReportUsageResponse], error) {
	o := &pb.ReportUsageResponse{}
	return run(s, ctx, r, o, func(c *call) error {
		return c.retiredUsage("ReportUsage is retired; report measured events with IngestEvent")
	})
}

// CorrectUsage is the retired quantity-correction call. It stays declared for
// wire compatibility, never bills, and directs callers to event corrections.
func (s *Server) CorrectUsage(ctx context.Context, r *connect.Request[pb.CorrectUsageRequest]) (*connect.Response[pb.CorrectUsageResponse], error) {
	o := &pb.CorrectUsageResponse{}
	return run(s, ctx, r, o, func(c *call) error {
		return c.retiredUsage("CorrectUsage is retired; correct accepted events with CorrectEvent")
	})
}

// retiredUsage keeps the previous service-scope check, so callers without
// usage:write still receive a permission error, and then rejects the call
// with a precondition failure that names the replacement API.
func (c *call) retiredUsage(message string) error {
	if e := c.serviceScope("usage:write"); e != nil {
		return e
	}
	c.s.Log.WarnContext(c.ctx, "retired usage call rejected", "category", "usage_reporting", "correlation_id", c.correlation, "service_id", c.service)
	return precondition(message)
}
func (s *Server) ListUsage(ctx context.Context, r *connect.Request[pb.ListUsageRequest]) (*connect.Response[pb.ListUsageResponse], error) {
	o := &pb.ListUsageResponse{}
	return run(s, ctx, r, o, func(c *call) error {
		if c.service != "" {
			if e := c.serviceScope("usage:read"); e != nil {
				return e
			}
			if !canonical(r.Msg.TenantId) {
				return invalid("invalid tenant ID")
			}
		} else if e := c.require(r.Msg.TenantId, pb.Role_ROLE_BILLING); e != nil {
			return e
		}
		n, after, e := page(r.Msg.Page)
		if e != nil {
			return e
		}
		rows, e := c.tx.Query(c.ctx, "SELECT id::text,tenant_id::text,service_id::text,event_key,meter,quantity::text,occurred_at,COALESCE(correction_of::text,'') FROM usage_events WHERE tenant_id=$1 AND id>$2 AND ($4='' OR service_id::text=$4) ORDER BY id LIMIT $3", r.Msg.TenantId, after, n+1, c.service)
		if e != nil {
			return e
		}
		defer rows.Close()
		for rows.Next() {
			u := &pb.Usage{}
			var at time.Time
			if e = rows.Scan(&u.Id, &u.TenantId, &u.ServiceId, &u.EventKey, &u.Meter, &u.Quantity, &at, &u.CorrectionOf); e != nil {
				return e
			}
			u.OccurredAt = timestamppb.New(at)
			o.Events = append(o.Events, u)
		}
		if len(o.Events) > n {
			o.Events = o.Events[:n]
			o.NextAfterId = o.Events[n-1].Id
		}
		return rows.Err()
	})
}
func (s *Server) ListLedger(ctx context.Context, r *connect.Request[pb.ListLedgerRequest]) (*connect.Response[pb.ListLedgerResponse], error) {
	o := &pb.ListLedgerResponse{}
	return run(s, ctx, r, o, func(c *call) error {
		if e := c.require(r.Msg.TenantId, pb.Role_ROLE_BILLING); e != nil {
			return e
		}
		n, after, e := page(r.Msg.Page)
		if e != nil {
			return e
		}
		rows, e := c.tx.Query(c.ctx, "SELECT id::text,kind,amount::text,meter,created_at,COALESCE(usage_id::text,'') FROM ledger WHERE tenant_id=$1 AND id>$2 ORDER BY id LIMIT $3", r.Msg.TenantId, after, n+1)
		if e != nil {
			return e
		}
		defer rows.Close()
		for rows.Next() {
			v := &pb.LedgerEntry{}
			var at time.Time
			if e = rows.Scan(&v.Id, &v.Kind, &v.Amount, &v.Meter, &at, &v.UsageId); e != nil {
				return e
			}
			v.CreatedAt = timestamppb.New(at)
			o.Entries = append(o.Entries, v)
		}
		if len(o.Entries) > n {
			o.Entries = o.Entries[:n]
			o.NextAfterId = o.Entries[n-1].Id
		}
		return rows.Err()
	})
}

// CheckTenantLifecycle lets registered services purge their retained tenant data
// only after Delcore has completed settlement and finalized deletion.
func (s *Server) CheckTenantLifecycle(ctx context.Context, r *connect.Request[pb.CheckTenantLifecycleRequest]) (*connect.Response[pb.CheckTenantLifecycleResponse], error) {
	out := &pb.CheckTenantLifecycleResponse{}
	return run(s, ctx, r, out, func(c *call) error {
		if e := c.serviceScope("authorize"); e != nil {
			return e
		}
		if len(r.Msg.TenantIds) == 0 || len(r.Msg.TenantIds) > 100 {
			return invalid("supply 1 to 100 known tenant IDs")
		}
		for _, id := range r.Msg.TenantIds {
			if !canonical(id) {
				return invalid("invalid tenant ID")
			}
		}
		rows, e := c.tx.Query(c.ctx, "SELECT id::text FROM tenants WHERE id=ANY($1::uuid[]) AND deleted_at IS NOT NULL ORDER BY id", r.Msg.TenantIds)
		if e != nil {
			return e
		}
		defer rows.Close()
		for rows.Next() {
			var id string
			if e = rows.Scan(&id); e != nil {
				return e
			}
			out.FinalizedTenantIds = append(out.FinalizedTenantIds, id)
		}
		return rows.Err()
	})
}
