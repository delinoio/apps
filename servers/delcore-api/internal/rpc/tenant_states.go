// SPDX-License-Identifier: FSL-1.1-ALv2

package rpc

import (
	"context"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/delcore/v1"
)

// GetTenantStates reports the billing-independent state of known tenants to
// registered services. Operator blocking and deletion are the only inputs:
// subscription, renewal, dispute and balance conditions never affect the
// result. Precedence is DELETED > DELETING > BLOCKED > ACTIVE, and an unknown
// tenant is UNKNOWN. The call is read-only and returns no personal or billing
// data.
func (s *Server) GetTenantStates(ctx context.Context, r *connect.Request[pb.GetTenantStatesRequest]) (*connect.Response[pb.GetTenantStatesResponse], error) {
	out := &pb.GetTenantStatesResponse{}
	return run(s, ctx, r, out, func(c *call) error {
		if e := c.serviceScope("authorize"); e != nil {
			return e
		}
		ids := r.Msg.TenantIds
		if len(ids) == 0 || len(ids) > 100 {
			return invalid("supply 1 to 100 known tenant IDs")
		}
		for _, id := range ids {
			if !canonical(id) {
				return invalid("invalid tenant ID")
			}
		}
		rows, e := c.tx.Query(c.ctx, "SELECT id::text,deleted_at IS NOT NULL,deleting_at IS NOT NULL,operator_block FROM tenants WHERE id=ANY($1::uuid[])", ids)
		if e != nil {
			return e
		}
		defer rows.Close()
		known := make(map[string]pb.TenantState, len(ids))
		for rows.Next() {
			var id string
			var deleted, deleting, blocked bool
			if e = rows.Scan(&id, &deleted, &deleting, &blocked); e != nil {
				return e
			}
			switch {
			case deleted:
				known[id] = pb.TenantState_TENANT_STATE_DELETED
			case deleting:
				known[id] = pb.TenantState_TENANT_STATE_DELETING
			case blocked:
				known[id] = pb.TenantState_TENANT_STATE_BLOCKED
			default:
				known[id] = pb.TenantState_TENANT_STATE_ACTIVE
			}
		}
		if e = rows.Err(); e != nil {
			return e
		}
		counts := map[string]int{}
		for _, id := range ids {
			state, ok := known[id]
			if !ok {
				state = pb.TenantState_TENANT_STATE_UNKNOWN
			}
			counts[state.String()]++
			out.States = append(out.States, &pb.TenantStateEntry{TenantId: id, State: state})
		}
		// Log only aggregate counts; tenant IDs and state details stay in the response.
		c.s.Log.InfoContext(c.ctx, "tenant states read", "correlation_id", c.correlation, "service_id", c.service, "requested", len(ids), "states", counts)
		return nil
	})
}
