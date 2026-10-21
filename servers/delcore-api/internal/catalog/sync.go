// SPDX-License-Identifier: FSL-1.1-ALv2

package catalog

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"sort"
	"time"

	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/delcore-api/internal/accounting"
	"github.com/delinoio/apps/servers/delcore-api/internal/events"
	"github.com/delinoio/apps/servers/delcore-api/internal/postgres"
	"github.com/jackc/pgx/v5"
	"go.yaml.in/yaml/v4"
)

var aggregations = map[string]pb.MeterAggregation{
	"count":        pb.MeterAggregation_METER_AGGREGATION_COUNT,
	"sum":          pb.MeterAggregation_METER_AGGREGATION_SUM,
	"min":          pb.MeterAggregation_METER_AGGREGATION_MIN,
	"max":          pb.MeterAggregation_METER_AGGREGATION_MAX,
	"avg":          pb.MeterAggregation_METER_AGGREGATION_AVG,
	"unique_count": pb.MeterAggregation_METER_AGGREGATION_UNIQUE_COUNT,
}
var operators = map[string]pb.MeterFilterOperator{
	"equal":                 pb.MeterFilterOperator_METER_FILTER_OPERATOR_EQUAL,
	"not_equal":             pb.MeterFilterOperator_METER_FILTER_OPERATOR_NOT_EQUAL,
	"in":                    pb.MeterFilterOperator_METER_FILTER_OPERATOR_IN,
	"greater_than":          pb.MeterFilterOperator_METER_FILTER_OPERATOR_GREATER,
	"greater_than_or_equal": pb.MeterFilterOperator_METER_FILTER_OPERATOR_GREATER_OR_EQUAL,
	"less_than":             pb.MeterFilterOperator_METER_FILTER_OPERATOR_LESS,
	"less_than_or_equal":    pb.MeterFilterOperator_METER_FILTER_OPERATOR_LESS_OR_EQUAL,
	"exists":                pb.MeterFilterOperator_METER_FILTER_OPERATOR_EXISTS,
}
var valueTypes = map[string]int{
	"boolean": 1, "integer": 2, "decimal": 3, "string": 4, "string_list": 5,
}
var featureKinds = map[string]int{"access": 1, "metered": 2, "configuration": 3}

// Sync applies a complete development file set under the already-acquired
// process lock. The caller must check schema compatibility before calling it.
func Sync(ctx context.Context, db *postgres.Store, dir string, logger *slog.Logger) error {
	items, err := load(dir)
	if err != nil {
		return err
	}
	counts := map[string]int{}
	err = db.Transaction(ctx, func(tx pgx.Tx) error {
		if err := snapshotLegacyProducts(ctx, tx, items); err != nil {
			return err
		}
		if err := validateDatabase(ctx, tx, items); err != nil {
			return err
		}
		for _, kind := range []string{"service", "meter", "feature", "plan", "addon", "price"} {
			for _, item := range items {
				if item.Kind != kind {
					continue
				}
				if err := apply(ctx, tx, item); err != nil {
					return fmt.Errorf("catalog file %s: %w", item.Path, err)
				}
				counts[kind]++
			}
		}
		// Product versions are selected only after every referenced price and
		// feature grant has reached its final state in this transaction. A price
		// file may name an owner omitted from this set, so it still rebuilds that
		// owner's active product version.
		products := map[string]string{}
		for _, item := range items {
			if item.Kind == "plan" || item.Kind == "addon" {
				scope := ""
				if item.Kind == "addon" {
					var err error
					scope, err = resolve(ctx, tx, "service", "", item.Service)
					if err != nil {
						return err
					}
				}
				id, err := resolve(ctx, tx, item.Kind, scope, item.Key)
				if err != nil {
					return err
				}
				if err = replaceGrants(ctx, tx, id, item.Features, scope); err != nil {
					return fmt.Errorf("catalog file %s: %w", item.Path, err)
				}
				products[item.Kind+":"+id] = id
			}
			if item.Kind == "price" {
				kind := "plan"
				scope, key := "", item.Plan
				if item.AddOn != nil {
					var err error
					scope, err = resolve(ctx, tx, "service", "", item.AddOn.Service)
					if err != nil {
						return err
					}
					kind, key = "addon", item.AddOn.Key
				}
				id, err := resolve(ctx, tx, kind, scope, key)
				if err != nil {
					return err
				}
				products[kind+":"+id] = id
			}
		}
		keys := make([]string, 0, len(products))
		for key := range products {
			keys = append(keys, key)
		}
		sort.Strings(keys)
		for _, key := range keys {
			kind := "plan"
			if key[:5] == "addon" {
				kind = "addon"
			}
			if _, err := postgres.RebuildProductVersion(ctx, tx, kind, products[key]); err != nil {
				return err
			}
		}
		return nil
	})
	if err != nil {
		return err
	}
	logger.InfoContext(ctx, "catalog synchronized", "category", "catalog_reconciliation", "files", len(items), "services", counts["service"], "meters", counts["meter"], "features", counts["feature"], "plans", counts["plan"], "addons", counts["addon"], "prices", counts["price"])
	return nil
}

// snapshotLegacyProducts keeps tenants that predate catalog versioning on
// their current plan and add-on policy. Such tenants have no selection row and
// read the mutable plan row and product feature grants, which synchronization
// replaces even when only a declared feature changes. Each declared plan or
// add-on that already exists and still has such a tenant is therefore
// snapshotted before validation or application changes it, matching PutPlan
// and PutAddOn. The snapshot records the current definition, so reapplying
// unchanged files reselects the same version instead of creating another one.
func snapshotLegacyProducts(ctx context.Context, tx pgx.Tx, items []definition) error {
	now := time.Now().UTC()
	for _, item := range items {
		var id string
		var needed bool
		var err error
		switch item.Kind {
		case "plan":
			err = tx.QueryRow(ctx, `SELECT p.id::text, EXISTS(SELECT 1 FROM tenants t WHERE t.plan_id=p.id AND NOT EXISTS (
   SELECT 1 FROM tenant_catalog_selections cs WHERE cs.tenant_id=t.id AND cs.product_id=t.plan_id AND cs.ends_at IS NULL))
 FROM catalog_entities ce JOIN plans p ON p.id=ce.id
 WHERE ce.kind='plan' AND ce.scope='' AND ce.key=$1`, item.Key).Scan(&id, &needed)
		case "addon":
			// An entitlement ending after now is still held, matching
			// SnapshotLegacyAddOnSelections.
			err = tx.QueryRow(ctx, `SELECT a.id::text, EXISTS(SELECT 1 FROM entitlements en JOIN prices p ON p.id=en.price_id
   WHERE (en.ends_at IS NULL OR en.ends_at>$3) AND p.addon_id=a.id AND NOT EXISTS (
     SELECT 1 FROM tenant_catalog_selections cs WHERE cs.tenant_id=en.tenant_id AND cs.product_id=a.id AND (cs.ends_at IS NULL OR cs.ends_at>$3)))
 FROM catalog_entities ce
 JOIN catalog_entities se ON se.kind='service' AND se.scope='' AND se.key=$2 AND ce.scope=se.id::text
 JOIN addons a ON a.id=ce.id AND a.service_id=se.id
 WHERE ce.kind='addon' AND ce.key=$1`, item.Key, item.Service, now).Scan(&id, &needed)
		default:
			continue
		}
		if errors.Is(err, pgx.ErrNoRows) || err == nil && !needed {
			continue
		}
		if err != nil {
			return err
		}
		if item.Kind == "plan" {
			err = postgres.SnapshotLegacyPlanSelections(ctx, tx, id, now)
		} else {
			err = postgres.SnapshotLegacyAddOnSelections(ctx, tx, id, now)
		}
		if err != nil {
			return err
		}
	}
	return nil
}

func resolve(ctx context.Context, tx pgx.Tx, kind, scope, key string) (string, error) {
	var id string
	err := tx.QueryRow(ctx, "SELECT id::text FROM catalog_entities WHERE kind=$1 AND scope=$2 AND key=$3", kind, scope, key).Scan(&id)
	if errors.Is(err, pgx.ErrNoRows) {
		return "", fmt.Errorf("unknown %s reference %q", kind, key)
	}
	return id, err
}

func validateDatabase(ctx context.Context, tx pgx.Tx, items []definition) error {
	// Full-set checks are independent of traversal order; references to omitted
	// entities are resolved from the database after the declared entities exist.
	plans := map[string]definition{}
	ranks := map[int]bool{}
	var newDefault string
	for _, item := range items {
		if item.Kind == "plan" {
			plans[item.Key] = item
			if ranks[*item.Rank] {
				return errors.New("duplicate plan rank")
			}
			ranks[*item.Rank] = true
			if item.DefaultFree != nil && *item.DefaultFree {
				if newDefault != "" {
					return errors.New("multiple default free plans")
				}
				newDefault = item.Key
			}
		}
	}
	var currentDefaultID, currentDefaultKey string
	if err := tx.QueryRow(ctx, `SELECT COALESCE((SELECT p.id::text FROM plans p WHERE p.is_default),''),
 COALESCE((SELECT ce.key FROM plans p LEFT JOIN catalog_entities ce ON ce.id=p.id WHERE p.is_default),'')`).Scan(&currentDefaultID, &currentDefaultKey); err != nil {
		return err
	}
	if currentDefaultID != "" {
		if newDefault != "" && newDefault != currentDefaultKey && plans[currentDefaultKey].Kind == "" {
			return errors.New("cannot replace an omitted default free plan")
		}
		if newDefault == "" {
			if item, ok := plans[currentDefaultKey]; ok && (item.DefaultFree == nil || !*item.DefaultFree) {
				return errors.New("cannot clear the default free plan without a replacement")
			}
		}
	}
	for key, item := range plans {
		var existing string
		err := tx.QueryRow(ctx, "SELECT COALESCE(ce.key,'') FROM plans p LEFT JOIN catalog_entities ce ON ce.id=p.id WHERE p.rank=$1", *item.Rank).Scan(&existing)
		if err != nil && !errors.Is(err, pgx.ErrNoRows) {
			return err
		}
		if err == nil && existing != key && plans[existing].Kind == "" {
			return fmt.Errorf("plan rank %d belongs to an omitted plan", *item.Rank)
		}
	}
	// Clearing the default before inserting the replacement avoids the partial
	// unique index conflict; transaction rollback restores it on any failure.
	if newDefault != "" {
		if _, err := tx.Exec(ctx, "UPDATE plans SET is_default=false WHERE is_default"); err != nil {
			return err
		}
	}
	// Make rank swaps possible without weakening the existing unique index.
	var minimum int64
	if err := tx.QueryRow(ctx, "SELECT COALESCE(min(rank),0) FROM plans").Scan(&minimum); err != nil {
		return err
	}
	if minimum > 0 {
		minimum = 0
	}
	for _, item := range items {
		if item.Kind != "plan" {
			continue
		}
		var id string
		err := tx.QueryRow(ctx, "SELECT id::text FROM catalog_entities WHERE kind='plan' AND scope='' AND key=$1", item.Key).Scan(&id)
		if errors.Is(err, pgx.ErrNoRows) {
			continue
		}
		if err != nil {
			return err
		}
		minimum--
		if minimum < -2147483648 {
			return errors.New("no available temporary plan rank")
		}
		if _, err = tx.Exec(ctx, "UPDATE plans SET rank=$2 WHERE id=$1", id, minimum); err != nil {
			return err
		}
	}
	return nil
}

func apply(ctx context.Context, tx pgx.Tx, d definition) error {
	scope := ""
	if d.Kind == "meter" || d.Kind == "feature" || d.Kind == "addon" {
		var err error
		scope, err = resolve(ctx, tx, "service", "", d.Service)
		if err != nil {
			return err
		}
	}
	var ownerKind, ownerID string
	if d.Kind == "price" {
		var err error
		if d.AddOn != nil {
			serviceID, err := resolve(ctx, tx, "service", "", d.AddOn.Service)
			if err != nil {
				return err
			}
			ownerKind = "addon"
			ownerID, err = resolve(ctx, tx, "addon", serviceID, d.AddOn.Key)
		} else {
			ownerKind = "plan"
			ownerID, err = resolve(ctx, tx, "plan", "", d.Plan)
		}
		if err != nil {
			return err
		}
		scope = ownerKind + ":" + ownerID
	}
	id, err := postgres.EnsureCatalogEntity(ctx, tx, d.Kind, scope, d.Key, "")
	if err != nil {
		return err
	}
	switch d.Kind {
	case "service":
		_, err = tx.Exec(ctx, `INSERT INTO services(id,name,client_id,capabilities,scopes) VALUES($1,$2,$3,$4,$5)
 ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name,client_id=EXCLUDED.client_id,capabilities=EXCLUDED.capabilities,scopes=EXCLUDED.scopes`, id, d.Name, d.ClientID, d.Capabilities, d.Scopes)
	case "meter":
		err = applyMeter(ctx, tx, d, id, scope)
	case "feature":
		err = applyFeature(ctx, tx, d, id, scope)
	case "plan":
		defaultFree := d.DefaultFree != nil && *d.DefaultFree
		postpaid := d.Postpaid != nil && *d.Postpaid
		_, err = tx.Exec(ctx, `INSERT INTO plans(id,name,rank,free,is_default,postpaid) VALUES($1,$2,$3,$4,$5,$6)
 ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name,rank=EXCLUDED.rank,free=EXCLUDED.free,is_default=EXCLUDED.is_default,postpaid=EXCLUDED.postpaid`, id, d.Name, *d.Rank, *d.Free, defaultFree, postpaid)
	case "addon":
		tag, execErr := tx.Exec(ctx, `INSERT INTO addons(id,service_id,name) VALUES($1,$2,$3)
 ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name WHERE addons.service_id=EXCLUDED.service_id`, id, scope, d.Name)
		err = execErr
		if err == nil && tag.RowsAffected() != 1 {
			err = errors.New("add-on service cannot change")
		}
	case "price":
		err = applyPrice(ctx, tx, d, id, ownerKind, ownerID)
	}
	return err
}

func applyMeter(ctx context.Context, tx pgx.Tx, d definition, id, serviceID string) error {
	filters := make([]*pb.MeterFilter, 0, len(d.Filters))
	for _, item := range d.Filters {
		values := make([]string, 0, len(item.Values))
		for _, value := range item.Values {
			values = append(values, value.Value)
		}
		// YAML presence checks take no values, but the server represents an
		// existence filter with exactly one boolean value; store that form so the
		// definition validates through PutMeter and the metering engine.
		if item.Operator == "exists" && len(values) == 0 {
			values = []string{"true"}
		}
		if item.Operator == "in" {
			sort.Strings(values)
		}
		filters = append(filters, &pb.MeterFilter{Attribute: item.Attribute, Operator: operators[item.Operator], Values: values})
	}
	sort.Slice(filters, func(i, j int) bool {
		a, _ := json.Marshal(filters[i])
		b, _ := json.Marshal(filters[j])
		return string(a) < string(b)
	})
	groups := append([]string(nil), d.GroupBy...)
	sort.Strings(groups)
	definition := struct {
		Name           string              `json:"name"`
		EventType      string              `json:"event_type"`
		Aggregation    pb.MeterAggregation `json:"aggregation"`
		ValueAttribute string              `json:"value_attribute"`
		Filters        []*pb.MeterFilter   `json:"filters"`
		GroupBy        []string            `json:"group_by"`
	}{d.Key, d.EventType, aggregations[d.Aggregation], d.ValueAttribute, filters, groups}
	previous, err := postgres.ActiveCatalogVersion(ctx, tx, "meter", id)
	if err != nil {
		return err
	}
	// A meter without a version, or whose stored version has no event type
	// because a name-only PutMeter registered it, is untyped and may still be
	// typed under the in-use rule below.
	untyped := previous == ""
	if previous != "" {
		var eventType string
		if err = tx.QueryRow(ctx, "SELECT COALESCE(definition->>'event_type','') FROM catalog_versions WHERE id=$1", previous).Scan(&eventType); err != nil {
			return err
		}
		untyped = eventType == ""
	}
	if untyped {
		// An untyped meter registered through the operator API already
		// evaluated events under the compatibility definition; only that
		// definition may type it.
		typed, err := events.CatalogDefinition(d.Key, d.EventType, aggregations[d.Aggregation], d.ValueAttribute, filters, groups)
		if err != nil {
			return err
		}
		used, err := events.UntypedMeterInUse(ctx, tx, id)
		if err != nil {
			return err
		}
		if used && !events.IsCompatibilityDefinition(typed) {
			return errors.New("meter definition is immutable for its key")
		}
	}
	version, _, err := postgres.SelectCatalogVersion(ctx, tx, id, definition)
	if err != nil {
		return err
	}
	if previous != "" && previous != version && !untyped {
		return errors.New("meter definition is immutable for its key")
	}
	var count int
	if err = tx.QueryRow(ctx, "SELECT count(*) FROM meters WHERE service_id=$1 AND name<>$2", serviceID, d.Key).Scan(&count); err != nil {
		return err
	}
	if count >= 100 {
		return errors.New("service meter limit reached")
	}
	_, err = tx.Exec(ctx, "INSERT INTO meters(service_id,name) VALUES($1,$2) ON CONFLICT DO NOTHING", serviceID, d.Key)
	return err
}

func applyFeature(ctx context.Context, tx pgx.Tx, d definition, id, serviceID string) error {
	if d.Type == "metered" {
		if _, err := resolve(ctx, tx, "meter", serviceID, d.Meter); err != nil {
			return err
		}
	}
	definition := struct {
		Kind      int32  `json:"kind"`
		ValueType int32  `json:"value_type"`
		MeterKey  string `json:"meter_key"`
	}{int32(featureKinds[d.Type]), int32(valueTypes[d.ValueType]), d.Meter}
	previous, err := postgres.ActiveCatalogVersion(ctx, tx, "feature", id)
	if err != nil {
		return err
	}
	version, _, err := postgres.SelectCatalogVersion(ctx, tx, id, definition)
	if err != nil {
		return err
	}
	if previous != "" && previous != version {
		return errors.New("feature type and meter are immutable for its key")
	}
	tag, err := tx.Exec(ctx, `INSERT INTO features(id,service_id,key,name,kind,value_type,meter_key) VALUES($1,$2,$3,$4,$5,$6,$7)
 ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name WHERE features.service_id=EXCLUDED.service_id AND features.key=EXCLUDED.key AND features.kind=EXCLUDED.kind AND features.value_type=EXCLUDED.value_type AND features.meter_key=EXCLUDED.meter_key`, id, serviceID, d.Key, d.Name, featureKinds[d.Type], valueTypes[d.ValueType], d.Meter)
	if err == nil && tag.RowsAffected() != 1 {
		return errors.New("feature definition is immutable for its key")
	}
	return err
}

func applyPrice(ctx context.Context, tx pgx.Tx, d definition, stableID, ownerKind, ownerID string) error {
	serviceID, err := resolve(ctx, tx, "service", "", d.Service)
	if err != nil {
		return err
	}
	if ownerKind == "addon" {
		var addonService string
		if err = tx.QueryRow(ctx, "SELECT service_id::text FROM addons WHERE id=$1", ownerID).Scan(&addonService); err != nil {
			return err
		}
		if addonService != serviceID {
			return errors.New("add-on price belongs to another service")
		}
	}
	if d.PriceKind == "usage" {
		if _, err = resolve(ctx, tx, "meter", serviceID, d.Meter); err != nil {
			return err
		}
	}
	unit, _ := accounting.Parse(d.UnitPrice)
	included, _ := accounting.Parse(d.Included)
	kind := int32(pb.PriceKind_PRICE_KIND_USAGE)
	if d.PriceKind == "recurring" {
		kind = int32(pb.PriceKind_PRICE_KIND_RECURRING)
	}
	definition := struct {
		Service       string `json:"service"`
		Meter         string `json:"meter"`
		UnitPrice     string `json:"unit_price"`
		Included      string `json:"included"`
		Currency      string `json:"currency"`
		ExternalPrice string `json:"external_price"`
		Kind          int32  `json:"kind"`
	}{serviceID, d.Meter, unit.String(), included.String(), d.Currency, d.ExternalPriceID, kind}
	version, created, err := postgres.SelectCatalogVersion(ctx, tx, stableID, definition)
	if err != nil {
		return err
	}
	if created {
		var planID, addonID any
		if ownerKind == "plan" {
			planID = ownerID
		} else {
			addonID = ownerID
		}
		_, err = tx.Exec(ctx, `INSERT INTO prices(id,plan_id,service_id,meter,unit_price,included,currency,external_price_id,created_at,addon_id,kind)
 VALUES($1,$2,$3,$4,$5,$6,$7,$8,now(),$9,$10)`, version, planID, serviceID, d.Meter, unit.String(), included.String(), d.Currency, d.ExternalPriceID, addonID, kind)
	}
	return err
}

func replaceGrants(ctx context.Context, tx pgx.Tx, productID string, grants []grant, addonService string) error {
	if _, err := tx.Exec(ctx, "DELETE FROM catalog_product_features WHERE product_id=$1", productID); err != nil {
		return err
	}
	for _, grant := range grants {
		serviceID, err := resolve(ctx, tx, "service", "", grant.Service)
		if err != nil {
			return err
		}
		if addonService != "" && serviceID != addonService {
			return errors.New("add-on feature belongs to another service")
		}
		featureID, err := resolve(ctx, tx, "feature", serviceID, grant.Key)
		if err != nil {
			return err
		}
		var kind, valueType int
		if err = tx.QueryRow(ctx, "SELECT kind,value_type FROM features WHERE id=$1", featureID).Scan(&kind, &valueType); err != nil {
			return err
		}
		value, err := normalizeGrant(grant, kind, valueType)
		if err != nil {
			return err
		}
		data, err := json.Marshal(value)
		if err != nil {
			return err
		}
		if _, err = tx.Exec(ctx, "INSERT INTO catalog_product_features(product_id,feature_id,value) VALUES($1,$2,$3)", productID, featureID, data); err != nil {
			return err
		}
	}
	return nil
}

func normalizeGrant(g grant, kind, valueType int) (any, error) {
	switch kind {
	case 1:
		if g.Enabled == nil {
			return nil, errors.New("access feature requires enabled")
		}
		return map[string]any{"enabled": *g.Enabled}, nil
	case 2:
		if g.Limit == nil {
			return nil, errors.New("metered feature requires a limit")
		}
		decimal, err := accounting.Parse(*g.Limit)
		if err != nil || len(*g.Limit) > 64 || decimal.Cmp(accounting.Zero()) < 0 {
			return nil, errors.New("metered limit must be a nonnegative decimal")
		}
		return map[string]any{"limit": decimal.String()}, nil
	case 3:
		if g.Configuration.Kind == 0 {
			return nil, errors.New("configuration feature requires a value")
		}
		node := g.Configuration
		switch valueType {
		case 1:
			if node.Tag != "!!bool" {
				break
			}
			var value bool
			if err := node.Decode(&value); err == nil {
				return map[string]any{"boolean": value}, nil
			}
		case 2:
			if node.Tag != "!!int" {
				break
			}
			var value int64
			if err := node.Decode(&value); err == nil {
				return map[string]any{"integer": value}, nil
			}
		case 3:
			if node.Tag != "!!str" {
				break
			}
			decimal, err := accounting.Parse(node.Value)
			if err == nil && len(node.Value) <= 64 {
				return map[string]any{"decimal": decimal.String()}, nil
			}
		case 4:
			if node.Tag == "!!str" && len(node.Value) <= 4096 {
				return map[string]any{"text": node.Value}, nil
			}
		case 5:
			if node.Kind != yaml.SequenceNode || len(node.Content) > 100 {
				break
			}
			values := make([]string, 0, len(node.Content))
			for _, item := range node.Content {
				if item.Tag != "!!str" || len(item.Value) > 4096 {
					return nil, errors.New("string list items must be strings of at most 4096 bytes")
				}
				values = append(values, item.Value)
			}
			return map[string]any{"string_list": values}, nil
		}
	}
	return nil, errors.New("feature value does not match its declared type")
}
