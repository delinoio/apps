// SPDX-License-Identifier: FSL-1.1-ALv2

package callkey

import (
	"regexp"
	"strings"
	"testing"

	"github.com/google/uuid"
)

var format = regexp.MustCompile(`^mcpa_[0-9a-f]{32}_[A-Za-z0-9_-]{43}$`)

func TestNewFormatAndParse(t *testing.T) {
	k, err := New()
	if err != nil {
		t.Fatal(err)
	}
	if !format.MatchString(k.Secret) || len(k.Secret) != Length {
		t.Fatalf("bad format %q", k.Secret)
	}
	if !strings.Contains(k.Secret, strings.ReplaceAll(k.ID.String(), "-", "")) {
		t.Fatal("secret must embed the key ID")
	}
	id, err := Parse(k.Secret)
	if err != nil || id != k.ID {
		t.Fatalf("parse = %v, %v", id, err)
	}
	if !Verify(k.Secret, k.Hash) {
		t.Fatal("verify failed")
	}
	other, _ := New()
	if Verify(other.Secret, k.Hash) {
		t.Fatal("other secret verified")
	}
	if len(k.Hash) != 32 {
		t.Fatal("hash must be SHA-256")
	}
}

func TestParseRejectsMalformed(t *testing.T) {
	k, _ := New()
	v4 := uuid.New()
	v4Key, _ := NewWithID(v4)
	cases := map[string]string{
		"empty":      "",
		"prefix":     "xcpa_" + k.Secret[5:],
		"uppercase":  strings.ToUpper(k.Secret[:10]) + k.Secret[10:],
		"short":      k.Secret[:len(k.Secret)-1],
		"long":       k.Secret + "A",
		"separator":  k.Secret[:37] + "-" + k.Secret[38:],
		"bad base64": k.Secret[:len(k.Secret)-1] + "!",
		"not v7":     v4Key.Secret,
		"dlk":        "dlk_" + k.Secret[4:],
		"whitespace": " " + k.Secret[1:],
	}
	for name, value := range cases {
		if _, err := Parse(value); err == nil {
			t.Errorf("%s: accepted %q", name, value)
		}
	}
}

func TestDisplayPrefix(t *testing.T) {
	id := uuid.MustParse("0192ab34-5678-7abc-8def-0123456789ab")
	if got := DisplayPrefix(id); got != "mcpa_0192ab34…" {
		t.Fatalf("got %q", got)
	}
}
