// SPDX-License-Identifier: FSL-1.1-ALv2

package rpc

import (
	"bytes"
	"encoding/base64"
	"strings"
	"testing"

	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/delcore-api/internal/events"
	"github.com/delinoio/apps/servers/delcore-api/internal/metering"
)

// Aggregate cursors carry the fixed-size group digest, so the cursor of the
// largest accepted group key is as small as any other and clients can page past
// it. Only base64url encodings of exactly one digest are cursors.
func TestAggregatePageAcceptsGroupDigestCursors(t *testing.T) {
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
	digest := events.GroupDigest(key)
	n, after, err := aggregatePage(&pb.Page{AfterId: base64.RawURLEncoding.EncodeToString(digest)})
	if err != nil || n != 50 || !bytes.Equal(after, digest) {
		t.Fatalf("worst-case group digest cursor rejected: %v", err)
	}
	// The first page starts after the empty digest, which is not SQL NULL.
	if _, after, err = aggregatePage(nil); err != nil || after == nil || len(after) != 0 {
		t.Fatalf("first page cursor = %v, %v", after, err)
	}
	for _, cursor := range []string{"!!", "AA", base64.RawURLEncoding.EncodeToString(digest[:31]), base64.RawURLEncoding.EncodeToString(append(digest, 0)), base64.RawURLEncoding.EncodeToString([]byte(key))} {
		if _, _, err = aggregatePage(&pb.Page{AfterId: cursor}); err == nil {
			t.Fatalf("cursor %.16q accepted", cursor)
		}
	}
}
