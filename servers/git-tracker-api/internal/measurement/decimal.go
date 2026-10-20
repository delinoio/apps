// SPDX-License-Identifier: FSL-1.1-ALv2

package measurement

import (
	"errors"
	"math/big"
	"regexp"
	"strings"

	pb "github.com/delinoio/apps/protos/gittracker/v1"
)

var decimalPattern = regexp.MustCompile(`^-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?$`)

// Parse never passes through a binary floating-point value. Bounds protect the
// parser from exponent expansion attacks, not product measurement quotas.
func Parse(s string) (*big.Rat, error) {
	if len(s) > 4096 || !decimalPattern.MatchString(s) {
		return nil, errors.New("invalid finite decimal")
	}
	if i := strings.IndexAny(s, "eE"); i >= 0 {
		e, ok := new(big.Int).SetString(strings.TrimPrefix(s[i+1:], "+"), 10)
		if !ok || !e.IsInt64() || e.Int64() > 4096 || e.Int64() < -4096 {
			return nil, errors.New("decimal exponent out of range")
		}
	}
	v, ok := new(big.Rat).SetString(s)
	if !ok {
		return nil, errors.New("invalid decimal")
	}
	return v, nil
}

// Text is for display only. Regression decisions always use exact rationals.
func Text(v *big.Rat) string {
	s := v.FloatString(18)
	s = strings.TrimRight(strings.TrimRight(s, "0"), ".")
	if s == "-0" {
		return "0"
	}
	return s
}

type unit struct {
	kind  string
	scale *big.Rat
}

var units = func() map[string]unit {
	m := map[string]unit{}
	add := func(name, kind, scale string) { r, _ := new(big.Rat).SetString(scale); m[name] = unit{kind, r} }
	for name, scale := range map[string]string{"B": "1", "kB": "1000", "MB": "1000000", "GB": "1000000000", "KiB": "1024", "MiB": "1048576", "GiB": "1073741824"} {
		add(name, "size", scale)
		add(name+"/s", "throughput", scale)
	}
	for name, scale := range map[string]string{"ns": "1/1000000000", "μs": "1/1000000", "ms": "1/1000", "s": "1"} {
		add(name, "time", scale)
	}
	add("ops/s", "operations", "1")
	add("count", "count", "1")
	add("%", "ratio", "1")
	add("unitless", "unitless", "1")
	return m
}()

func Validate(d *pb.Dataset, rows []*pb.Row) error {
	if d == nil || d.Id == "" || d.Name == "" || len(d.Columns) == 0 {
		return errors.New("dataset id, name and columns are required")
	}
	cols := map[string]bool{}
	for _, c := range d.Columns {
		if c == nil || c.Id == "" || cols[c.Id] {
			return errors.New("column IDs must be unique and nonempty")
		}
		cols[c.Id] = true
		if _, ok := units[c.Unit]; !ok {
			return errors.New("unsupported column unit")
		}
		if c.Direction < pb.Direction_DIRECTION_LOWER || c.Direction > pb.Direction_DIRECTION_NEUTRAL {
			return errors.New("invalid direction")
		}
		for _, s := range []string{c.AbsoluteThreshold, c.PercentageThreshold} {
			if s != "" {
				v, e := Parse(s)
				if e != nil || v.Sign() < 0 {
					return errors.New("threshold must be a nonnegative decimal")
				}
			}
		}
	}
	seen := map[string]bool{}
	for _, r := range rows {
		if r == nil || r.Id == "" || seen[r.Id] {
			return errors.New("row IDs must be unique and nonempty")
		}
		seen[r.Id] = true
		if len(r.Values) != len(cols) {
			return errors.New("row " + r.Id + ": all columns are required")
		}
		for k, v := range r.Values {
			if !cols[k] {
				return errors.New("row " + r.Id + ": unknown column")
			}
			if _, e := Parse(v); e != nil {
				return errors.New("row " + r.Id + ": invalid decimal")
			}
		}
	}
	return nil
}

func Compare(target, base *pb.Column, value, baseline string) *pb.Comparison {
	out := &pb.Comparison{ColumnId: target.Id, Value: value, Unit: target.Unit, Status: pb.ComparisonStatus_COMPARISON_STATUS_NEW}
	if base == nil {
		return out
	}
	a, okA := units[target.Unit]
	b, okB := units[base.Unit]
	if !okA || !okB || a.kind != b.kind {
		out.Status = pb.ComparisonStatus_COMPARISON_STATUS_INCOMPATIBLE
		return out
	}
	v, e := Parse(value)
	if e != nil {
		return out
	}
	old, e := Parse(baseline)
	if e != nil {
		return out
	}
	old.Mul(old, b.scale).Quo(old, a.scale)
	delta := new(big.Rat).Sub(v, old)
	out.BaselineValue = baseline
	out.BaselineUnit = base.Unit
	out.Delta = Text(delta)
	var pct *big.Rat
	if old.Sign() != 0 {
		pct = new(big.Rat).Quo(delta, new(big.Rat).Abs(old))
		pct.Mul(pct, big.NewRat(100, 1))
		out.Percentage = Text(pct)
	}
	out.Status = pb.ComparisonStatus_COMPARISON_STATUS_UNCHANGED
	if delta.Sign() == 0 {
		return out
	}
	out.Status = pb.ComparisonStatus_COMPARISON_STATUS_CHANGED
	if target.Direction == pb.Direction_DIRECTION_NEUTRAL {
		return out
	}
	worse := (target.Direction == pb.Direction_DIRECTION_LOWER && delta.Sign() > 0) || (target.Direction == pb.Direction_DIRECTION_HIGHER && delta.Sign() < 0)
	if !worse {
		out.Status = pb.ComparisonStatus_COMPARISON_STATUS_IMPROVEMENT
		return out
	}
	for _, t := range []struct {
		s string
		v *big.Rat
	}{{target.AbsoluteThreshold, delta}, {target.PercentageThreshold, pct}} {
		if t.s != "" && t.v != nil {
			limit, e := Parse(t.s)
			if e == nil && new(big.Rat).Abs(t.v).Cmp(limit) > 0 {
				out.Status = pb.ComparisonStatus_COMPARISON_STATUS_REGRESSION
			}
		}
	}
	return out
}
