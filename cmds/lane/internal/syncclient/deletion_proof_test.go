// SPDX-License-Identifier: Apache-2.0
package syncclient

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	lanev1 "github.com/delinoio/apps/protos/lane/v1"
)

func TestPrivateDeletionProofBoundAndConsumed(t *testing.T) {
	s := openTestStore(t)
	calls := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls++
		if r.URL.Path != "/auth/reauth/proof" {
			t.Error("callback attempted destructive RPC")
		}
		json.NewEncoder(w).Encode(map[string]string{"reauth_proof": "private-one-use-proof"})
	}))
	defer server.Close()
	vault := &memoryCredentials{}
	a := Auth{Credentials: vault, CredentialLockRoot: t.TempDir()}
	c := testCredential(server.URL)
	c.RedirectURI = "lane://auth/callback"
	challenge := tid("challenge")
	mode := lanev1.DataDeletionMode_DATA_DELETION_MODE_DELETE_EVERYWHERE
	if e := a.StoreDeletionChallenge(context.Background(), s, c, mode, challenge); e != nil {
		t.Fatal(e)
	}
	if _, e := a.ConsumeDeletionProof(context.Background(), s, c, mode, challenge); e == nil {
		t.Fatal("unapproved proof consumed")
	}
	wrong := c
	wrong.AccountID = "other"
	if _, e := a.CompleteDeletion(context.Background(), s, wrong, "lane://auth/callback?reauth_challenge="+challenge); e == nil {
		t.Fatal("wrong account callback")
	}
	result, e := a.CompleteDeletion(context.Background(), s, c, "lane://auth/callback?reauth_challenge="+challenge)
	if e != nil || result != challenge {
		t.Fatalf("callback %s %v", result, e)
	}
	if count(t, s, "SELECT count(*) FROM store_meta WHERE key='account_deleted'") != 0 {
		t.Fatal("callback deleted local data")
	}
	if _, e = a.ConsumeDeletionProof(context.Background(), s, c, lanev1.DataDeletionMode_DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL, challenge); e == nil {
		t.Fatal("wrong mode proof")
	}
	proof, e := a.ConsumeDeletionProof(context.Background(), s, c, mode, challenge)
	if e != nil || proof != "private-one-use-proof" {
		t.Fatal("proof unavailable")
	}
	if _, e = a.ConsumeDeletionProof(context.Background(), s, c, mode, challenge); e == nil {
		t.Fatal("proof reused")
	}
	if _, e = a.CompleteDeletion(context.Background(), s, c, "lane://auth/callback?reauth_challenge="+challenge); e == nil {
		t.Fatal("callback revived consumed challenge")
	}
	if calls != 1 {
		t.Fatal("duplicate proof redemption")
	}
}
func TestDeletionCancellationAndExpiry(t *testing.T) {
	s := openTestStore(t)
	vault := &memoryCredentials{}
	a := Auth{Credentials: vault, CredentialLockRoot: t.TempDir()}
	c := testCredential("https://lane.example")
	c.RedirectURI = "lane://auth/callback"
	mode := lanev1.DataDeletionMode_DATA_DELETION_MODE_DELETE_EVERYWHERE
	first, next := tid("first"), tid("next")
	for _, id := range []string{first, next} {
		if e := a.StoreDeletionChallenge(context.Background(), s, c, mode, id); e != nil {
			t.Fatal(e)
		}
	}
	wrong := c
	wrong.DeviceID = "different"
	if e := a.CancelDeletion(context.Background(), s, wrong, first); e == nil {
		t.Fatal("mismatched cancellation")
	}
	if e := a.CancelDeletion(context.Background(), s, c, first); e != nil {
		t.Fatal(e)
	}
	if _, e := a.readDeletion(deletionKey(s.SpaceID(), first)); e == nil {
		t.Fatal("canceled challenge usable")
	}
	if _, e := a.readDeletion(deletionKey(s.SpaceID(), next)); e != nil {
		t.Fatal("newer challenge canceled")
	}
	raw, _ := vault.Get(deletionKey(s.SpaceID(), next))
	var pending deletionChallenge
	json.Unmarshal([]byte(raw), &pending)
	pending.ExpiresAt = time.Now().Add(-time.Second)
	body, _ := json.Marshal(pending)
	vault.Set(deletionKey(s.SpaceID(), next), string(body))
	if _, e := a.ConsumeDeletionProof(context.Background(), s, c, mode, next); e == nil {
		t.Fatal("expired proof usable")
	}
}
