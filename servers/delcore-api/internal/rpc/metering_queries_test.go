// SPDX-License-Identifier: FSL-1.1-ALv2

package rpc

import (
	"encoding/base64"
	"strings"
	"testing"

	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/delcore-api/internal/events"
	"github.com/delinoio/apps/servers/delcore-api/internal/metering"
)

// The largest accepted group key escapes every name and value byte. Its cursor
// must be accepted so clients can page past it.
func TestAggregatePageAcceptsWorstCaseGroupKey(t *testing.T) {
	// Each character is valid UTF-8 that encoding/json escapes as \u00XX.
	escaped := []rune("<>&\x01\x02\x03\x04\x05\x06\x07\x0b\x0e\x0f\x10\x11\x12\x13\x14\x15\x16")
	if len(escaped) < events.MaxGroupAttributes {
		t.Fatal("not enough distinct escaped characters")
	}
	values := make(map[string]metering.Value, events.MaxGroupAttributes)
	for _, r := range escaped[:events.MaxGroupAttributes] {
		values[strings.Repeat(string(r), events.MaxAttributeNameBytes)] = metering.String(strings.Repeat("\x01", events.MaxAttributeValueBytes))
	}
	if err := events.ValidateContent("call", values); err != nil {
		t.Fatalf("worst-case group values are not acceptable event attributes: %v", err)
	}
	key, err := metering.GroupKey(values)
	if err != nil {
		t.Fatal(err)
	}
	if len(key) <= 1<<16 || len(key) > events.MaxGroupKeyBytes {
		t.Fatalf("worst-case group key has %d bytes, bound %d", len(key), events.MaxGroupKeyBytes)
	}
	n, after, err := aggregatePage(&pb.Page{AfterId: base64.RawURLEncoding.EncodeToString([]byte(key))})
	if err != nil || n != 50 || after != key {
		t.Fatalf("worst-case cursor rejected: %v", err)
	}
	oversized := base64.RawURLEncoding.EncodeToString([]byte(strings.Repeat("a", events.MaxGroupKeyBytes+1)))
	if _, _, err = aggregatePage(&pb.Page{AfterId: oversized}); err == nil {
		t.Fatal("cursor above the group-key bound accepted")
	}
}
