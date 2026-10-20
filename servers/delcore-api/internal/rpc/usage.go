// SPDX-License-Identifier: FSL-1.1-ALv2

package rpc

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"github.com/delinoio/apps/servers/delcore-api/internal/postgres"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/delcore-api/internal/accounting"
	"github.com/jackc/pgx/v5"
	"google.golang.org/protobuf/types/known/timestamppb"
)

func (s *Server) Authorize(ctx context.Context, r *connect.Request[pb.AuthorizeRequest]) (*connect.Response[pb.AuthorizeResponse], error) {
	o := &pb.AuthorizeResponse{}
	return run(s, ctx, r, o, func(c *call) error {
		q := r.Msg
		if e := c.serviceScope("authorize"); e != nil {
			return e
		}
		if !canonical(q.TenantId) || (q.ConsumerAuthorization != pb.ConsumerAuthorization_CONSUMER_AUTHORIZATION_TENANT_KEY && !canonical(q.UserId)) {
			return invalid("invalid tenant or user ID")
		}
		var allowed bool
		var e error
		if q.ConsumerAuthorization == pb.ConsumerAuthorization_CONSUMER_AUTHORIZATION_UNSPECIFIED {
			e = c.tx.QueryRow(c.ctx, `SELECT EXISTS(SELECT 1 FROM members m JOIN tenants t ON t.id=m.tenant_id JOIN users u ON u.id=m.user_id JOIN services s ON s.id=$3
 WHERE m.tenant_id=$1 AND m.user_id=$2 AND u.withdrawing_at IS NULL AND t.deleted_at IS NULL AND $4=ANY(s.capabilities)
 AND (1=ANY(m.roles) OR EXISTS(SELECT 1 FROM group_members gm JOIN group_permissions gp ON gp.group_id=gm.group_id WHERE gm.tenant_id=m.tenant_id AND gm.user_id=m.user_id AND gp.service_id=$3 AND gp.capability=$4)))`, q.TenantId, q.UserId, c.service, q.Capability).Scan(&allowed)
			if e != nil {
				return e
			}
		}
		if q.ConsumerAuthorization != pb.ConsumerAuthorization_CONSUMER_AUTHORIZATION_UNSPECIFIED {
			allowed, e = c.consumerAuthorization(q)
			if e != nil {
				return e
			}
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
   (NOT p.postpaid AND COALESCE((SELECT sum(amount) FROM ledger WHERE tenant_id=t.id AND kind IN ('credit','usage','refund','correction')),0)<=0),
   EXISTS(SELECT 1 FROM entitlements en JOIN prices pr ON pr.id=en.price_id WHERE en.tenant_id=t.id AND pr.service_id=$2 AND en.starts_at<=$3 AND (en.ends_at IS NULL OR en.ends_at>$3))
   FROM tenants t JOIN plans p ON p.id=t.plan_id WHERE t.id=$1`, q.TenantId, c.service, c.now).Scan(&operator, &renewal, &deleting, &dispute, &negative, &depleted, &entitled)
			if e != nil {
				return e
			}
			var callTimeAI bool
			if e = c.tx.QueryRow(c.ctx, "SELECT name='delitools' FROM services WHERE id=$1", c.service).Scan(&callTimeAI); e != nil {
				return e
			}
			if depleted && !callTimeAI {
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
		o.Allowed = len(o.Reasons) == 0
		return nil
	})
}
func (s *Server) ReportUsage(ctx context.Context, r *connect.Request[pb.ReportUsageRequest]) (*connect.Response[pb.ReportUsageResponse], error) {
	o := &pb.ReportUsageResponse{}
	return run(s, ctx, r, o, func(c *call) error { id, e := c.report(r.Msg.Usage, false); o.UsageId = id; return e })
}
func (s *Server) CorrectUsage(ctx context.Context, r *connect.Request[pb.CorrectUsageRequest]) (*connect.Response[pb.CorrectUsageResponse], error) {
	o := &pb.CorrectUsageResponse{}
	return run(s, ctx, r, o, func(c *call) error { id, e := c.report(r.Msg.Usage, true); o.UsageId = id; return e })
}
func (c *call) report(u *pb.Usage, correction bool) (string, error) {
	if e := c.serviceScope("usage:write"); e != nil {
		return "", e
	}
	if u == nil || !canonical(u.TenantId) || u.ServiceId != c.service || u.OccurredAt == nil || u.OccurredAt.CheckValid() != nil {
		return "", invalid("invalid usage attribution or occurrence")
	}
	if e := nonempty(u.EventKey, 200); e != nil {
		return "", e
	}
	if e := nonempty(u.Meter, 100); e != nil {
		return "", e
	}
	quantity, e := accounting.Parse(u.Quantity)
	if e != nil || len(u.Quantity) > 64 || quantity.Cmp(accounting.Zero()) < 0 {
		return "", invalid("quantity must be nonnegative decimal")
	}
	if (u.CorrectionOf != "") != correction || correction && !canonical(u.CorrectionOf) {
		return "", invalid("correction must reference a prior event")
	}
	// PostgreSQL timestamps preserve microseconds; normalize fingerprints and
	// corrections to the same precision before storage.
	occurred := u.OccurredAt.AsTime().UTC().Truncate(time.Microsecond)
	if occurred.After(c.now) || occurred.Before(c.now.Add(-time.Duration(c.s.Config.UsageLatenessSeconds)*time.Second)) {
		return "", invalid("usage occurrence is outside reporting window")
	}
	aiCost, aiJSON, e := c.aiCost(u)
	if e != nil {
		return "", e
	}
	fingerprint := usageFingerprint(u, c.service, quantity.String(), occurred, aiJSON)
	var existing, old string
	e = c.tx.QueryRow(c.ctx, "SELECT id::text,fingerprint FROM usage_events WHERE service_id=$1 AND event_key=$2 UNION ALL SELECT event_id::text,fingerprint FROM usage_replay WHERE service_id=$1 AND event_key=$2 LIMIT 1", c.service, u.EventKey).Scan(&existing, &old)
	if e == nil {
		if old != fingerprint {
			return "", conflict()
		}
		return existing, nil
	}
	if !errors.Is(e, pgx.ErrNoRows) {
		return "", e
	}
	var free, postpaid bool
	var deleting *time.Time
	e = c.tx.QueryRow(c.ctx, "SELECT p.free,p.postpaid,t.deleting_at FROM tenants t JOIN plans p ON p.id=t.plan_id WHERE t.id=$1 AND t.deleted_at IS NULL", u.TenantId).Scan(&free, &postpaid, &deleting)
	if e != nil {
		return "", forbidden()
	}
	if deleting != nil && (occurred.After(*deleting) || c.now.After(deleting.Add(time.Duration(c.s.Config.ReportingGraceSeconds)*time.Second))) {
		return "", precondition("tenant reporting grace has ended")
	}
	start := time.Date(occurred.Year(), occurred.Month(), 1, 0, 0, 0, 0, time.UTC)
	end := start.AddDate(0, 1, 0)
	e = c.tx.QueryRow(c.ctx, "SELECT starts_at,ends_at,postpaid FROM billing_periods WHERE tenant_id=$1 AND starts_at<=$2 AND ends_at>$2", u.TenantId, occurred).Scan(&start, &end, &postpaid)
	if e != nil && !errors.Is(e, pgx.ErrNoRows) {
		return "", e
	}
	historicalPaid := e == nil
	var price string
	e = c.tx.QueryRow(c.ctx, `SELECT p.id::text FROM entitlements en JOIN prices p ON p.id=en.price_id WHERE en.tenant_id=$1 AND p.service_id=$2 AND p.meter=$3 AND p.kind=1 AND en.starts_at<=$4 AND (en.ends_at IS NULL OR en.ends_at>$4) ORDER BY en.starts_at DESC LIMIT 1`, u.TenantId, c.service, u.Meter, occurred).Scan(&price)
	if e != nil {
		return "", precondition("historical price unavailable")
	}

	if !historicalPaid {
		var historicFree bool
		if e = c.tx.QueryRow(c.ctx, "SELECT p.free,p.postpaid FROM prices pr JOIN plans p ON p.id=pr.plan_id WHERE pr.id=$1", price).Scan(&historicFree, &postpaid); e != nil || !historicFree {
			return "", precondition("historical billing period unavailable")
		}
	}
	var reference any
	if correction {
		var originalTenant, originalService, meter, originalQuantity string
		var originalTime time.Time
		e = c.tx.QueryRow(c.ctx, "SELECT tenant_id::text,service_id::text,meter,quantity::text,occurred_at FROM usage_events WHERE id=$1", u.CorrectionOf).Scan(&originalTenant, &originalService, &meter, &originalQuantity, &originalTime)
		if e != nil || originalTenant != u.TenantId || originalService != c.service || meter != u.Meter || !originalTime.Equal(occurred) {
			return "", forbidden()
		}
		if u.AiUsage != nil {
			var raw []byte
			if e = c.tx.QueryRow(c.ctx, "SELECT details FROM ai_usage_prices WHERE usage_id=$1", u.CorrectionOf).Scan(&raw); e != nil {
				return "", e
			}
			originalAI := &pb.AiUsage{}
			if e = json.Unmarshal(raw, originalAI); e != nil {
				return "", e
			}
			if !sameAIPrices(originalAI, u.AiUsage) {
				return "", invalid("AI corrections must preserve the original price snapshot")
			}
		}
		_ = originalQuantity
		reference = u.CorrectionOf
	}
	// The original event's price remains authoritative across correction chains.
	// Re-rate the period after insertion; each event retains its occurrence order.
	var priorCharge string
	e = c.tx.QueryRow(c.ctx, "SELECT (-COALESCE(sum(amount),0))::text FROM ledger WHERE tenant_id=$1 AND service_id=$2 AND meter=$3 AND period_start=$4 AND kind IN ('usage','postpaid','correction')", u.TenantId, c.service, u.Meter, start).Scan(&priorCharge)
	if e != nil {
		return "", e
	}
	id := ID()
	_, e = c.tx.Exec(c.ctx, `INSERT INTO usage_events VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)`, id, u.TenantId, c.service, u.EventKey, fingerprint, u.Meter, quantity.String(), occurred, c.now, price, start, end, reference)
	if e != nil {
		return "", e
	}
	if u.AiUsage != nil {
		if _, e = c.tx.Exec(c.ctx, "INSERT INTO ai_usage_prices(usage_id,details,cost) VALUES($1,$2,$3)", id, aiJSON, aiCost.String()); e != nil {
			return "", e
		}
	}
	total, _, e := c.ratePeriod(u.TenantId, c.service, u.Meter, start)
	if e != nil {
		return "", e
	}
	prior, _ := accounting.Parse(priorCharge)
	cost := total.Sub(prior)
	kind := "usage"
	if correction {
		kind = "correction"
	}
	if cost.Cmp(accounting.Zero()) < 0 {
		// Reverse unpaid usage before returning prepaid money. This prevents a
		// correction of postpaid usage from minting spendable purchased credits.
		var unpaidText string
		if e = c.tx.QueryRow(c.ctx, "SELECT greatest(-COALESCE(sum(amount),0),0)::text FROM ledger WHERE tenant_id=$1 AND service_id=$2 AND meter=$3 AND period_start=$4 AND kind='postpaid'", u.TenantId, c.service, u.Meter, start).Scan(&unpaidText); e != nil {
			return "", e
		}
		unpaid, _ := accounting.Parse(unpaidText)
		refund := cost.Neg()
		if refund.Cmp(unpaid) > 0 {
			refund = unpaid
		}
		if refund.Cmp(accounting.Zero()) > 0 {
			if e = c.ledger(u.TenantId, c.service, u.Meter, id, "postpaid", refund.String(), start, end, "postpaid:"+id); e != nil {
				return "", e
			}
			cost = cost.Add(refund)
		}
	} else if postpaid && cost.Cmp(accounting.Zero()) > 0 {
		var balance string
		if e = c.tx.QueryRow(c.ctx, "SELECT greatest(COALESCE(sum(amount),0),0)::text FROM ledger WHERE tenant_id=$1 AND kind IN ('credit','usage','refund','correction')", u.TenantId).Scan(&balance); e != nil {
			return "", e
		}
		b, _ := accounting.Parse(balance)
		if cost.Cmp(b) > 0 {
			unpaid := cost.Sub(b)
			if e = c.ledger(u.TenantId, c.service, u.Meter, id, "postpaid", unpaid.Neg().String(), start, end, "postpaid:"+id); e != nil {
				return "", e
			}
			cost = b
		}
	}
	if e = c.ledger(u.TenantId, c.service, u.Meter, id, kind, cost.Neg().String(), start, end, "usage:"+id); e != nil {
		return "", e
	}
	return id, nil
}
func (c *call) ledger(tenant, service, meter, usage, kind, amount string, start, end time.Time, key string) error {
	_, e := c.tx.Exec(c.ctx, `INSERT INTO ledger VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)`, ID(), tenant, service, meter, usage, kind, amount, start, end, key, c.now)
	return e
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
		rows, e := c.tx.Query(c.ctx, "SELECT u.id::text,u.tenant_id::text,u.service_id::text,u.event_key,u.meter,u.quantity::text,u.occurred_at,COALESCE(u.correction_of::text,''),a.details FROM usage_events u LEFT JOIN ai_usage_prices a ON a.usage_id=u.id WHERE u.tenant_id=$1 AND u.id>$2 AND ($4='' OR u.service_id::text=$4) ORDER BY u.id LIMIT $3", r.Msg.TenantId, after, n+1, c.service)
		if e != nil {
			return e
		}
		defer rows.Close()
		for rows.Next() {
			u := &pb.Usage{}
			var details []byte
			var at time.Time
			if e = rows.Scan(&u.Id, &u.TenantId, &u.ServiceId, &u.EventKey, &u.Meter, &u.Quantity, &at, &u.CorrectionOf, &details); e != nil {
				return e
			}
			if len(details) > 0 {
				u.AiUsage = &pb.AiUsage{}
				if e = json.Unmarshal(details, u.AiUsage); e != nil {
					return e
				}
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

// Preserve the pre-AI encoding for durable retries of ordinary usage.
func usageFingerprint(u *pb.Usage, service, quantity string, occurred time.Time, aiJSON []byte) string {
	canonicalInput := struct {
		Tenant, Service, Key, Meter, Quantity, Correction string
		Time                                              time.Time
		AI                                                string `json:",omitempty"`
	}{u.TenantId, service, u.EventKey, u.Meter, quantity, u.CorrectionOf, occurred, string(aiJSON)}
	raw, _ := json.Marshal(canonicalInput)
	sum := sha256.Sum256(raw)
	return hex.EncodeToString(sum[:])
}
