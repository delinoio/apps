// SPDX-License-Identifier: FSL-1.1-ALv2

package api

import (
	"encoding/base64"
	"math"
	"net/http"
	"strconv"
	"strings"
	"testing"
	"time"

	"google.golang.org/protobuf/types/known/structpb"
)

func TestKeyFormatRoundTripAndRejection(t *testing.T) {
	id := newID()
	secret, hash, e := newSecret()
	if e != nil || len(hash) != 32 {
		t.Fatal(e)
	}
	raw := FormatKey(id, secret)
	gotID, gotSecret, ok := ParseKey(raw)
	if !ok || gotID != id || gotSecret != secret {
		t.Fatal("round trip failed")
	}
	for name, bad := range map[string]string{
		"prefix":    "xxx_" + raw[4:],
		"short":     raw[:len(raw)-1],
		"long":      raw + "a",
		"separator": raw[:36] + "-" + raw[37:],
		"id":        "dbk_" + strings.Repeat("z", 32) + raw[36:],
		"uppercase": "dbk_" + strings.ToUpper(raw[4:36]) + raw[36:],
		"secret":    raw[:37] + strings.Repeat("!", 43),
		"version":   "dbk_" + strings.Repeat("0", 32) + raw[36:],
	} {
		if _, _, ok := ParseKey(bad); ok {
			t.Errorf("%s accepted", name)
		}
	}
}

func TestCredentialParsing(t *testing.T) {
	h := http.Header{}
	if _, e := parseCredential(h); e == nil {
		t.Fatal("missing header accepted")
	}
	h.Set("Authorization", "Basic abc")
	if _, e := parseCredential(h); e == nil {
		t.Fatal("basic accepted")
	}
	h.Set("Authorization", "bearer login-token")
	c, e := parseCredential(h)
	if e != nil || c.keyID != "" || c.authorization != "Bearer login-token" {
		t.Fatal("login token mishandled", c, e)
	}
	secret, _, _ := newSecret()
	id := newID()
	h.Set("Authorization", "Bearer "+FormatKey(id, secret))
	if c, e = parseCredential(h); e != nil || c.keyID != id || c.secret != secret {
		t.Fatal("key mishandled", c, e)
	}
	h.Set("Authorization", "Bearer dbk_malformed")
	if _, e = parseCredential(h); e == nil {
		t.Fatal("malformed key accepted")
	}
}

func TestValidation(t *testing.T) {
	good := []string{"a", "A b", strings.Repeat("é", 128), "user@example.com"}
	for _, s := range good {
		if e := identifier(s, "x"); e != nil {
			t.Errorf("%q rejected: %v", s, e)
		}
	}
	for _, s := range []string{"", " a", "a ", "\ta", "a\n", strings.Repeat("a", 257), "a\x00b", "\xff"} {
		if e := identifier(s, "x"); e == nil {
			t.Errorf("%q accepted", s)
		}
	}
	for _, s := range []string{"", "a@example.com", "first.last+tag@sub.example.org"} {
		if e := userEmail(s); e != nil {
			t.Errorf("email %q rejected: %v", s, e)
		}
	}
	for _, s := range []string{"nope", "a@", "@b.c", "Name <a@b.c>", "a b@c.d", "a@b.c,d@e.f", strings.Repeat("a", 250) + "@b.cd"} {
		if e := userEmail(s); e == nil {
			t.Errorf("email %q accepted", s)
		}
	}
	if e := userName(strings.Repeat("n", 256)); e != nil {
		t.Error(e)
	}
	if e := userName(strings.Repeat("n", 257)); e == nil {
		t.Error("long name accepted")
	}
	if e := idempotencyKey("abcdefgh"); e != nil {
		t.Error(e)
	}
	for _, s := range []string{"", "short", strings.Repeat("a", 129), "has space!", "bad/slash1"} {
		if e := idempotencyKey(s); e == nil {
			t.Errorf("idempotency key %q accepted", s)
		}
	}
	if !canonicalUUID(newID()) || canonicalUUID("00000000-0000-4000-8000-000000000000") || canonicalUUID(strings.ToUpper(newID())) || canonicalUUID("x") {
		t.Error("canonical UUID v7 check")
	}
}

func TestMetadataLimits(t *testing.T) {
	b, e := metadataJSON(nil)
	if e != nil || string(b) != "{}" {
		t.Fatal("nil metadata", string(b), e)
	}
	small, _ := structpb.NewStruct(map[string]any{"plan": "pro", "n": 3, "nested": map[string]any{"ok": true}})
	if _, e = metadataJSON(small); e != nil {
		t.Fatal(e)
	}
	// 16 KiB is the serialized limit.
	fits, _ := structpb.NewStruct(map[string]any{"k": strings.Repeat("x", maxMetadata-len(`{"k":""}`))})
	if b, e = metadataJSON(fits); e != nil || len(b) > maxMetadata {
		t.Fatal("exact limit rejected", len(b), e)
	}
	over, _ := structpb.NewStruct(map[string]any{"k": strings.Repeat("x", maxMetadata)})
	if _, e = metadataJSON(over); e == nil {
		t.Fatal("oversize metadata accepted")
	}
}

func TestCursorRoundTrip(t *testing.T) {
	at := time.Date(2026, 10, 20, 1, 2, 3, 456789000, time.UTC)
	id := newID()
	c, e := decodeCursor(encodeCursor(at, id))
	if e != nil || !c.At.Equal(at) || c.ID != id {
		t.Fatal(c, e)
	}
	if c, e = decodeCursor(""); c != nil || e != nil {
		t.Fatal("empty token")
	}
	// Tokens whose timestamp the service could never have issued are malformed, including the
	// int64 extremes that overflow PostgreSQL's epoch conversion.
	for _, micro := range []int64{math.MinInt64, math.MinInt64 + 1, -1, maxCursor.UnixMicro() + 1, math.MaxInt64} {
		forged := base64.RawURLEncoding.EncodeToString([]byte(strconv.FormatInt(micro, 10) + "." + id))
		if _, e = decodeCursor(forged); e == nil {
			t.Errorf("timestamp %d accepted", micro)
		}
	}
	for _, edge := range []time.Time{minCursor, maxCursor} {
		if _, e = decodeCursor(encodeCursor(edge, id)); e != nil {
			t.Errorf("edge %v rejected: %v", edge, e)
		}
	}
	for _, bad := range []string{"%%%", "MTIz", encodeCursor(at, "not-a-uuid")} {
		if _, e = decodeCursor(bad); e == nil {
			t.Errorf("%q accepted", bad)
		}
	}
	for _, n := range []int32{-1, 101} {
		if _, e = pageSize(n); e == nil {
			t.Errorf("page size %d accepted", n)
		}
	}
	if n, e := pageSize(0); n != 50 || e != nil {
		t.Fatal("default page size")
	}
}
