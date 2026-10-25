// SPDX-License-Identifier: Apache-2.0
package syncclient

import (
	"context"
	"database/sql"
	"path/filepath"
	"strconv"
	"time"
)

type ItemError struct {
	RequestID string `json:"requestId"`
	EntityID  string `json:"entityId"`
	Reason    string `json:"reason"`
}
type NotificationPreferences struct {
	Enabled    bool   `json:"enabled"`
	Content    string `json:"content"`
	Permission string `json:"permission"`
}
type LocalStatus struct {
	ReconnectRequired       bool                    `json:"reconnectRequired"`
	AccountID               string                  `json:"accountId"`
	SyncErrorReason         string                  `json:"syncErrorReason"`
	SpaceID                 string                  `json:"spaceId"`
	DeviceID                string                  `json:"deviceId"`
	SyncEnabled             bool                    `json:"syncEnabled"`
	Connected               bool                    `json:"connected"`
	PendingItemCount        string                  `json:"pendingItemCount"`
	ItemErrors              []ItemError             `json:"itemErrors"`
	LastSyncAt              string                  `json:"lastSyncAt,omitempty"`
	NotificationPreferences NotificationPreferences `json:"notificationPreferences"`
}

// Status reads only local state. The caller supplies the pinned registry state
// and whether usable credentials exist; this does not claim server reachability.
func Status(ctx context.Context, s Store, syncEnabled, connected bool) (LocalStatus, error) {
	out := LocalStatus{SpaceID: s.SpaceID(), DeviceID: s.DeviceID(), SyncEnabled: syncEnabled, Connected: syncEnabled && connected, ItemErrors: []ItemError{}, NotificationPreferences: NotificationPreferences{Content: "NOTIFICATION_CONTENT_DETAIL", Permission: "NOTIFICATION_PERMISSION_NOT_REQUESTED"}}
	err := s.WithRead(ctx, func(c *sql.Conn) error {
		var count int64
		if err := c.QueryRowContext(ctx, "SELECT count(*) FROM outbox").Scan(&count); err != nil {
			return err
		}
		out.PendingItemCount = strconv.FormatInt(count, 10)
		rows, err := c.QueryContext(ctx, "SELECT request_id,entity_id,error_reason FROM outbox WHERE error_reason IS NOT NULL ORDER BY created_at_us,rowid")
		if err != nil {
			return err
		}
		for rows.Next() {
			var item ItemError
			if err = rows.Scan(&item.RequestID, &item.EntityID, &item.Reason); err != nil {
				rows.Close()
				return err
			}
			out.ItemErrors = append(out.ItemErrors, item)
		}
		err = rows.Err()
		rows.Close()
		if err != nil {
			return err
		}
		values := map[string]string{}
		for _, key := range []string{"last_sync_at_us", "notification_enabled", "notification_content", "notification_permission", "reconnect_required", "sync_error_reason"} {
			value, e := meta(ctx, c, key)
			if e != nil {
				return e
			}
			values[key] = value
		}
		if us, e := strconv.ParseInt(values["last_sync_at_us"], 10, 64); e == nil && us > 0 {
			out.LastSyncAt = time.UnixMicro(us).UTC().Format(time.RFC3339Nano)
		}
		out.ReconnectRequired = values["reconnect_required"] == "1"
		out.SyncErrorReason = values["sync_error_reason"]
		out.NotificationPreferences.Enabled = values["notification_enabled"] == "1"
		if v := values["notification_content"]; v == "GENERIC" || v == "2" || v == "NOTIFICATION_CONTENT_GENERIC" {
			out.NotificationPreferences.Content = "NOTIFICATION_CONTENT_GENERIC"
		}
		if v := values["notification_permission"]; v != "" {
			out.NotificationPreferences.Permission = v
		}
		return nil
	})
	if err == nil {
		registry, e := sql.Open("sqlite", filepath.Join(s.Root(), "registry.sqlite")+"?mode=ro")
		if e != nil {
			return out, e
		}
		defer registry.Close()
		var id sql.NullString
		e = registry.QueryRowContext(ctx, "SELECT account_id FROM spaces WHERE id=?", s.SpaceID()).Scan(&id)
		if e != nil {
			return out, e
		}
		out.AccountID = id.String
	}
	return out, err
}
