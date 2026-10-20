// SPDX-License-Identifier: FSL-1.1-ALv2

package rpc

import (
	"connectrpc.com/connect"
	"context"
	"encoding/json"
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/delcore-api/internal/metering"
	"github.com/delinoio/apps/servers/delcore-api/internal/postgres"
	"sort"
)

func validateCatalogMeter(m *pb.Meter) error {
	aggregation := metering.Count
	switch m.Aggregation {
	case pb.MeterAggregation_METER_AGGREGATION_COUNT:
		aggregation = metering.Count
	case pb.MeterAggregation_METER_AGGREGATION_SUM:
		aggregation = metering.Sum
	case pb.MeterAggregation_METER_AGGREGATION_MIN:
		aggregation = metering.Min
	case pb.MeterAggregation_METER_AGGREGATION_MAX:
		aggregation = metering.Max
	case pb.MeterAggregation_METER_AGGREGATION_AVG:
		aggregation = metering.Avg
	case pb.MeterAggregation_METER_AGGREGATION_UNIQUE_COUNT:
		aggregation = metering.UniqueCount
	default:
		return invalid("unknown meter aggregation")
	}
	definition := metering.Definition{Key: m.Name, EventType: m.EventType, Aggregation: aggregation, ValueAttribute: m.ValueAttribute, GroupBy: m.GroupBy}
	for _, filter := range m.Filters {
		if filter == nil {
			return invalid("invalid meter filter")
		}
		item := metering.Filter{Attribute: filter.Attribute}
		switch filter.Operator {
		case pb.MeterFilterOperator_METER_FILTER_OPERATOR_EQUAL:
			if len(filter.Values) != 1 {
				return invalid("equal meter filters require one value")
			}
			item.Operator, item.Value = metering.Equal, metering.String(filter.Values[0])
		case pb.MeterFilterOperator_METER_FILTER_OPERATOR_NOT_EQUAL:
			if len(filter.Values) != 1 {
				return invalid("not-equal meter filters require one value")
			}
			item.Operator, item.Value = metering.NotEqual, metering.String(filter.Values[0])
		case pb.MeterFilterOperator_METER_FILTER_OPERATOR_IN:
			if len(filter.Values) == 0 {
				return invalid("in meter filters require values")
			}
			item.Operator = metering.In
			item.Values = make([]metering.Value, 0, len(filter.Values))
			for _, value := range filter.Values {
				item.Values = append(item.Values, metering.String(value))
			}
		case pb.MeterFilterOperator_METER_FILTER_OPERATOR_GREATER,
			pb.MeterFilterOperator_METER_FILTER_OPERATOR_GREATER_OR_EQUAL,
			pb.MeterFilterOperator_METER_FILTER_OPERATOR_LESS,
			pb.MeterFilterOperator_METER_FILTER_OPERATOR_LESS_OR_EQUAL:
			if len(filter.Values) != 1 {
				return invalid("numeric meter filters require one value")
			}
			value, err := metering.Number(filter.Values[0])
			if err != nil {
				return invalid("numeric meter filter requires a decimal value")
			}
			item.Value = value
			switch filter.Operator {
			case pb.MeterFilterOperator_METER_FILTER_OPERATOR_GREATER:
				item.Operator = metering.GreaterThan
			case pb.MeterFilterOperator_METER_FILTER_OPERATOR_GREATER_OR_EQUAL:
				item.Operator = metering.GreaterThanOrEqual
			case pb.MeterFilterOperator_METER_FILTER_OPERATOR_LESS:
				item.Operator = metering.LessThan
			default:
				item.Operator = metering.LessThanOrEqual
			}
		case pb.MeterFilterOperator_METER_FILTER_OPERATOR_EXISTS:
			if len(filter.Values) != 1 || (filter.Values[0] != "true" && filter.Values[0] != "false") {
				return invalid("exists meter filters require one boolean value")
			}
			item.Operator = metering.Exists
			item.Value = metering.Boolean(filter.Values[0] == "true")
		default:
			return invalid("unknown meter filter operator")
		}
		definition.Filters = append(definition.Filters, item)
	}
	if err := definition.Validate(); err != nil {
		return invalid(err.Error())
	}
	return nil
}

func (s *Server) PutMeter(ctx context.Context, r *connect.Request[pb.PutMeterRequest]) (*connect.Response[pb.PutMeterResponse], error) {
	o := &pb.PutMeterResponse{}
	return run(s, ctx, r, o, func(c *call) error {
		q := r.Msg
		if e := c.operator(q.Reason); e != nil {
			return e
		}
		m := q.Meter
		if m == nil || !canonical(m.ServiceId) {
			return invalid("invalid meter service")
		}
		if e := nonempty(m.Name, 100); e != nil {
			return e
		}
		if m.Id != "" && !canonical(m.Id) {
			return invalid("invalid meter ID")
		}
		legacy := m.EventType == "" && m.Aggregation == pb.MeterAggregation_METER_AGGREGATION_UNSPECIFIED && m.ValueAttribute == "" && len(m.Filters) == 0 && len(m.GroupBy) == 0
		var e error
		m.Key, e = c.catalogKey(m.Id, m.Key, m.Name)
		if e != nil {
			return e
		}
		if m.Name != m.Key {
			return invalid("meter name must match its stable key")
		}
		m.Id, e = c.catalogEntity("meter", m.ServiceId, m.Key, m.Id)
		if e != nil {
			return e
		}
		if m.Aggregation == pb.MeterAggregation_METER_AGGREGATION_UNSPECIFIED {
			m.Aggregation = pb.MeterAggregation_METER_AGGREGATION_SUM
		}
		if m.Aggregation < pb.MeterAggregation_METER_AGGREGATION_COUNT || m.Aggregation > pb.MeterAggregation_METER_AGGREGATION_UNIQUE_COUNT {
			return invalid("unknown meter aggregation")
		}
		if len(m.EventType) > 100 || len(m.ValueAttribute) > 100 || len(m.Filters) > 100 || len(m.GroupBy) > 20 {
			return invalid("meter definition exceeds catalog limits")
		}
		if !legacy {
			if e = validateCatalogMeter(m); e != nil {
				return e
			}
		}
		filters := make([]*pb.MeterFilter, 0, len(m.Filters))
		for _, filter := range m.Filters {
			if filter == nil || filter.Attribute == "" || filter.Operator < pb.MeterFilterOperator_METER_FILTER_OPERATOR_EQUAL || filter.Operator > pb.MeterFilterOperator_METER_FILTER_OPERATOR_EXISTS {
				return invalid("invalid meter filter")
			}
			item := &pb.MeterFilter{Attribute: filter.Attribute, Operator: filter.Operator, Values: append([]string(nil), filter.Values...)}
			if item.Operator == pb.MeterFilterOperator_METER_FILTER_OPERATOR_IN {
				sort.Strings(item.Values)
			}
			filters = append(filters, item)
		}
		sort.Slice(filters, func(i, j int) bool {
			a, _ := json.Marshal(filters[i])
			b, _ := json.Marshal(filters[j])
			return string(a) < string(b)
		})
		groups := append([]string(nil), m.GroupBy...)
		sort.Strings(groups)
		for i, group := range groups {
			if e = nonempty(group, 100); e != nil {
				return e
			}
			if i > 0 && group == groups[i-1] {
				return invalid("repeated meter group attribute")
			}
		}
		definition := struct {
			Name           string              `json:"name"`
			EventType      string              `json:"event_type"`
			Aggregation    pb.MeterAggregation `json:"aggregation"`
			ValueAttribute string              `json:"value_attribute"`
			Filters        []*pb.MeterFilter   `json:"filters"`
			GroupBy        []string            `json:"group_by"`
		}{m.Name, m.EventType, m.Aggregation, m.ValueAttribute, filters, groups}
		if !legacy {
			previous, e := postgres.ActiveCatalogVersion(c.ctx, c.tx, "meter", m.Id)
			if e != nil {
				return e
			}
			version, _, e := postgres.SelectCatalogVersion(c.ctx, c.tx, m.Id, definition)
			if e != nil {
				return e
			}
			if previous != "" && previous != version {
				return invalid("meter definition is immutable for its key")
			}
		}
		var count int
		if e := c.tx.QueryRow(c.ctx, "SELECT count(*) FROM meters WHERE service_id=$1 AND name<>$2", m.ServiceId, m.Name).Scan(&count); e != nil {
			return e
		}
		if count >= 100 {
			return invalid("service meter limit reached")
		}
		_, e = c.tx.Exec(c.ctx, "INSERT INTO meters VALUES($1,$2) ON CONFLICT DO NOTHING", m.ServiceId, m.Name)
		if e != nil {
			return e
		}
		o.Meter = m
		return c.audit("", "catalog.meter", q.Reason)
	})
}
func (s *Server) PutFeature(ctx context.Context, r *connect.Request[pb.PutFeatureRequest]) (*connect.Response[pb.PutFeatureResponse], error) {
	o := &pb.PutFeatureResponse{}
	return run(s, ctx, r, o, func(c *call) error {
		q := r.Msg
		if e := c.operator(q.Reason); e != nil {
			return e
		}
		v := q.Feature
		if v == nil || !canonical(v.ServiceId) || v.Id != "" && !canonical(v.Id) {
			return invalid("invalid feature identity")
		}
		if e := nonempty(v.Name, 200); e != nil {
			return e
		}
		if !catalogKeyPattern.MatchString(v.Key) {
			return invalid("invalid feature key")
		}
		switch v.Kind {
		case pb.FeatureKind_FEATURE_KIND_ACCESS:
			if v.ValueType != pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_UNSPECIFIED || v.MeterKey != "" {
				return invalid("access feature has no meter or configuration type")
			}
		case pb.FeatureKind_FEATURE_KIND_METERED:
			if v.ValueType != pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_UNSPECIFIED || !catalogKeyPattern.MatchString(v.MeterKey) {
				return invalid("metered feature requires a meter key")
			}
			var exists bool
			if e := c.tx.QueryRow(c.ctx, "SELECT EXISTS(SELECT 1 FROM catalog_entities WHERE kind='meter' AND scope=$1 AND key=$2)", v.ServiceId, v.MeterKey).Scan(&exists); e != nil {
				return e
			}
			if !exists {
				return invalid("unknown feature meter")
			}
		case pb.FeatureKind_FEATURE_KIND_CONFIGURATION:
			if v.ValueType < pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_BOOLEAN || v.ValueType > pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_STRING_LIST || v.MeterKey != "" {
				return invalid("configuration feature requires a value type")
			}
		default:
			return invalid("unknown feature kind")
		}
		var e error
		v.Id, e = c.catalogEntity("feature", v.ServiceId, v.Key, v.Id)
		if e != nil {
			return e
		}
		definition := struct {
			Kind      int32  `json:"kind"`
			ValueType int32  `json:"value_type"`
			MeterKey  string `json:"meter_key"`
		}{int32(v.Kind), int32(v.ValueType), v.MeterKey}
		previous, e := postgres.ActiveCatalogVersion(c.ctx, c.tx, "feature", v.Id)
		if e != nil {
			return e
		}
		version, _, e := postgres.SelectCatalogVersion(c.ctx, c.tx, v.Id, definition)
		if e != nil {
			return e
		}
		if previous != "" && previous != version {
			return invalid("feature type and meter are immutable for its key")
		}
		tag, e := c.tx.Exec(c.ctx, `INSERT INTO features(id,service_id,key,name,kind,value_type,meter_key) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name WHERE features.service_id=EXCLUDED.service_id AND features.key=EXCLUDED.key AND features.kind=EXCLUDED.kind AND features.value_type=EXCLUDED.value_type AND features.meter_key=EXCLUDED.meter_key`, v.Id, v.ServiceId, v.Key, v.Name, int32(v.Kind), int32(v.ValueType), v.MeterKey)
		if e != nil {
			return e
		}
		if tag.RowsAffected() != 1 {
			return invalid("feature definition is immutable for its key")
		}
		o.Feature = v
		return c.audit("", "catalog.feature", q.Reason)
	})
}
func (s *Server) PutAddOn(ctx context.Context, r *connect.Request[pb.PutAddOnRequest]) (*connect.Response[pb.PutAddOnResponse], error) {
	o := &pb.PutAddOnResponse{}
	return run(s, ctx, r, o, func(c *call) error {
		q := r.Msg
		if e := c.operator(q.Reason); e != nil {
			return e
		}
		a := q.AddOn
		if a == nil || !canonical(a.ServiceId) {
			return invalid("invalid add-on service")
		}
		if e := nonempty(a.Name, 200); e != nil {
			return e
		}
		if a.Id != "" && !canonical(a.Id) {
			return invalid("invalid add-on ID")
		}
		var e error
		a.Key, e = c.catalogKey(a.Id, a.Key, a.Name)
		if e != nil {
			return e
		}
		a.Id, e = c.catalogEntity("addon", a.ServiceId, a.Key, a.Id)
		if e != nil {
			return e
		}
		tag, e := c.tx.Exec(c.ctx, "INSERT INTO addons VALUES($1,$2,$3) ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name WHERE addons.service_id=EXCLUDED.service_id", a.Id, a.ServiceId, a.Name)
		if e != nil {
			return e
		}
		if tag.RowsAffected() != 1 {
			return invalid("add-on service cannot change")
		}
		if e = c.replaceProductFeatures(a.Id, a.FeatureValues, q.ReplaceFeatureValues, a.ServiceId); e != nil {
			return e
		}
		a.ActiveVersionId, e = postgres.RebuildProductVersion(c.ctx, c.tx, "addon", a.Id)
		if e != nil {
			return e
		}
		o.AddOn = a
		return c.audit("", "catalog.addon", q.Reason)
	})
}
func (s *Server) ListBillingHistory(ctx context.Context, r *connect.Request[pb.ListBillingHistoryRequest]) (*connect.Response[pb.ListBillingHistoryResponse], error) {
	o := &pb.ListBillingHistoryResponse{}
	return run(s, ctx, r, o, func(c *call) error {
		if e := c.require(r.Msg.TenantId, pb.Role_ROLE_BILLING); e != nil {
			return e
		}
		n, after, e := page(r.Msg.Page)
		if e != nil {
			return e
		}
		rows, e := c.tx.Query(c.ctx, "SELECT id::text,kind,state,error_category,attempts,checkout_url FROM jobs WHERE tenant_id=$1 AND kind<>'email' AND id>$2 ORDER BY id LIMIT $3", r.Msg.TenantId, after, n+1)
		if e != nil {
			return e
		}
		defer rows.Close()
		states := map[string]pb.JobState{"pending": pb.JobState_JOB_STATE_PENDING, "running": pb.JobState_JOB_STATE_RUNNING, "reconcile": pb.JobState_JOB_STATE_RECONCILE, "done": pb.JobState_JOB_STATE_DONE, "failed": pb.JobState_JOB_STATE_FAILED}
		for rows.Next() {
			j := &pb.Job{}
			var state string
			if e = rows.Scan(&j.Id, &j.Kind, &state, &j.ErrorCategory, &j.Attempts, &j.CheckoutUrl); e != nil {
				return e
			}
			j.State = states[state]
			o.Jobs = append(o.Jobs, j)
		}
		if len(o.Jobs) > n {
			o.Jobs = o.Jobs[:n]
			o.NextAfterId = o.Jobs[n-1].Id
		}
		return rows.Err()
	})
}
func (s *Server) DeleteGroup(ctx context.Context, r *connect.Request[pb.DeleteGroupRequest]) (*connect.Response[pb.DeleteGroupResponse], error) {
	o := &pb.DeleteGroupResponse{}
	return run(s, ctx, r, o, func(c *call) error {
		q := r.Msg
		if e := c.require(q.TenantId, pb.Role_ROLE_ADMIN); e != nil {
			return e
		}
		if !canonical(q.GroupId) {
			return invalid("invalid group ID")
		}
		var exists bool
		if e := c.tx.QueryRow(c.ctx, "SELECT EXISTS(SELECT 1 FROM groups WHERE id=$1 AND tenant_id=$2)", q.GroupId, q.TenantId).Scan(&exists); e != nil {
			return e
		}
		if !exists {
			return forbidden()
		}
		if _, e := c.tx.Exec(c.ctx, "DELETE FROM group_members WHERE group_id=$1", q.GroupId); e != nil {
			return e
		}
		if _, e := c.tx.Exec(c.ctx, "DELETE FROM group_permissions WHERE group_id=$1", q.GroupId); e != nil {
			return e
		}
		if _, e := c.tx.Exec(c.ctx, "DELETE FROM groups WHERE id=$1", q.GroupId); e != nil {
			return e
		}
		return c.audit(q.TenantId, "group.delete", "")
	})
}
