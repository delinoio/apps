// SPDX-License-Identifier: FSL-1.1-ALv2

package credential

import (
	"bytes"
	"testing"

	"github.com/delinoio/apps/servers/mcpa-api/internal/crypto"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

func TestSealOpen(t *testing.T) {
	kr, _ := crypto.NewKeyring(map[string][]byte{"k": bytes.Repeat([]byte{7}, 32)}, "k")
	c := store.Credential{ID: store.NewID(), ResourceID: store.NewID()}
	s, err := Seal(kr, c, Payload{Kind: KindAPIKey, APIKey: "sk-test-123456"})
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Contains(s.Ciphertext, []byte("sk-test")) {
		t.Fatal("plaintext in ciphertext")
	}
	p, err := Open(kr, c, s)
	if err != nil || p.APIKey != "sk-test-123456" || p.Kind != KindAPIKey || p.Version != PayloadVersion {
		t.Fatalf("open: %+v %v", p, err)
	}
	other := c
	other.ResourceID = store.NewID()
	if _, err := Open(kr, other, s); err == nil {
		t.Fatal("payload opened for another resource")
	}
}

func TestValidation(t *testing.T) {
	if ValidateAPIKey("sk-abc") != nil || ValidateAPIKey("") == nil || ValidateAPIKey("a b") == nil || ValidateAPIKey("ключ") == nil {
		t.Fatal("api key validation")
	}
	if SecretHint("short") != "" || SecretHint("sk-1234567890abcd") != "abcd" {
		t.Fatal("hint")
	}
	models, err := NormalizeModels([]string{" gpt-4o ", "gpt-4o", "o3"}, "models")
	if err != nil || len(models) != 2 || models[0] != "gpt-4o" {
		t.Fatalf("models %v %v", models, err)
	}
	if _, err := NormalizeModels([]string{"a b"}, "models"); err == nil {
		t.Fatal("space accepted")
	}
	for _, ok := range []string{"https://api.example.com/v1", "https://8.8.8.8/v1"} {
		if err := ValidateUpstreamURL(ok); err != nil {
			t.Errorf("%s: %v", ok, err)
		}
	}
	for _, bad := range []string{
		"http://api.example.com", "https://localhost/v1", "https://127.0.0.1", "https://10.0.0.1", "https://169.254.169.254/latest",
		"https://[::1]/", "https://user:pw@example.com", "https://example.com/?a=b", "https://metadata.google.internal", "https://intranet",
		"https://100.64.1.1", "https://[fd00::1]/",
	} {
		if err := ValidateUpstreamURL(bad); err == nil {
			t.Errorf("accepted %s", bad)
		}
	}
}
