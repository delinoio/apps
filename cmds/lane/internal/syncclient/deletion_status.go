// SPDX-License-Identifier: Apache-2.0
package syncclient

import (
	"context"
	"database/sql"
	"encoding/json"
	"io"
	"net/http"

	lanev1 "github.com/delinoio/apps/protos/lane/v1"
)

// DeletionStatus contains only a revoked session's account-generation directive.
// It never authorizes access to account data or mutation of another generation.
type DeletionStatus struct {
	Mode      lanev1.DataDeletionMode
	OldEpoch  string
	NewEpoch  string
	AccountID string
	DeviceID  string
}

func Epoch(ctx context.Context, s Store) (string, error) {
	var epoch string
	err := s.WithRead(ctx, func(c *sql.Conn) error { var e error; epoch, e = meta(ctx, c, "account_epoch"); return e })
	return epoch, err
}

// DeletionStatus uses the existing access token, even when revoked or expired.
// Do not refresh here: only this restricted server route accepts the old token.
func (a Auth) DeletionStatus(ctx context.Context, c Credential, oldEpoch string) (DeletionStatus, error) {
	var out DeletionStatus
	if !validUUID(oldEpoch) || c.AccessToken == "" || c.AccountID == "" || c.DeviceID == "" {
		return out, ErrInvalidResponse
	}
	endpoint, e := endpointURL(c.Endpoint)
	if e != nil {
		return out, e
	}
	req, e := http.NewRequestWithContext(ctx, http.MethodGet, endpoint+"/auth/deletion-status", nil)
	if e != nil {
		return out, ErrCredentials
	}
	req.Header.Set("Authorization", "Bearer "+c.AccessToken)
	req.Header.Set("Accept", "application/json")
	response, e := a.httpClient().Do(req)
	if e != nil {
		return out, ErrCredentials
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusOK {
		return out, ErrCredentials
	}
	body, e := io.ReadAll(io.LimitReader(response.Body, 64<<10))
	if e != nil {
		return out, ErrInvalidResponse
	}
	var wire struct {
		Mode         string `json:"mode"`
		OldEpoch     string `json:"old_epoch"`
		NewEpoch     string `json:"new_epoch"`
		AccountID    string `json:"account_id"`
		DeviceID     string `json:"device_id"`
		Disconnected bool   `json:"disconnected"`
	}
	if json.Unmarshal(body, &wire) != nil {
		return out, ErrInvalidResponse
	}
	if !wire.Disconnected || wire.AccountID != c.AccountID || wire.DeviceID != c.DeviceID || wire.OldEpoch != oldEpoch || !validUUID(wire.NewEpoch) || wire.NewEpoch == oldEpoch {
		return out, ErrAccountMismatch
	}
	mode := lanev1.DataDeletionMode(lanev1.DataDeletionMode_value[wire.Mode])
	if mode != lanev1.DataDeletionMode_DATA_DELETION_MODE_DELETE_EVERYWHERE && mode != lanev1.DataDeletionMode_DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL {
		return out, ErrInvalidResponse
	}
	return DeletionStatus{Mode: mode, OldEpoch: oldEpoch, NewEpoch: wire.NewEpoch, AccountID: wire.AccountID, DeviceID: wire.DeviceID}, nil
}

// ApplyDeletionStatus rechecks the local generation under its write lock. It
// returns ErrAccountDeleted only after redaction commits; the caller must then
// disable sync, remove OS credentials and coordinate removal of the space.
func ApplyDeletionStatus(ctx context.Context, s Store, status DeletionStatus) error {
	if status.DeviceID != s.DeviceID() || status.AccountID == "" || !validUUID(status.OldEpoch) || !validUUID(status.NewEpoch) || status.OldEpoch == status.NewEpoch {
		return ErrAccountMismatch
	}
	if status.Mode != lanev1.DataDeletionMode_DATA_DELETION_MODE_DELETE_EVERYWHERE && status.Mode != lanev1.DataDeletionMode_DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL {
		return ErrInvalidResponse
	}
	err := s.WithWrite(ctx, func(c *sql.Conn) error {
		epoch, e := meta(ctx, c, "account_epoch")
		if e != nil {
			return e
		}
		if epoch != status.OldEpoch {
			return ErrStaleEpoch
		}
		if status.Mode == lanev1.DataDeletionMode_DATA_DELETION_MODE_DELETE_EVERYWHERE {
			return applyChange(ctx, c, &lanev1.Change{Change: &lanev1.Change_Deletion{Deletion: &lanev1.DeletionRecord{Kind: lanev1.EntityKindOrRemoteKind_ENTITY_KIND_OR_REMOTE_KIND_ACCOUNT, Id: status.AccountID, DeletionVersionId: status.NewEpoch}}})
		}
		if e = putMeta(ctx, c, "reconnect_required", "1"); e != nil {
			return e
		}
		if e = putMeta(ctx, c, "sync_error_reason", lanev1.ErrorReason_ERROR_REASON_STALE_EPOCH.String()); e != nil {
			return e
		}
		_, e = c.ExecContext(ctx, "INSERT INTO changes(change_kind) VALUES(4)")
		return e
	})
	if err != nil {
		return err
	}
	if status.Mode == lanev1.DataDeletionMode_DATA_DELETION_MODE_DELETE_EVERYWHERE {
		return ErrAccountDeleted
	}
	return ErrReconnectRequired
}
