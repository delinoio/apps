// SPDX-License-Identifier: FSL-1.1-ALv2

package rpc

import (
	"encoding/json"
	"errors"
	"regexp"
	"strings"

	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/delcore-api/internal/accounting"
	"github.com/delinoio/apps/servers/delcore-api/internal/postgres"
)

var catalogKeyPattern = regexp.MustCompile(`^[a-z][a-z0-9-]{0,99}$`)

func (c *call) catalogKey(id, supplied, fallback string) (string, error) {
	if supplied != "" {
		if !catalogKeyPattern.MatchString(supplied) {
			return "", invalid("invalid catalog key")
		}
		return supplied, nil
	}
	if id != "" {
		key, err := postgres.CatalogKey(c.ctx, c.tx, id)
		if err != nil || key != "" {
			return key, err
		}
	}
	key := strings.ToLower(strings.TrimSpace(fallback))
	key = strings.NewReplacer(" ", "-", "_", "-").Replace(key)
	if !catalogKeyPattern.MatchString(key) {
		return "", invalid("a stable catalog key is required")
	}
	return key, nil
}

func (c *call) catalogEntity(kind, scope, key, id string) (string, error) {
	entity, err := postgres.EnsureCatalogEntity(c.ctx, c.tx, kind, scope, key, id)
	if errors.Is(err, postgres.ErrCatalogIdentity) {
		return "", invalid("catalog key and ID identify different entities")
	}
	return entity, err
}

func (c *call) replaceProductFeatures(product string, values []*pb.FeatureValue, replace bool, service string) error {
	if !replace && len(values) == 0 {
		return nil
	}
	if len(values) > 100 {
		return invalid("too many feature values")
	}
	seen := map[string]bool{}
	type grant struct {
		id    string
		value []byte
	}
	grants := []grant{}
	for _, value := range values {
		if value == nil || !canonical(value.FeatureId) || seen[value.FeatureId] {
			return invalid("invalid or repeated feature")
		}
		seen[value.FeatureId] = true
		var kind, valueType int
		var featureService string
		if err := c.tx.QueryRow(c.ctx, "SELECT kind,value_type,service_id::text FROM features WHERE id=$1", value.FeatureId).Scan(&kind, &valueType, &featureService); err != nil {
			return invalid("unknown feature")
		}
		if service != "" && featureService != service {
			return invalid("add-on feature belongs to another service")
		}
		var normalized any
		switch pb.FeatureKind(kind) {
		case pb.FeatureKind_FEATURE_KIND_ACCESS:
			if value.Enabled == nil || value.Limit != "" || value.Configuration != nil {
				return invalid("access feature requires a boolean value")
			}
			normalized = map[string]any{"enabled": value.GetEnabled()}
		case pb.FeatureKind_FEATURE_KIND_METERED:
			limit, err := accounting.Parse(value.Limit)
			if value.Enabled != nil || value.Configuration != nil || err != nil || len(value.Limit) > 64 || limit.Cmp(accounting.Zero()) < 0 {
				return invalid("metered feature requires a nonnegative decimal limit")
			}
			normalized = map[string]any{"limit": limit.String()}
		case pb.FeatureKind_FEATURE_KIND_CONFIGURATION:
			if value.Enabled != nil || value.Limit != "" || value.Configuration == nil {
				return invalid("configuration feature requires a typed value")
			}
			config, err := normalizeConfiguration(value.Configuration, pb.ConfigurationValueType(valueType))
			if err != nil {
				return err
			}
			normalized = config
		default:
			return invalid("unknown feature kind")
		}
		data, err := json.Marshal(normalized)
		if err != nil {
			return err
		}
		grants = append(grants, grant{value.FeatureId, data})
	}
	if _, err := c.tx.Exec(c.ctx, "DELETE FROM catalog_product_features WHERE product_id=$1", product); err != nil {
		return err
	}
	for _, grant := range grants {
		if _, err := c.tx.Exec(c.ctx, "INSERT INTO catalog_product_features(product_id,feature_id,value) VALUES($1,$2,$3)", product, grant.id, grant.value); err != nil {
			return err
		}
	}
	return nil
}

func normalizeConfiguration(v *pb.ConfigurationValue, kind pb.ConfigurationValueType) (any, error) {
	switch typed := v.Value.(type) {
	case *pb.ConfigurationValue_Boolean:
		if kind == pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_BOOLEAN {
			return map[string]any{"boolean": typed.Boolean}, nil
		}
	case *pb.ConfigurationValue_Integer:
		if kind == pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_INTEGER {
			return map[string]any{"integer": typed.Integer}, nil
		}
	case *pb.ConfigurationValue_Decimal:
		if kind == pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_DECIMAL {
			d, err := accounting.Parse(typed.Decimal)
			if err == nil && len(typed.Decimal) <= 64 {
				return map[string]any{"decimal": d.String()}, nil
			}
		}
	case *pb.ConfigurationValue_Text:
		if kind == pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_STRING && len(typed.Text) <= 4096 {
			return map[string]any{"text": typed.Text}, nil
		}
	case *pb.ConfigurationValue_StringList:
		if kind == pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_STRING_LIST && typed.StringList != nil && len(typed.StringList.Values) <= 100 {
			for _, item := range typed.StringList.Values {
				if len(item) > 4096 {
					return nil, invalid("configuration list item is too long")
				}
			}
			return map[string]any{"string_list": typed.StringList.Values}, nil
		}
	}
	return nil, invalid("configuration value does not match its feature type")
}

func (c *call) catalogFeatureValues(version string) ([]*pb.FeatureValue, error) {
	if version == "" {
		return nil, nil
	}
	rows, err := c.tx.Query(c.ctx, "SELECT feature_id::text,value FROM catalog_version_features WHERE version_id=$1 ORDER BY feature_id", version)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var result []*pb.FeatureValue
	for rows.Next() {
		v := &pb.FeatureValue{}
		var data []byte
		if err = rows.Scan(&v.FeatureId, &data); err != nil {
			return nil, err
		}
		var raw map[string]json.RawMessage
		if err = json.Unmarshal(data, &raw); err != nil {
			return nil, err
		}
		if value, ok := raw["enabled"]; ok {
			var enabled bool
			if err = json.Unmarshal(value, &enabled); err != nil {
				return nil, err
			}
			v.Enabled = &enabled
		} else if value, ok := raw["limit"]; ok {
			if err = json.Unmarshal(value, &v.Limit); err != nil {
				return nil, err
			}
		} else {
			v.Configuration = &pb.ConfigurationValue{}
			for key, value := range raw {
				switch key {
				case "boolean":
					var x bool
					if err = json.Unmarshal(value, &x); err == nil {
						v.Configuration.Value = &pb.ConfigurationValue_Boolean{Boolean: x}
					}
				case "integer":
					var x int64
					if err = json.Unmarshal(value, &x); err == nil {
						v.Configuration.Value = &pb.ConfigurationValue_Integer{Integer: x}
					}
				case "decimal":
					var x string
					if err = json.Unmarshal(value, &x); err == nil {
						v.Configuration.Value = &pb.ConfigurationValue_Decimal{Decimal: x}
					}
				case "text":
					var x string
					if err = json.Unmarshal(value, &x); err == nil {
						v.Configuration.Value = &pb.ConfigurationValue_Text{Text: x}
					}
				case "string_list":
					var x []string
					if err = json.Unmarshal(value, &x); err == nil {
						v.Configuration.Value = &pb.ConfigurationValue_StringList{StringList: &pb.StringList{Values: x}}
					}
				}
				if err != nil {
					return nil, err
				}
			}
		}
		result = append(result, v)
	}
	return result, rows.Err()
}
