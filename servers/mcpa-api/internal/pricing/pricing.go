// SPDX-License-Identifier: FSL-1.1-ALv2

// Package pricing holds the traffic price and exact decimal helpers.
package pricing

import (
	"errors"
	"math/big"
	"regexp"
	"strings"
)

// Delcore catalog identities of the mcpa traffic meter.
const (
	ServiceKey = "mcpa"
	// MeterKey is the Delcore meter key. Delcore stable keys only allow
	// lowercase letters, digits and hyphens.
	MeterKey = "mcpa-traffic-bytes"
	// PriceKey is the Delcore price key of the traffic price.
	PriceKey = "mcpa-traffic"
	// EventType is the event type of traffic events.
	EventType = "mcpa.traffic"
	// AttributeBytes is the numeric attribute summed by the meter.
	AttributeBytes = "bytes"
	// AttributeResourceID is the group-by attribute of the meter.
	AttributeResourceID = "resource_id"
)

// UnitPricePerByte is USD 0.07 per GiB (0.07 / 2^30) as an exact decimal.
const UnitPricePerByte = "0.00000000006519258022308349609375"

var (
	gib          = new(big.Rat).SetInt64(1 << 30)
	usdPerGiB    = big.NewRat(7, 100)
	usdPerByte   = new(big.Rat).Quo(usdPerGiB, gib)
	limitPattern = regexp.MustCompile(`^(0|[1-9][0-9]*)(\.[0-9]+)?$`)
)

// Maximum fraction digits of exact byte-derived amounts. Any byte count is
// n / 2^30, which terminates after at most 30 fraction digits, and the USD
// amount n * 7 / (100 * 2^30) terminates after at most 32 fraction digits, so
// rendering with these widths never rounds.
const (
	gibFractionDigits = 30
	usdFractionDigits = 32
)

// MaxLimitLength is the maximum length in characters of a budget limit.
const MaxLimitLength = 64

// PerByte returns the USD price of one byte.
func PerByte() *big.Rat { return new(big.Rat).Set(usdPerByte) }

// SpendUSD returns the exact USD amount of bytes.
func SpendUSD(bytes int64) *big.Rat {
	return new(big.Rat).Mul(new(big.Rat).SetInt64(bytes), usdPerByte)
}

// GiB returns bytes / 2^30 as an exact canonical decimal (at most 30 fraction
// digits, no trailing zeros). Display rounding belongs to the console.
func GiB(bytes int64) string {
	return Format(new(big.Rat).Quo(new(big.Rat).SetInt64(bytes), gib), gibFractionDigits)
}

// EstimateUSD returns the exact estimated USD amount of bytes (at most 32
// fraction digits, no trailing zeros). Display rounding belongs to the console.
func EstimateUSD(bytes int64) string {
	return Format(SpendUSD(bytes), usdFractionDigits)
}

// Format renders r rounded half-up to at most places decimals, without
// trailing zeros.
func Format(r *big.Rat, places int) string {
	s := r.FloatString(places)
	neg := strings.HasPrefix(s, "-")
	// FloatString rounds half away from zero, which equals half-up for the
	// nonnegative amounts used here.
	if strings.Contains(s, ".") {
		s = strings.TrimRight(s, "0")
		s = strings.TrimSuffix(s, ".")
	}
	if s == "-0" || (neg && s == "-") {
		return "0"
	}
	return s
}

// ErrInvalidLimit means a budget limit is not a canonical nonnegative decimal.
var ErrInvalidLimit = errors.New("limit must be a canonical nonnegative decimal with at most 64 characters")

// CanonicalDecimal removes the trailing fraction zeros of a canonical
// nonnegative decimal string, so "10.50" becomes "10.5" and "1.000" becomes
// "1". It works on the digits and never rounds. It reports false when raw is
// not a canonical nonnegative decimal (no sign, exponent, leading zeros,
// missing integer or fraction digits, or whitespace). It does not bound the
// length; ParseLimit does.
func CanonicalDecimal(raw string) (string, bool) {
	if !limitPattern.MatchString(raw) {
		return "", false
	}
	if strings.Contains(raw, ".") {
		raw = strings.TrimRight(raw, "0")
		raw = strings.TrimSuffix(raw, ".")
	}
	return raw, true
}

// ParseLimit validates a USD budget limit and returns its canonical form.
// The raw input is limited to MaxLimitLength characters. Trailing fractional
// zeros are removed, so "10.50" becomes "10.5". The value is never rounded.
func ParseLimit(raw string) (string, *big.Rat, error) {
	if len(raw) > MaxLimitLength {
		return "", nil, ErrInvalidLimit
	}
	canonical, ok := CanonicalDecimal(raw)
	if !ok {
		return "", nil, ErrInvalidLimit
	}
	r, ok := new(big.Rat).SetString(canonical)
	if !ok {
		return "", nil, ErrInvalidLimit
	}
	return canonical, r, nil
}
