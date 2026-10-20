// SPDX-License-Identifier: FSL-1.1-ALv2
package rpc

import (
	"crypto/sha256"
	"encoding/hex"
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"testing"
	"time"
)

func TestUsageFingerprintPreservesLegacyEncoding(t *testing.T) {
	u := &pb.Usage{TenantId: "tenant", EventKey: "event", Meter: "units"}
	occurred := time.Date(2026, 1, 2, 3, 4, 5, 0, time.UTC)
	legacy := []byte(`{"Tenant":"tenant","Service":"service","Key":"event","Meter":"units","Quantity":"1","Correction":"","Time":"2026-01-02T03:04:05Z"}`)
	sum := sha256.Sum256(legacy)
	want := hex.EncodeToString(sum[:])
	if got := usageFingerprint(u, "service", "1", occurred, nil); got != want {
		t.Fatalf("legacy fingerprint changed: %s != %s", got, want)
	}
	first := usageFingerprint(u, "service", "1", occurred, []byte(`{"price":"1"}`))
	second := usageFingerprint(u, "service", "1", occurred, []byte(`{"price":"2"}`))
	if first == want || first == second {
		t.Fatal("AI evidence must remain part of the fingerprint")
	}
}
