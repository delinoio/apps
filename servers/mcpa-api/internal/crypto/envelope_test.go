// SPDX-License-Identifier: FSL-1.1-ALv2

package crypto

import (
	"bytes"
	"errors"
	"testing"
)

func key(b byte) []byte { return bytes.Repeat([]byte{b}, 32) }

func TestSealOpenRoundTrip(t *testing.T) {
	kr, err := NewKeyring(map[string][]byte{"k1": key(1)}, "k1")
	if err != nil {
		t.Fatal(err)
	}
	aad := CredentialAAD("cred", "res")
	s, err := kr.Seal([]byte("sk-secret"), aad)
	if err != nil {
		t.Fatal(err)
	}
	if s.KeyID != "k1" || bytes.Contains(s.Ciphertext, []byte("sk-secret")) {
		t.Fatalf("unexpected sealed value: %+v", s)
	}
	got, err := kr.Open(s, aad)
	if err != nil || string(got) != "sk-secret" {
		t.Fatalf("open = %q, %v", got, err)
	}
}

func TestOpenRejectsOtherAADAndTampering(t *testing.T) {
	kr, _ := NewKeyring(map[string][]byte{"k1": key(1)}, "k1")
	s, _ := kr.Seal([]byte("secret"), CredentialAAD("a", "r"))
	if _, err := kr.Open(s, CredentialAAD("b", "r")); !errors.Is(err, ErrDecrypt) {
		t.Fatalf("other credential: %v", err)
	}
	if _, err := kr.Open(s, CredentialAAD("a", "other")); !errors.Is(err, ErrDecrypt) {
		t.Fatalf("other resource: %v", err)
	}
	tampered := s
	tampered.Ciphertext = append([]byte(nil), s.Ciphertext...)
	tampered.Ciphertext[0] ^= 1
	if _, err := kr.Open(tampered, CredentialAAD("a", "r")); !errors.Is(err, ErrDecrypt) {
		t.Fatalf("tampered: %v", err)
	}
	relabeled := s
	relabeled.KeyID = "k2"
	kr2, _ := NewKeyring(map[string][]byte{"k1": key(1), "k2": key(1)}, "k1")
	if _, err := kr2.Open(relabeled, CredentialAAD("a", "r")); !errors.Is(err, ErrDecrypt) {
		t.Fatalf("relabeled key ID must not open: %v", err)
	}
}

func TestOpenUnknownKey(t *testing.T) {
	old, _ := NewKeyring(map[string][]byte{"old": key(1)}, "old")
	s, _ := old.Seal([]byte("secret"), nil)
	fresh, _ := NewKeyring(map[string][]byte{"new": key(2)}, "new")
	if _, err := fresh.Open(s, nil); !errors.Is(err, ErrUnknownKey) {
		t.Fatalf("err = %v", err)
	}
}

func TestRewrapRotation(t *testing.T) {
	old, _ := NewKeyring(map[string][]byte{"old": key(1)}, "old")
	aad := CredentialAAD("c", "r")
	s, _ := old.Seal([]byte("token"), aad)

	// Step 1: both keys configured, new key active.
	both, err := NewKeyring(map[string][]byte{"old": key(1), "new": key(2)}, "new")
	if err != nil {
		t.Fatal(err)
	}
	if got, err := both.Open(s, aad); err != nil || string(got) != "token" {
		t.Fatalf("old value must stay readable during rotation: %q %v", got, err)
	}
	rotated, changed, err := both.Rewrap(s, aad)
	if err != nil || !changed {
		t.Fatalf("rewrap: changed=%v err=%v", changed, err)
	}
	if rotated.KeyID != "new" || !bytes.Equal(rotated.Ciphertext, s.Ciphertext) || !bytes.Equal(rotated.Nonce, s.Nonce) {
		t.Fatalf("rewrap must only replace the wrapped key: %+v", rotated)
	}
	again, changed, err := both.Rewrap(rotated, aad)
	if err != nil || changed || again.KeyID != "new" {
		t.Fatalf("second rewrap must be a no-op: changed=%v err=%v", changed, err)
	}
	if _, _, err := both.Rewrap(s, CredentialAAD("c", "x")); !errors.Is(err, ErrDecrypt) {
		t.Fatalf("rewrap must verify the binding: %v", err)
	}

	// Step 2: old key removed; rotated values still open.
	onlyNew, _ := NewKeyring(map[string][]byte{"new": key(2)}, "new")
	if got, err := onlyNew.Open(rotated, aad); err != nil || string(got) != "token" {
		t.Fatalf("open after removal: %q %v", got, err)
	}
	if _, err := onlyNew.Open(s, aad); !errors.Is(err, ErrUnknownKey) {
		t.Fatalf("unrotated value after removal: %v", err)
	}
}

func TestNewKeyringValidation(t *testing.T) {
	if _, err := NewKeyring(nil, "a"); err == nil {
		t.Fatal("empty keyring accepted")
	}
	if _, err := NewKeyring(map[string][]byte{"a": key(1)}, "b"); err == nil {
		t.Fatal("missing active key accepted")
	}
	if _, err := NewKeyring(map[string][]byte{"a": []byte("short")}, "a"); err == nil {
		t.Fatal("short key accepted")
	}
}

func TestAADDomainsDiffer(t *testing.T) {
	kr, _ := NewKeyring(map[string][]byte{"k": key(3)}, "k")
	s, _ := kr.Seal([]byte("x"), SessionAAD("id"))
	if _, err := kr.Open(s, OAuthFlowAAD("id", "")); err == nil {
		t.Fatal("session value opened as OAuth flow value")
	}
}
