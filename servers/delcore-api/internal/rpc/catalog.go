// SPDX-License-Identifier: FSL-1.1-ALv2

package rpc

import (
	"connectrpc.com/connect"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/delcore-api/internal/accounting"
	"github.com/delinoio/apps/servers/delcore-api/internal/postgres"
	"github.com/jackc/pgx/v5"
)

func (s *Server) PutService(ctx context.Context, r *connect.Request[pb.PutServiceRequest]) (*connect.Response[pb.PutServiceResponse], error) {
	o := &pb.PutServiceResponse{}
	return run(s, ctx, r, o, func(c *call) error {
		q := r.Msg
		if e := c.operator(q.Reason); e != nil {
			return e
		}
		v := q.Service
		if v == nil {
			return invalid("service required")
		}
		if e := nonempty(v.Name, 200); e != nil {
			return e
		}
		if e := nonempty(v.ClientId, 200); e != nil {
			return e
		}
		if len(v.Capabilities) == 0 || len(v.Capabilities) > 100 || len(v.Scopes) > 10 {
			return invalid("invalid service permissions")
		}
		for _, cap := range v.Capabilities {
			if e := nonempty(cap, 100); e != nil {
				return e
			}
		}
		for _, scope := range v.Scopes {
			if scope != "authorize" && scope != "usage:write" && scope != "usage:read" {
				return invalid("unknown service scope")
			}
		}
		if v.Id != "" && !canonical(v.Id) {
			return invalid("invalid service ID")
		}
		var e error
		v.Key, e = c.catalogKey(v.Id, v.Key, v.Name)
		if e != nil {
			return e
		}
		v.Id, e = c.catalogEntity("service", "", v.Key, v.Id)
		if e != nil {
			return e
		}
		_, e = c.tx.Exec(c.ctx, `INSERT INTO services VALUES($1,$2,$3,$4,$5) ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name,client_id=EXCLUDED.client_id,capabilities=EXCLUDED.capabilities,scopes=EXCLUDED.scopes`, v.Id, v.Name, v.ClientId, v.Capabilities, v.Scopes)
		if e != nil {
			return e
		}
		o.Service = v
		return c.audit("", "catalog.service", q.Reason)
	})
}
func (s *Server) PutPlan(ctx context.Context, r *connect.Request[pb.PutPlanRequest]) (*connect.Response[pb.PutPlanResponse], error) {
	o := &pb.PutPlanResponse{}
	return run(s, ctx, r, o, func(c *call) error {
		q := r.Msg
		if e := c.operator(q.Reason); e != nil {
			return e
		}
		v := q.Plan
		if v == nil {
			return invalid("plan required")
		}
		if e := nonempty(v.Name, 200); e != nil {
			return e
		}
		if v.Rank < 0 || v.DefaultFree && !v.Free {
			return invalid("invalid plan")
		}
		if v.Free && v.Postpaid {
			c.s.Log.WarnContext(c.ctx, "rejected free postpaid plan", "correlation_id", c.correlation, "plan_id", v.Id, "default_free", v.DefaultFree)
			return invalid("free plans cannot enable postpaid continuation")
		}
		if v.Id != "" && !canonical(v.Id) {
			return invalid("invalid plan ID")
		}
		var e error
		v.Key, e = c.catalogKey(v.Id, v.Key, v.Name)
		if e != nil {
			return e
		}
		v.Id, e = c.catalogEntity("plan", "", v.Key, v.Id)
		if e != nil {
			return e
		}
		var existing bool
		if e = c.tx.QueryRow(c.ctx, "SELECT EXISTS(SELECT 1 FROM plans WHERE id=$1)", v.Id).Scan(&existing); e != nil {
			return e
		}
		if existing {
			if e = postgres.SnapshotLegacyPlanSelections(c.ctx, c.tx, v.Id, c.now); e != nil {
				return e
			}
		}
		if !v.DefaultFree {
			var currentDefault bool
			if e := c.tx.QueryRow(c.ctx, "SELECT EXISTS(SELECT 1 FROM plans WHERE id=$1 AND is_default)", v.Id).Scan(&currentDefault); e != nil {
				return e
			}
			if currentDefault {
				return precondition("select a replacement default free plan before clearing this default")
			}
		}
		if v.DefaultFree {
			if _, e := c.tx.Exec(c.ctx, "UPDATE plans SET is_default=false WHERE is_default"); e != nil {
				return e
			}
		}
		_, e = c.tx.Exec(c.ctx, `INSERT INTO plans VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name,rank=EXCLUDED.rank,free=EXCLUDED.free,is_default=EXCLUDED.is_default,postpaid=EXCLUDED.postpaid`, v.Id, v.Name, v.Rank, v.Free, v.DefaultFree, v.Postpaid)
		if e != nil {
			return e
		}
		if e = c.replaceProductFeatures(v.Id, v.FeatureValues, q.ReplaceFeatureValues, ""); e != nil {
			return e
		}
		v.ActiveVersionId, e = postgres.RebuildProductVersion(c.ctx, c.tx, "plan", v.Id)
		if e != nil {
			return e
		}
		// Report the selected version's values, including preserved assignments.
		if v.FeatureValues, e = c.catalogFeatureValues(v.ActiveVersionId); e != nil {
			return e
		}
		o.Plan = v
		return c.audit("", "catalog.plan", q.Reason)
	})
}
func (s *Server) CreatePrice(ctx context.Context, r *connect.Request[pb.CreatePriceRequest]) (*connect.Response[pb.CreatePriceResponse], error) {
	o := &pb.CreatePriceResponse{}
	return run(s, ctx, r, o, func(c *call) error {
		q := r.Msg
		if e := c.operator(q.Reason); e != nil {
			return e
		}
		v := q.Price
		if v == nil || ((!canonical(v.PlanId) || v.AddOnId != "") && (!canonical(v.AddOnId) || v.PlanId != "")) || !canonical(v.ServiceId) || v.Currency != accounting.USD {
			return invalid("invalid price references or currency")
		}
		for _, n := range []string{v.UnitPrice, v.Included} {
			d, e := accounting.Parse(n)
			if e != nil || len(n) > 64 || d.Cmp(accounting.Zero()) < 0 {
				return invalid("price and allowance must be nonnegative decimals")
			}
		}
		if e := nonempty(v.Meter, 100); e != nil {
			return e
		}
		unit, _ := accounting.Parse(v.UnitPrice)
		included, _ := accounting.Parse(v.Included)
		v.UnitPrice, v.Included = unit.String(), included.String()
		var planID, addonID any
		if v.PlanId != "" {
			planID = v.PlanId
		}
		if v.AddOnId != "" {
			addonID = v.AddOnId
		}
		if v.Kind == pb.PriceKind_PRICE_KIND_UNSPECIFIED {
			v.Kind = pb.PriceKind_PRICE_KIND_USAGE
		}
		if v.Kind != pb.PriceKind_PRICE_KIND_USAGE && v.Kind != pb.PriceKind_PRICE_KIND_RECURRING {
			return invalid("unknown price kind")
		}
		if v.Kind == pb.PriceKind_PRICE_KIND_RECURRING && v.ExternalPriceId == "" {
			return invalid("recurring price requires Paddle mapping")
		}
		if v.Kind == pb.PriceKind_PRICE_KIND_USAGE {
			var exists bool
			if e := c.tx.QueryRow(c.ctx, "SELECT EXISTS(SELECT 1 FROM meters WHERE service_id=$1 AND name=$2)", v.ServiceId, v.Meter).Scan(&exists); e != nil {
				return e
			}
			if !exists {
				return invalid("unregistered meter")
			}
		}
		if v.AddOnId != "" {
			var addonService string
			if e := c.tx.QueryRow(c.ctx, "SELECT service_id::text FROM addons WHERE id=$1", v.AddOnId).Scan(&addonService); e != nil {
				if errors.Is(e, pgx.ErrNoRows) {
					return invalid("unknown add-on")
				}
				return e
			}
			if addonService != v.ServiceId {
				c.s.Log.WarnContext(c.ctx, "rejected add-on price for another service", "correlation_id", c.correlation, "add_on_id", v.AddOnId, "service_id", v.ServiceId, "add_on_service_id", addonService)
				return invalid("add-on belongs to another service")
			}
		}
		ownerKind, ownerID := "plan", v.PlanId
		if v.AddOnId != "" {
			ownerKind, ownerID = "addon", v.AddOnId
		}
		if v.Key == "" {
			sum := sha256.Sum256([]byte(fmt.Sprintf("%s:%s:%d", v.ServiceId, v.Meter, v.Kind)))
			v.Key = "price-" + hex.EncodeToString(sum[:8])
		}
		if !catalogKeyPattern.MatchString(v.Key) {
			return invalid("invalid price key")
		}
		if v.StableId != "" && !canonical(v.StableId) {
			return invalid("invalid stable price ID")
		}
		var e error
		v.StableId, e = c.catalogEntity("price", ownerKind+":"+ownerID, v.Key, v.StableId)
		if e != nil {
			return e
		}
		definition := struct {
			Service       string `json:"service"`
			Meter         string `json:"meter"`
			UnitPrice     string `json:"unit_price"`
			Included      string `json:"included"`
			Currency      string `json:"currency"`
			ExternalPrice string `json:"external_price"`
			Kind          int32  `json:"kind"`
		}{v.ServiceId, v.Meter, v.UnitPrice, v.Included, v.Currency, v.ExternalPriceId, int32(v.Kind)}
		var created bool
		v.Id, created, e = postgres.SelectCatalogVersion(c.ctx, c.tx, v.StableId, definition)
		if e != nil {
			return e
		}
		// SelectCatalogVersion made this version the active one for its key.
		v.Active = true
		if created {
			_, e = c.tx.Exec(c.ctx, `INSERT INTO prices(id,plan_id,service_id,meter,unit_price,included,currency,external_price_id,created_at,addon_id,kind) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)`, v.Id, planID, v.ServiceId, v.Meter, v.UnitPrice, v.Included, v.Currency, v.ExternalPriceId, c.now, addonID, int32(v.Kind))
			if e != nil {
				return e
			}
		}
		if _, e = postgres.RebuildProductVersion(c.ctx, c.tx, ownerKind, ownerID); e != nil {
			return e
		}
		o.Price = v
		return c.audit("", "catalog.price", q.Reason)
	})
}
func (s *Server) GetCatalog(ctx context.Context, r *connect.Request[pb.GetCatalogRequest]) (*connect.Response[pb.GetCatalogResponse], error) {
	o := &pb.GetCatalogResponse{}
	return run(s, ctx, r, o, func(c *call) error {
		n, after, e := page(r.Msg.Page)
		if e != nil {
			return e
		}
		rows, e := c.tx.Query(c.ctx, "SELECT s.id::text,s.name,s.client_id,s.capabilities,s.scopes,COALESCE(ce.key,'') FROM services s LEFT JOIN catalog_entities ce ON ce.id=s.id WHERE s.id>$1 ORDER BY s.id LIMIT $2", after, n+1)
		if e != nil {
			return e
		}
		for rows.Next() {
			v := &pb.Service{}
			if e = rows.Scan(&v.Id, &v.Name, &v.ClientId, &v.Capabilities, &v.Scopes, &v.Key); e != nil {
				rows.Close()
				return e
			}
			if !c.identity.Operator {
				v.ClientId = ""
				v.Scopes = nil
			}
			o.Services = append(o.Services, v)
		}
		rows.Close()
		if e = rows.Err(); e != nil {
			return e
		}
		rows, e = c.tx.Query(c.ctx, "SELECT p.id::text,p.name,p.rank,p.free,p.is_default,p.postpaid,COALESCE(ce.key,''),COALESCE(ce.active_version_id::text,'') FROM plans p LEFT JOIN catalog_entities ce ON ce.id=p.id WHERE p.id>$1 ORDER BY p.id LIMIT $2", after, n+1)
		if e != nil {
			return e
		}
		for rows.Next() {
			v := &pb.Plan{}
			if e = rows.Scan(&v.Id, &v.Name, &v.Rank, &v.Free, &v.DefaultFree, &v.Postpaid, &v.Key, &v.ActiveVersionId); e != nil {
				rows.Close()
				return e
			}
			o.Plans = append(o.Plans, v)
		}
		rows.Close()
		if e = rows.Err(); e != nil {
			return e
		}
		rows, e = c.tx.Query(c.ctx, `SELECT p.id::text,COALESCE(p.plan_id::text,''),p.service_id::text,p.meter,p.unit_price::text,p.included::text,p.currency,p.external_price_id,COALESCE(p.addon_id::text,''),p.kind,COALESCE(ce.key,''),COALESCE(ce.id::text,''),
CASE WHEN ce.id IS NOT NULL THEN COALESCE(ce.active_version_id=p.id,false)
 ELSE NOT EXISTS (SELECT 1 FROM prices newer WHERE newer.plan_id IS NOT DISTINCT FROM p.plan_id AND newer.addon_id IS NOT DISTINCT FROM p.addon_id AND newer.service_id=p.service_id AND newer.meter=p.meter AND newer.kind=p.kind AND (newer.created_at,newer.id)>(p.created_at,p.id)) END
FROM prices p LEFT JOIN catalog_versions cv ON cv.id=p.id LEFT JOIN catalog_entities ce ON ce.id=cv.entity_id
WHERE p.id>$1 ORDER BY p.id LIMIT $2`, after, n+1)
		if e != nil {
			return e
		}
		defer rows.Close()
		for rows.Next() {
			v := &pb.Price{}
			if e = rows.Scan(&v.Id, &v.PlanId, &v.ServiceId, &v.Meter, &v.UnitPrice, &v.Included, &v.Currency, &v.ExternalPriceId, &v.AddOnId, &v.Kind, &v.Key, &v.StableId, &v.Active); e != nil {
				return e
			}
			o.Prices = append(o.Prices, v)
		}
		rows.Close()
		if e = rows.Err(); e != nil {
			return e
		}
		rows, e = c.tx.Query(c.ctx, "SELECT a.id::text,a.service_id::text,a.name,COALESCE(ce.key,''),COALESCE(ce.active_version_id::text,'') FROM addons a LEFT JOIN catalog_entities ce ON ce.id=a.id WHERE a.id>$1 ORDER BY a.id LIMIT $2", after, n+1)
		if e != nil {
			return e
		}
		for rows.Next() {
			v := &pb.AddOn{}
			if e = rows.Scan(&v.Id, &v.ServiceId, &v.Name, &v.Key, &v.ActiveVersionId); e != nil {
				rows.Close()
				return e
			}
			o.AddOns = append(o.AddOns, v)
		}
		rows.Close()
		if e = rows.Err(); e != nil {
			return e
		}
		rows, e = c.tx.Query(c.ctx, "SELECT id::text,service_id::text,key,name,kind,value_type,meter_key FROM features WHERE id>$1 ORDER BY id LIMIT $2", after, n+1)
		if e != nil {
			return e
		}
		for rows.Next() {
			v := &pb.Feature{}
			if e = rows.Scan(&v.Id, &v.ServiceId, &v.Key, &v.Name, &v.Kind, &v.ValueType, &v.MeterKey); e != nil {
				rows.Close()
				return e
			}
			o.Features = append(o.Features, v)
		}
		rows.Close()
		if e = rows.Err(); e != nil {
			return e
		}
		// One cursor advances all catalog collections only to their minimum full boundary.
		boundary := ""
		if len(o.Services) > n {
			boundary = o.Services[n-1].Id
		}
		if len(o.Plans) > n && (boundary == "" || o.Plans[n-1].Id < boundary) {
			boundary = o.Plans[n-1].Id
		}
		if len(o.Prices) > n && (boundary == "" || o.Prices[n-1].Id < boundary) {
			boundary = o.Prices[n-1].Id
		}
		if len(o.AddOns) > n && (boundary == "" || o.AddOns[n-1].Id < boundary) {
			boundary = o.AddOns[n-1].Id
		}
		if len(o.Features) > n && (boundary == "" || o.Features[n-1].Id < boundary) {
			boundary = o.Features[n-1].Id
		}
		if boundary != "" {
			for len(o.Features) > 0 && o.Features[len(o.Features)-1].Id > boundary {
				o.Features = o.Features[:len(o.Features)-1]
			}
			for len(o.AddOns) > 0 && o.AddOns[len(o.AddOns)-1].Id > boundary {
				o.AddOns = o.AddOns[:len(o.AddOns)-1]
			}
			for len(o.Services) > 0 && o.Services[len(o.Services)-1].Id > boundary {
				o.Services = o.Services[:len(o.Services)-1]
			}
			for len(o.Plans) > 0 && o.Plans[len(o.Plans)-1].Id > boundary {
				o.Plans = o.Plans[:len(o.Plans)-1]
			}
			for len(o.Prices) > 0 && o.Prices[len(o.Prices)-1].Id > boundary {
				o.Prices = o.Prices[:len(o.Prices)-1]
			}
			o.NextAfterId = boundary
		}
		// Meters travel with their service; service registration bounds each set.
		for _, service := range o.Services {
			rows, e = c.tx.Query(c.ctx, `SELECT m.service_id::text,m.name,COALESCE(ce.id::text,''),COALESCE(ce.key,''),COALESCE(cv.definition,'{}'::jsonb) FROM meters m
 LEFT JOIN catalog_entities ce ON ce.kind='meter' AND ce.scope=m.service_id::text
 AND EXISTS (SELECT 1 FROM catalog_versions cv WHERE cv.id=ce.active_version_id AND cv.definition->>'name'=m.name)
 LEFT JOIN catalog_versions cv ON cv.id=ce.active_version_id
 WHERE m.service_id=$1 ORDER BY m.name`, service.Id)
			if e != nil {
				return e
			}
			for rows.Next() {
				v := &pb.Meter{}
				var definition []byte
				if e = rows.Scan(&v.ServiceId, &v.Name, &v.Id, &v.Key, &definition); e != nil {
					rows.Close()
					return e
				}
				if e = json.Unmarshal(definition, v); e != nil {
					rows.Close()
					return e
				}
				o.Meters = append(o.Meters, v)
			}
			rows.Close()
			if e = rows.Err(); e != nil {
				return e
			}
		}
		for _, plan := range o.Plans {
			plan.FeatureValues, e = c.catalogFeatureValues(plan.ActiveVersionId)
			if e != nil {
				return e
			}
		}
		for _, addon := range o.AddOns {
			addon.FeatureValues, e = c.catalogFeatureValues(addon.ActiveVersionId)
			if e != nil {
				return e
			}
		}
		return nil
	})
}
func (s *Server) SetGroup(ctx context.Context, r *connect.Request[pb.SetGroupRequest]) (*connect.Response[pb.SetGroupResponse], error) {
	o := &pb.SetGroupResponse{}
	return run(s, ctx, r, o, func(c *call) error {
		q := r.Msg
		if e := c.require(q.TenantId, pb.Role_ROLE_ADMIN); e != nil {
			return e
		}
		t, e := c.tenant(q.TenantId)
		if e != nil {
			return e
		}
		if t.Personal || t.Deleting {
			return precondition("groups require an active organization")
		}
		g := q.Group
		if g == nil {
			return invalid("group required")
		}
		if e = nonempty(g.Name, 200); e != nil {
			return e
		}
		if len(g.UserIds) > 1000 || len(g.Permissions) > 1000 {
			return invalid("group is too large")
		}
		if g.Id == "" {
			g.Id = ID()
			_, e = c.tx.Exec(c.ctx, "INSERT INTO groups VALUES($1,$2,$3)", g.Id, q.TenantId, g.Name)
		} else {
			if !canonical(g.Id) {
				return invalid("invalid group ID")
			}
			tag, err := c.tx.Exec(c.ctx, "UPDATE groups SET name=$3 WHERE id=$1 AND tenant_id=$2", g.Id, q.TenantId, g.Name)
			e = err
			if e == nil && tag.RowsAffected() != 1 {
				return forbidden()
			}
		}
		if e != nil {
			return e
		}
		if _, e = c.tx.Exec(c.ctx, "DELETE FROM group_members WHERE group_id=$1", g.Id); e != nil {
			return e
		}
		if _, e = c.tx.Exec(c.ctx, "DELETE FROM group_permissions WHERE group_id=$1", g.Id); e != nil {
			return e
		}
		for _, user := range g.UserIds {
			if !canonical(user) {
				return invalid("invalid member ID")
			}
			if _, e = c.tx.Exec(c.ctx, "INSERT INTO group_members VALUES($1,$2,$3)", q.TenantId, g.Id, user); e != nil {
				return invalid("group contains a non-member or duplicate")
			}
		}
		for _, p := range g.Permissions {
			if p == nil || !canonical(p.ServiceId) {
				return invalid("invalid permission")
			}
			var ok bool
			if e = c.tx.QueryRow(c.ctx, "SELECT $2=ANY(capabilities) FROM services WHERE id=$1", p.ServiceId, p.Capability).Scan(&ok); e != nil || !ok {
				return invalid("unregistered capability")
			}
			if _, e = c.tx.Exec(c.ctx, "INSERT INTO group_permissions VALUES($1,$2,$3)", g.Id, p.ServiceId, p.Capability); e != nil {
				return invalid("duplicate permission")
			}
		}
		o.Group = g
		return c.audit(q.TenantId, "group.set", "")
	})
}
func (s *Server) ListGroups(ctx context.Context, r *connect.Request[pb.ListGroupsRequest]) (*connect.Response[pb.ListGroupsResponse], error) {
	o := &pb.ListGroupsResponse{}
	return run(s, ctx, r, o, func(c *call) error {
		if e := c.member(r.Msg.TenantId); e != nil {
			return e
		}
		n, after, e := page(r.Msg.Page)
		if e != nil {
			return e
		}
		rows, e := c.tx.Query(c.ctx, "SELECT id::text,name FROM groups WHERE tenant_id=$1 AND id>$2 ORDER BY id LIMIT $3", r.Msg.TenantId, after, n+1)
		if e != nil {
			return e
		}
		for rows.Next() {
			g := &pb.Group{}
			if e = rows.Scan(&g.Id, &g.Name); e != nil {
				rows.Close()
				return e
			}
			o.Groups = append(o.Groups, g)
		}
		rows.Close()
		if e = rows.Err(); e != nil {
			return e
		}
		if len(o.Groups) > n {
			o.Groups = o.Groups[:n]
			o.NextAfterId = o.Groups[n-1].Id
		}
		for _, g := range o.Groups {
			rows, e = c.tx.Query(c.ctx, "SELECT user_id::text FROM group_members WHERE group_id=$1 ORDER BY user_id", g.Id)
			if e != nil {
				return e
			}
			for rows.Next() {
				var u string
				if e = rows.Scan(&u); e != nil {
					rows.Close()
					return e
				}
				g.UserIds = append(g.UserIds, u)
			}
			rows.Close()
			if e = rows.Err(); e != nil {
				return e
			}
			rows, e = c.tx.Query(c.ctx, "SELECT service_id::text,capability FROM group_permissions WHERE group_id=$1 ORDER BY service_id,capability", g.Id)
			if e != nil {
				return e
			}
			for rows.Next() {
				p := &pb.Permission{}
				if e = rows.Scan(&p.ServiceId, &p.Capability); e != nil {
					rows.Close()
					return e
				}
				g.Permissions = append(g.Permissions, p)
			}
			rows.Close()
			if e = rows.Err(); e != nil {
				return e
			}
		}
		return nil
	})
}
