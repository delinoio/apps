// SPDX-License-Identifier: FSL-1.1-ALv2

package catalog

import (
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"

	"github.com/delinoio/apps/servers/delcore-api/internal/accounting"
	"go.yaml.in/yaml/v4"
)

var keyPattern = regexp.MustCompile(`^[a-z][a-z0-9-]{0,99}$`)

type reference struct {
	Service string `yaml:"service"`
	Key     string `yaml:"key"`
}

type filter struct {
	Attribute string      `yaml:"attribute"`
	Operator  string      `yaml:"operator"`
	Values    []yaml.Node `yaml:"values"`
}

type grant struct {
	Service       string    `yaml:"service"`
	Key           string    `yaml:"key"`
	Enabled       *bool     `yaml:"enabled"`
	Limit         *string   `yaml:"limit"`
	Configuration yaml.Node `yaml:"configuration"`
}

type definition struct {
	Kind            string     `yaml:"kind"`
	Key             string     `yaml:"key"`
	Name            string     `yaml:"name"`
	ClientID        string     `yaml:"client_id"`
	Capabilities    []string   `yaml:"capabilities"`
	Scopes          []string   `yaml:"scopes"`
	Service         string     `yaml:"service"`
	EventType       string     `yaml:"event_type"`
	Aggregation     string     `yaml:"aggregation"`
	ValueAttribute  string     `yaml:"value_attribute"`
	Filters         []filter   `yaml:"filters"`
	GroupBy         []string   `yaml:"group_by"`
	Type            string     `yaml:"type"`
	ValueType       string     `yaml:"value_type"`
	Meter           string     `yaml:"meter"`
	Rank            *int       `yaml:"rank"`
	Free            *bool      `yaml:"free"`
	DefaultFree     *bool      `yaml:"default_free"`
	Postpaid        *bool      `yaml:"postpaid"`
	Features        []grant    `yaml:"features"`
	Plan            string     `yaml:"plan"`
	AddOn           *reference `yaml:"add_on"`
	PriceKind       string     `yaml:"price_kind"`
	UnitPrice       string     `yaml:"unit_price"`
	Included        string     `yaml:"included"`
	Currency        string     `yaml:"currency"`
	ExternalPriceID string     `yaml:"external_price_id"`
	Path            string     `yaml:"-"`
}

var allowed = map[string]map[string]bool{}

func init() {
	for kind, fields := range map[string]string{
		"service": "name client_id capabilities scopes",
		"meter":   "service event_type aggregation value_attribute filters group_by",
		"feature": "service name type value_type meter",
		"plan":    "name rank free default_free postpaid features",
		"addon":   "service name features",
		"price":   "service plan add_on meter price_kind unit_price included currency external_price_id",
	} {
		allowed[kind] = map[string]bool{"kind": true, "key": true}
		for _, field := range strings.Fields(fields) {
			allowed[kind][field] = true
		}
	}
}

func defaultDirectory(dir string) string {
	if dir != "servers/delcore-api/catalog" {
		return dir
	}
	cwd, err := os.Getwd()
	if err == nil && filepath.Base(cwd) == "delcore-api" && filepath.Base(filepath.Dir(cwd)) == "servers" {
		return "catalog"
	}
	return dir
}

func load(dir string) ([]definition, error) {
	dir = defaultDirectory(dir)
	info, err := os.Stat(dir)
	if err != nil || !info.IsDir() {
		return nil, errors.New("catalog directory is missing or not a directory")
	}
	var paths []string
	err = filepath.WalkDir(dir, func(path string, entry os.DirEntry, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		if !entry.IsDir() && (strings.HasSuffix(path, ".yaml") || strings.HasSuffix(path, ".yml")) {
			paths = append(paths, path)
		}
		return nil
	})
	if err != nil {
		return nil, fmt.Errorf("catalog discovery: %w", err)
	}
	if len(paths) == 0 {
		return nil, errors.New("catalog directory contains no YAML files")
	}
	sort.Strings(paths)
	seen := map[string]string{}
	items := make([]definition, 0, len(paths))
	for _, path := range paths {
		data, err := os.ReadFile(path)
		if err != nil {
			return nil, fmt.Errorf("catalog file %s: %w", path, err)
		}
		decoder := yaml.NewDecoder(strings.NewReader(string(data)))
		var node yaml.Node
		if err = decoder.Decode(&node); err != nil {
			return nil, fmt.Errorf("catalog file %s: %w", path, err)
		}
		var extra yaml.Node
		if err = decoder.Decode(&extra); !errors.Is(err, io.EOF) {
			return nil, fmt.Errorf("catalog file %s: expected exactly one entity", path)
		}
		if len(node.Content) != 1 || node.Content[0].Kind != yaml.MappingNode {
			return nil, fmt.Errorf("catalog file %s: expected an entity mapping", path)
		}
		var item definition
		strict := yaml.NewDecoder(strings.NewReader(string(data)))
		strict.KnownFields(true)
		if err = strict.Decode(&item); err != nil {
			return nil, fmt.Errorf("catalog file %s: %w", path, err)
		}
		item.Path = path
		if !allowed[item.Kind]["kind"] {
			return nil, fmt.Errorf("catalog file %s: unknown kind", path)
		}
		mapping := node.Content[0].Content
		fields := map[string]bool{}
		for i := 0; i < len(mapping); i += 2 {
			field := mapping[i].Value
			if !allowed[item.Kind][field] || fields[field] {
				return nil, fmt.Errorf("catalog file %s: unknown or duplicate %s field", path, field)
			}
			fields[field] = true
		}
		if err = validate(item); err != nil {
			return nil, fmt.Errorf("catalog file %s: %w", path, err)
		}
		identity := item.Kind + ":" + item.scope() + ":" + item.Key
		if prior := seen[identity]; prior != "" {
			return nil, fmt.Errorf("catalog file %s: duplicate identity also declared in %s", path, prior)
		}
		seen[identity] = path
		items = append(items, item)
	}
	return items, nil
}

func (d definition) scope() string {
	switch d.Kind {
	case "meter", "feature", "addon":
		return d.Service
	case "price":
		if d.AddOn != nil {
			return "addon:" + d.AddOn.Service + ":" + d.AddOn.Key
		}
		return "plan:" + d.Plan
	default:
		return ""
	}
}

func validKey(key string) bool   { return keyPattern.MatchString(key) }
func validName(name string) bool { return name != "" && len(name) <= 200 }

func validate(d definition) error {
	if !validKey(d.Key) {
		return errors.New("invalid stable key")
	}
	switch d.Kind {
	case "service":
		if !validName(d.Name) || !validName(d.ClientID) || len(d.Capabilities) == 0 || len(d.Capabilities) > 100 || len(d.Scopes) > 10 {
			return errors.New("invalid service definition")
		}
		for _, capability := range d.Capabilities {
			if capability == "" || len(capability) > 100 {
				return errors.New("invalid capability")
			}
		}
		for _, scope := range d.Scopes {
			if scope != "authorize" && scope != "usage:write" && scope != "usage:read" {
				return errors.New("unknown service scope")
			}
		}
	case "meter":
		if !validKey(d.Service) || d.EventType == "" || d.Aggregation == "" || len(d.EventType) > 100 || len(d.ValueAttribute) > 100 || len(d.Filters) > 100 || len(d.GroupBy) > 20 {
			return errors.New("invalid meter definition")
		}
		if _, ok := aggregations[d.Aggregation]; !ok {
			return errors.New("unknown meter aggregation")
		}
		if d.Aggregation != "count" && d.ValueAttribute == "" {
			return errors.New("aggregation requires a value attribute")
		}
		// The metering engine rejects a count meter with a value attribute; refuse
		// the same definition here so synchronization cannot store one.
		if d.Aggregation == "count" && d.ValueAttribute != "" {
			return errors.New("count aggregation cannot have a value attribute")
		}
		groups := map[string]bool{}
		for _, group := range d.GroupBy {
			if group == "" || len(group) > 100 || groups[group] {
				return errors.New("invalid or repeated group attribute")
			}
			groups[group] = true
		}
		for _, item := range d.Filters {
			if err := validateFilter(item); err != nil {
				return err
			}
		}
	case "feature":
		if !validKey(d.Service) || !validName(d.Name) {
			return errors.New("invalid feature")
		}
		switch d.Type {
		case "access":
			if d.ValueType != "" || d.Meter != "" {
				return errors.New("access feature cannot have a meter or value type")
			}
		case "metered":
			if !validKey(d.Meter) || d.ValueType != "" {
				return errors.New("metered feature requires a meter key")
			}
		case "configuration":
			if d.Meter != "" || valueTypes[d.ValueType] == 0 {
				return errors.New("configuration feature requires a value type")
			}
		default:
			return errors.New("unknown feature type")
		}
	case "plan":
		if !validName(d.Name) || d.Rank == nil || *d.Rank < 0 || d.Free == nil || (d.DefaultFree != nil && *d.DefaultFree && !*d.Free) || (*d.Free && d.Postpaid != nil && *d.Postpaid) {
			return errors.New("invalid plan")
		}
		if err := validateGrants(d.Features); err != nil {
			return err
		}
	case "addon":
		if !validKey(d.Service) || !validName(d.Name) {
			return errors.New("invalid add-on")
		}
		if err := validateGrants(d.Features); err != nil {
			return err
		}
	case "price":
		if !validKey(d.Service) || (d.Plan == "") == (d.AddOn == nil) || d.AddOn != nil && (!validKey(d.AddOn.Service) || !validKey(d.AddOn.Key)) || d.Plan != "" && !validKey(d.Plan) || !validKey(d.Meter) || d.Currency != "USD" {
			return errors.New("invalid price references or currency")
		}
		if d.PriceKind != "usage" && d.PriceKind != "recurring" {
			return errors.New("price_kind must be usage or recurring")
		}
		if d.PriceKind == "recurring" && d.ExternalPriceID == "" {
			return errors.New("recurring price requires an external price ID")
		}
		for _, value := range []string{d.UnitPrice, d.Included} {
			decimal, err := accounting.Parse(value)
			if err != nil || len(value) > 64 || decimal.Cmp(accounting.Zero()) < 0 {
				return errors.New("price and allowance must be nonnegative decimal strings")
			}
		}
	}
	return nil
}

func validateGrants(items []grant) error {
	if len(items) > 100 {
		return errors.New("too many feature grants")
	}
	seen := map[string]bool{}
	for _, item := range items {
		if !validKey(item.Service) || !validKey(item.Key) {
			return errors.New("invalid feature reference")
		}
		identity := item.Service + ":" + item.Key
		if seen[identity] {
			return errors.New("duplicate feature grant")
		}
		seen[identity] = true
		values := 0
		if item.Enabled != nil {
			values++
		}
		if item.Limit != nil {
			values++
		}
		if item.Configuration.Kind != 0 {
			values++
		}
		if values != 1 {
			return errors.New("feature grant requires exactly one value")
		}
	}
	return nil
}

func validateFilter(item filter) error {
	if item.Attribute == "" || len(item.Attribute) > 100 {
		return errors.New("invalid filter attribute")
	}
	if _, ok := operators[item.Operator]; !ok {
		return errors.New("unknown filter operator")
	}
	if item.Operator == "exists" {
		if len(item.Values) != 0 {
			return errors.New("exists filter has no values")
		}
		return nil
	}
	if len(item.Values) == 0 || item.Operator != "in" && len(item.Values) != 1 {
		return errors.New("filter requires the expected number of values")
	}
	for _, value := range item.Values {
		if value.Kind != yaml.ScalarNode || value.Tag != "!!str" || len(value.Value) > 4096 {
			return errors.New("filter values must be quoted string scalars")
		}
		if item.Operator != "equal" && item.Operator != "not_equal" && item.Operator != "in" {
			decimal, err := accounting.Parse(value.Value)
			if err != nil || decimal.String() != value.Value {
				return errors.New("numeric filter values must be canonical decimals")
			}
		}
	}
	return nil
}
