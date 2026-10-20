// SPDX-License-Identifier: FSL-1.1-ALv2
package api

import (
	"context"
	"encoding/json"
	pb "github.com/delinoio/apps/protos/delitools/v1"
	"github.com/jackc/pgx/v5"
	"time"
)

func (s *Server) SyncPrices(ctx context.Context) (err error) {
	defer func() {
		if err != nil {
			s.event("price_sync_failure", ID())
		}
	}()
	catalog, e := s.Gateway.Catalog(ctx)
	if e != nil {
		return e
	}
	return begin(ctx, s.DB, func(tx pgx.Tx) error {
		// Claims must observe every price whose effective time precedes their start.
		// Share the claim lock through commit, and timestamp only after acquiring it.
		if _, e := tx.Exec(ctx, "SELECT pg_advisory_xact_lock(41824703)"); e != nil {
			return e
		}
		now := s.now()
		for model, p := range catalog {
			if !p.Valid() {
				continue
			}
			raw, _ := json.Marshal(p)
			if _, e := tx.Exec(ctx, `INSERT INTO catalog SELECT $1,$2::jsonb,$3 WHERE $2::jsonb IS DISTINCT FROM (SELECT price FROM catalog WHERE model=$1 ORDER BY effective_at DESC LIMIT 1) ON CONFLICT(model,effective_at) DO NOTHING`, model, raw, now); e != nil {
				return e
			}
		}
		return nil
	})
}

type pending struct {
	id, tenant, model, generation string
	cap                           pb.Capability
	price, counts                 []byte
	effective, occurred, created  time.Time
	failures                      int
}

func (s *Server) Deliver(ctx context.Context, v *pb.Settings) error {
	now := s.now()
	rows, e := s.DB.Pool.Query(ctx, `SELECT id::text,tenant_id::text,capability,model,price,price_effective_at,occurred_at,generation_id,usage,created_at,failures FROM attempts WHERE NOT delivered AND retry_at<=$1 AND created_at>$2 ORDER BY retry_at LIMIT 100`, now, now.Add(-Retention))
	if e != nil {
		return e
	}
	batch := []pending{}
	for rows.Next() {
		var p pending
		if e = rows.Scan(&p.id, &p.tenant, &p.cap, &p.model, &p.price, &p.effective, &p.occurred, &p.generation, &p.counts, &p.created, &p.failures); e != nil {
			rows.Close()
			return e
		}
		batch = append(batch, p)
	}
	rows.Close()
	if e = rows.Err(); e != nil {
		return e
	}
	for _, p := range batch {
		attemptCtx, cancel := context.WithTimeout(ctx, time.Duration(v.ExecutionTimeoutSeconds)*time.Second)
		var n *Counts
		if p.counts != nil {
			n = &Counts{}
			e = json.Unmarshal(p.counts, n)
		} else {
			n, e = s.Gateway.Lookup(attemptCtx, p.generation, p.model)
		}
		if e == nil && n != nil {
			raw, _ := json.Marshal(n)
			_, e = s.DB.Pool.Exec(attemptCtx, "UPDATE attempts SET usage=$2 WHERE id=$1", p.id, raw)
			var price Price
			if e == nil {
				e = json.Unmarshal(p.price, &price)
			}
			if e == nil {
				u, buildErr := usage(p.tenant, s.ServiceID, p.id, p.model, capability(p.cap), *n, price, p.effective, p.occurred)
				e = buildErr
				if e == nil {
					e = s.Auth.Report(attemptCtx, u, p.id)
				}
			}
		}
		if e == nil && n != nil {
			_, e = s.DB.Pool.Exec(attemptCtx, "UPDATE attempts SET delivered=true WHERE id=$1", p.id)
		}
		cancel()
		if e != nil || n == nil {
			delay := uint64(v.DeliveryRetrySeconds)
			for i := 0; i < min(p.failures, 32) && delay < uint64(v.MaxBackoffSeconds); i++ {
				delay = min(delay*2, uint64(v.MaxBackoffSeconds))
			}
			if _, err := s.DB.Pool.Exec(ctx, "UPDATE attempts SET retry_at=$2,failures=least(failures,2147483646)+1 WHERE id=$1", p.id, now.Add(time.Duration(delay)*time.Second)); err != nil {
				return err
			}
			s.event("usage_pending", p.id)
		}
	}
	return nil
}
func (s *Server) Background(ctx context.Context) error {
	nextSync := time.Time{}
	for {
		v, e := s.DB.Settings(ctx)
		if e != nil {
			return e
		}
		if !s.now().Before(nextSync) {
			call, cancel := context.WithTimeout(ctx, time.Duration(v.ExecutionTimeoutSeconds)*time.Second)
			_ = s.SyncPrices(call)
			cancel()
			nextSync = s.now().Add(time.Duration(v.PriceSyncSeconds) * time.Second)
		}
		if e = s.DB.Expire(ctx, s.now()); e != nil {
			return e
		}
		if e = s.Deliver(ctx, v); e != nil {
			s.event("delivery_failure", ID())
		}
		s.flushEvents(ctx)
		if e = s.Alerts(ctx); e != nil {
			s.event("alert_query_failure", ID())
		}
		timer := time.NewTimer(time.Duration(v.DeliveryRetrySeconds) * time.Second)
		select {
		case <-ctx.Done():
			timer.Stop()
			return ctx.Err()
		case <-timer.C:
		}
	}
}
