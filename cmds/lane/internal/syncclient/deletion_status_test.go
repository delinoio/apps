// SPDX-License-Identifier: Apache-2.0
package syncclient

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestOldTokenDeletionDirective(t *testing.T) {
	for _, mode := range []string{"DATA_DELETION_MODE_DELETE_EVERYWHERE", "DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL"} {
		t.Run(mode, func(t *testing.T) {
			s := openTestStore(t)
			enqueue(t, s, "req", version("task", "v"))
			if _, e := s.db.Exec("INSERT INTO store_meta VALUES('account_epoch',?)", tid("old")); e != nil {
				t.Fatal(e)
			}
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				if r.URL.Path != "/auth/deletion-status" || r.Method != "GET" || r.Header.Get("Authorization") != "Bearer secret-access" {
					t.Error("unexpected credential request")
				}
				json.NewEncoder(w).Encode(map[string]any{"mode": mode, "old_epoch": tid("old"), "new_epoch": tid("new"), "account_id": "account", "device_id": "device", "disconnected": true})
			}))
			defer server.Close()
			a := Auth{}
			status, e := a.DeletionStatus(context.Background(), testCredential(server.URL), tid("old"))
			if e != nil {
				t.Fatal(e)
			}
			e = ApplyDeletionStatus(context.Background(), s, status)
			if mode == "DATA_DELETION_MODE_DELETE_EVERYWHERE" {
				if !errors.Is(e, ErrAccountDeleted) || count(t, s, "SELECT count(*) FROM entities") != 0 {
					t.Fatal("deletion directive not applied")
				}
			} else {
				if !errors.Is(e, ErrReconnectRequired) || count(t, s, "SELECT count(*) FROM entities") != 1 {
					t.Fatal("keep-local lost content")
				}
			}
		})
	}
}
func TestDeletionDirectiveRejectsWrongIdentityOrEpoch(t *testing.T) {
	for _, field := range []string{"account_id", "device_id", "old_epoch", "mode", "disconnected"} {
		t.Run(field, func(t *testing.T) {
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				body := map[string]any{"mode": "DATA_DELETION_MODE_DELETE_EVERYWHERE", "old_epoch": tid("old"), "new_epoch": tid("new"), "account_id": "account", "device_id": "device", "disconnected": true}
				if field == "disconnected" {
					body[field] = false
				} else {
					body[field] = "wrong"
				}
				json.NewEncoder(w).Encode(body)
			}))
			defer server.Close()
			if _, e := (Auth{}).DeletionStatus(context.Background(), testCredential(server.URL), tid("old")); e == nil {
				t.Fatal("unbound directive accepted")
			}
		})
	}
}
