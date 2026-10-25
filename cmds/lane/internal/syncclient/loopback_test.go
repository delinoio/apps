// SPDX-License-Identifier: Apache-2.0
package syncclient

import (
	"context"
	"encoding/json"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"testing"
)

func TestStandaloneLoopbackApproval(t *testing.T) {
	s := openTestStore(t)
	tokenServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { json.NewEncoder(w).Encode(testCredential("")) }))
	defer tokenServer.Close()
	listener, e := net.Listen("tcp4", "127.0.0.1:0")
	if e != nil {
		t.Fatal(e)
	}
	redirect := "http://" + listener.Addr().String() + "/auth/callback"
	a := Auth{Credentials: &memoryCredentials{}, CredentialLockRoot: t.TempDir()}
	credential, e := a.approveListener(context.Background(), s, tokenServer.URL, "account", false, func(approval Approval) error {
		u, e := url.Parse(approval.AuthorizationURL)
		if e != nil {
			return e
		}
		if u.Query().Get("redirect_uri") != redirect {
			t.Error("incorrect registered callback")
		}
		func() {
			response, e := http.Get(redirect + "?code=code&state=" + approval.ChallengeID)
			if e != nil {
				t.Error(e)
				return
			}
			defer response.Body.Close()
			if response.StatusCode != 200 {
				t.Errorf("callback status %d", response.StatusCode)
			}
		}()
		return nil
	}, listener, redirect)
	if e != nil || credential.AccountID != "account" {
		t.Fatalf("loopback result %v %v", credential.AccountID, e)
	}
}
