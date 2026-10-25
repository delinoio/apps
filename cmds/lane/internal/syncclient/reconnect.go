// SPDX-License-Identifier: Apache-2.0
package syncclient

import (
	"context"
	"database/sql"
	"errors"
	"time"

	"connectrpc.com/connect"
	lanev1 "github.com/delinoio/apps/protos/lane/v1"
	"github.com/delinoio/apps/protos/lane/v1/lanev1connect"
	"github.com/google/uuid"
	"google.golang.org/protobuf/encoding/protojson"
)

var ErrReconnectRequired = errors.New("server generation changed; run sync connect with --confirm-epoch-change and approve again")

var ErrAccountDeleted = errors.New("server deleted this account's Lane data; remove the local space and credentials")

// ApprovedReconnect is called only after explicit browser approval, while sync
// remains disabled. A generation change never authorizes an automatic worker
// reset. ErrAccountDeleted requires coordinated space removal by the caller.
func ApprovedReconnect(ctx context.Context, s Store, client lanev1connect.SyncServiceClient, accountID, tenantID string, approvals ...Credential) (err error) {
	w, err := NewWorker(s, client, Options{AccountID: accountID, TenantID: tenantID})
	if err != nil {
		return err
	}
	if err = w.lease(ctx); err != nil {
		return err
	}
	defer func() { err = errors.Join(err, w.release()) }()
	call, cancel := context.WithTimeout(ctx, 20*time.Second)
	info, err := client.GetSyncInfo(call, connect.NewRequest(&lanev1.GetSyncInfoRequest{}))
	cancel()
	if err != nil {
		return err
	}
	if info.Msg.FormatVersion != 1 {
		return ErrUnsupportedFormat
	}
	if info.Msg.AccountId != accountID || info.Msg.TenantId != tenantID {
		return ErrAccountMismatch
	}
	if !validUUID(info.Msg.Epoch) {
		return ErrInvalidResponse
	}
	var old, sequence string
	if err = s.WithRead(ctx, func(c *sql.Conn) error {
		var e error
		old, e = meta(ctx, c, "account_epoch")
		if e != nil {
			return e
		}
		return c.QueryRowContext(ctx, "SELECT CAST(coalesce(max(sequence),0) AS TEXT) FROM changes WHERE change_kind IN(1,2)").Scan(&sequence)
	}); err != nil {
		return err
	}
	var consent Credential
	if len(approvals) == 1 {
		consent = approvals[0]
	}
	if consent.ApprovalSpaceID == s.SpaceID() && (consent.ApprovalEpoch != old || consent.ApprovalSequence != sequence) {
		return ErrReconnectRequired
	}
	if old == info.Msg.Epoch {
		return nil
	}
	if old != "" {
		call, cancel = context.WithTimeout(ctx, 20*time.Second)
		directive, e := client.PullChanges(call, connect.NewRequest(&lanev1.PullChangesRequest{FormatVersion: 1, Epoch: old, PageSize: 1}))
		cancel()
		if e == nil {
			if len(directive.Msg.Changes) != 1 || directive.Msg.HasMore || directive.Msg.NextCursor != "" {
				return ErrInvalidResponse
			}
			d := directive.Msg.Changes[0].GetDeletion()
			if d == nil || d.Kind != lanev1.EntityKindOrRemoteKind_ENTITY_KIND_OR_REMOTE_KIND_ACCOUNT || d.Id != accountID || d.DeletionVersionId != info.Msg.Epoch {
				return ErrInvalidResponse
			}
			if err = s.WithWrite(ctx, func(c *sql.Conn) error {
				if e := w.guard(ctx, c); e != nil {
					return e
				}
				return applyChange(ctx, c, directive.Msg.Changes[0])
			}); err != nil {
				return err
			}
			return ErrAccountDeleted
		}
		if !hasReason(e, lanev1.ErrorReason_ERROR_REASON_STALE_EPOCH) {
			return e
		}
		confirmed := consent.ReconnectConfirmed && consent.ApprovalSpaceID == s.SpaceID() && consent.ApprovalEpoch == old && consent.AccountID == accountID && consent.TenantID == tenantID && time.Now().Before(consent.ApprovalExpiresAt)
		if !confirmed {
			e = s.WithWrite(ctx, func(c *sql.Conn) error {
				if e := w.guard(ctx, c); e != nil {
					return e
				}
				if e := putMeta(ctx, c, "reconnect_required", "1"); e != nil {
					return e
				}
				return putMeta(ctx, c, "sync_error_reason", lanev1.ErrorReason_ERROR_REASON_STALE_EPOCH.String())
			})
			if e != nil {
				return e
			}
			return ErrReconnectRequired
		}
	}
	return s.WithWrite(ctx, func(c *sql.Conn) error {
		if e := w.guard(ctx, c); e != nil {
			return e
		}
		current, e := meta(ctx, c, "account_epoch")
		if e != nil {
			return e
		}
		if current != old {
			return ErrReconnectRequired
		}
		if old != "" {
			var sequence string
			if e := c.QueryRowContext(ctx, "SELECT CAST(coalesce(max(sequence),0) AS TEXT) FROM changes WHERE change_kind IN(1,2)").Scan(&sequence); e != nil {
				return e
			}
			if sequence != consent.ApprovalSequence {
				return ErrReconnectRequired
			}
		}
		if old != "" {
			required, e := meta(ctx, c, "reconnect_required")
			if e != nil {
				return e
			}
			if required != "1" {
				return ErrReconnectRequired
			}
		}
		if old != "" {
			versions, e := orderedVersions(ctx, c)
			if e != nil {
				return e
			}
			if _, e = c.ExecContext(ctx, "DELETE FROM outbox"); e != nil {
				return e
			}
			for i, v := range versions {
				id, e := uuid.NewV7()
				if e != nil {
					return e
				}
				kind := lanev1.MutationKind_MUTATION_KIND_CREATE
				if len(v.ParentVersionIds) == 1 {
					kind = lanev1.MutationKind_MUTATION_KIND_SAVE
				}
				if len(v.ParentVersionIds) > 1 {
					kind = lanev1.MutationKind_MUTATION_KIND_RESOLVE
				}
				if v.Deleted {
					kind = lanev1.MutationKind_MUTATION_KIND_DELETE
				}
				mutation := &lanev1.SyncMutation{Meta: &lanev1.MutationMeta{RequestId: id.String(), Origin: v.Origin}, Version: v, ExpectedHeadIds: v.ParentVersionIds, Kind: kind}
				raw, e := protojson.Marshal(mutation)
				if e != nil {
					return e
				}
				if _, e = c.ExecContext(ctx, "INSERT INTO outbox(request_id,entity_id,request_json,state,created_at_us) VALUES(?,?,?,1,?)", id.String(), v.EntityId, string(raw), int64(i)); e != nil {
					return e
				}
			}
		}
		if e := putMeta(ctx, c, "account_epoch", info.Msg.Epoch); e != nil {
			return e
		}
		if e := putMeta(ctx, c, "reconnect_required", "0"); e != nil {
			return e
		}
		if e := putMeta(ctx, c, "sync_error_reason", ""); e != nil {
			return e
		}
		if e := putMeta(ctx, c, "pull_cursor", ""); e != nil {
			return e
		}
		_, e = c.ExecContext(ctx, "INSERT INTO changes(change_kind) VALUES(4)")
		return e
	})
}
func hasReason(err error, reason lanev1.ErrorReason) bool {
	var ce *connect.Error
	if !errors.As(err, &ce) {
		return false
	}
	for _, detail := range ce.Details() {
		v, e := detail.Value()
		if e == nil {
			if d, ok := v.(*lanev1.ErrorDetail); ok && d.Reason == reason {
				return true
			}
		}
	}
	return false
}
func orderedVersions(ctx context.Context, c *sql.Conn) ([]*lanev1.EntityVersion, error) {
	rows, e := c.QueryContext(ctx, "SELECT envelope_json FROM versions ORDER BY created_at_us,version_id")
	if e != nil {
		return nil, e
	}
	var pending []*lanev1.EntityVersion
	for rows.Next() {
		var raw string
		if e = rows.Scan(&raw); e != nil {
			rows.Close()
			return nil, e
		}
		v := new(lanev1.EntityVersion)
		if e = protojson.Unmarshal([]byte(raw), v); e != nil {
			rows.Close()
			return nil, e
		}
		pending = append(pending, v)
	}
	e = rows.Err()
	rows.Close()
	if e != nil {
		return nil, e
	}
	var result []*lanev1.EntityVersion
	seen := map[string]bool{}
	for len(pending) > 0 {
		var next []*lanev1.EntityVersion
		for _, v := range pending {
			ready := true
			if !v.Deleted {
				for _, p := range v.ParentVersionIds {
					if !seen[p] {
						ready = false
					}
				}
			}
			if ready {
				result = append(result, v)
				seen[v.VersionId] = true
			} else {
				next = append(next, v)
			}
		}
		if len(next) == len(pending) {
			return nil, invalid("incomplete version ancestry")
		}
		pending = next
	}
	return result, nil
}
