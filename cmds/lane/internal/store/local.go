// SPDX-License-Identifier: Apache-2.0
package store

import (
	"context"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"errors"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/reflect/protoreflect"
	"google.golang.org/protobuf/reflect/protoregistry"
	"strconv"
)

type AccountIdentity struct {
	Issuer, AccountID, TenantID string
	SyncEnabled                 bool
}

func (s *Store) Identity(ctx context.Context) (a AccountIdentity, e error) {
	e = s.registry.QueryRowContext(ctx, "SELECT coalesce(issuer,''),coalesce(account_id,''),coalesce(tenant_id,''),sync_enabled FROM spaces WHERE id=?", s.space).Scan(&a.Issuer, &a.AccountID, &a.TenantID, &a.SyncEnabled)
	return
}

// SelectAccount requires completed browser approval from the caller. It never merges another space and leaves sync disabled until explicit enablement.
func (s *Store) SelectAccount(ctx context.Context, issuer, accountID, tenantID string) (*Store, error) {
	if issuer == "" || accountID == "" || tenantID == "" {
		return nil, domainError(1, "Approved account identity is required", "")
	}
	sum := sha256.Sum256([]byte(issuer + "\x00" + accountID + "\x00" + tenantID))
	key := hex.EncodeToString(sum[:])
	var id string
	e := transaction(ctx, s.registry, true, func(c *sql.Conn) error {
		if e := compatible(ctx, c, ""); e != nil {
			return e
		}
		e := c.QueryRowContext(ctx, "SELECT id FROM spaces WHERE account_key=?", key).Scan(&id)
		if errors.Is(e, sql.ErrNoRows) {
			id = NewID()
			_, e = c.ExecContext(ctx, "INSERT INTO spaces(id,account_key,account_id,tenant_id,issuer,sync_enabled) VALUES(?,?,?,?,?,0)", id, key, accountID, tenantID, issuer)
		}
		if e != nil {
			return e
		}
		if _, e = c.ExecContext(ctx, "UPDATE spaces SET sync_enabled=0 WHERE id=?", id); e != nil {
			return e
		}
		_, e = c.ExecContext(ctx, "UPDATE registry_meta SET value=? WHERE key='active_space_id'", id)
		return e
	})
	if e != nil {
		return nil, e
	}
	return Open(s.root, id)
}
func (s *Store) SetSyncEnabled(ctx context.Context, enabled bool) error {
	return transaction(ctx, s.registry, true, func(c *sql.Conn) error {
		if e := compatible(ctx, c, ""); e != nil {
			return e
		}
		r, e := c.ExecContext(ctx, "UPDATE spaces SET sync_enabled=? WHERE id=?", enabled, s.space)
		if e != nil {
			return e
		}
		n, _ := r.RowsAffected()
		if n != 1 {
			return domainError(7, "Space is unavailable", s.space)
		}
		return nil
	})
}

// RemoveSpace drains writers and removes logical data. Empty database files remain because
// another process may still hold a SQLite connection; unlinking them could split writers.
func (s *Store) RemoveSpace(ctx context.Context) error {
	e := s.WithWrite(ctx, func(c *sql.Conn) error {
		for _, q := range []string{"DELETE FROM heads", "DELETE FROM versions", "DELETE FROM entities", "DELETE FROM receipts", "DELETE FROM outbox", "DELETE FROM remote_objects", "DELETE FROM notification_receipts", "DELETE FROM repository_mappings", "DELETE FROM store_meta WHERE key NOT IN ('space_id','device_id','format_version')", "INSERT INTO changes(change_kind) VALUES(3)"} {
			if _, e := c.ExecContext(ctx, q); e != nil {
				return e
			}
		}
		// The marker is in the same store transaction, so queued writers fail their entry check.
		_, e := c.ExecContext(ctx, "INSERT OR REPLACE INTO store_meta(key,value) VALUES('removed','1')")
		return e
	})
	if e != nil {
		return e
	}
	e = transaction(ctx, s.registry, true, func(c *sql.Conn) error {
		if _, e := c.ExecContext(ctx, "DELETE FROM spaces WHERE id=?", s.space); e != nil {
			return e
		}
		_, e := c.ExecContext(ctx, "DELETE FROM registry_meta WHERE key='active_space_id' AND value=?", s.space)
		return e
	})
	if e != nil {
		return e
	}
	_, e = s.db.ExecContext(ctx, "PRAGMA wal_checkpoint(TRUNCATE)")
	return storageError(e)
}

type NotificationPreferences struct {
	Enabled    bool   `json:"enabled"`
	Content    string `json:"content"`
	Permission string `json:"permission"`
}

func (s *Store) preferences(ctx context.Context, c *sql.Conn) (NotificationPreferences, error) {
	p := NotificationPreferences{Content: "NOTIFICATION_CONTENT_DETAIL", Permission: "NOTIFICATION_PERMISSION_UNSUPPORTED"}
	rows, e := c.QueryContext(ctx, "SELECT key,value FROM store_meta WHERE key IN ('notification_enabled','notification_content')")
	if e != nil {
		return p, e
	}
	defer rows.Close()
	for rows.Next() {
		var k, v string
		if e = rows.Scan(&k, &v); e != nil {
			return p, e
		}
		if k == "notification_enabled" {
			p.Enabled = v == "1"
		}
		if k == "notification_content" && v == "GENERIC" {
			p.Content = "NOTIFICATION_CONTENT_GENERIC"
		}
	}
	return p, rows.Err()
}
func (s *Store) InvokeJSON(ctx context.Context, method string, raw []byte) ([]byte, error) {
	switch method {
	case "GetNotificationPreferences":
		var p NotificationPreferences
		e := s.WithRead(ctx, func(c *sql.Conn) error { var e error; p, e = s.preferences(ctx, c); return e })
		if e != nil {
			return nil, e
		}
		return json.Marshal(p)
	case "SetNotificationPreferences":
		var in struct {
			Enabled bool            `json:"enabled"`
			Content json.RawMessage `json:"content"`
		}
		if e := json.Unmarshal(raw, &in); e != nil {
			return nil, domainError(1, "Invalid preferences", "")
		}
		content := "DETAIL"
		switch string(in.Content) {
		case "", `"NOTIFICATION_CONTENT_DETAIL"`, `1`:
		case `"NOTIFICATION_CONTENT_GENERIC"`, `2`:
			content = "GENERIC"
		default:
			return nil, domainError(1, "Invalid notification content", "")
		}
		e := s.WithWrite(ctx, func(c *sql.Conn) error {
			if _, e := c.ExecContext(ctx, "UPDATE store_meta SET value=? WHERE key='notification_enabled'", map[bool]string{false: "0", true: "1"}[in.Enabled]); e != nil {
				return e
			}
			if _, e := c.ExecContext(ctx, "UPDATE store_meta SET value=? WHERE key='notification_content'", content); e != nil {
				return e
			}
			_, e := c.ExecContext(ctx, "INSERT INTO changes(change_kind) VALUES(3)")
			return e
		})
		if e != nil {
			return nil, e
		}
		return s.InvokeJSON(ctx, "GetNotificationPreferences", []byte(`{}`))
	case "GetLocalStatus":
		a, e := s.Identity(ctx)
		if e != nil {
			return nil, e
		}
		out := map[string]any{"spaceId": s.space, "deviceId": s.device, "syncEnabled": a.SyncEnabled, "connected": false}
		e = s.WithRead(ctx, func(c *sql.Conn) error {
			var n int64
			if e := c.QueryRowContext(ctx, "SELECT count(*) FROM outbox").Scan(&n); e != nil {
				return e
			}
			out["pendingItemCount"] = strconv.FormatInt(n, 10)
			p, e := s.preferences(ctx, c)
			if e != nil {
				return e
			}
			out["notificationPreferences"] = p
			items := []map[string]string{}
			rows, e := c.QueryContext(ctx, "SELECT request_id,entity_id,error_reason FROM outbox WHERE error_reason IS NOT NULL ORDER BY request_id")
			if e != nil {
				return e
			}
			defer rows.Close()
			for rows.Next() {
				var r, id, reason string
				if e = rows.Scan(&r, &id, &reason); e != nil {
					return e
				}
				items = append(items, map[string]string{"requestId": r, "entityId": id, "reason": reason})
			}
			out["itemErrors"] = items
			return rows.Err()
		})
		if e != nil {
			return nil, e
		}
		return json.Marshal(out)
	}
	mt, e := protoregistry.GlobalTypes.FindMessageByName(protoreflect.FullName("lane.v1." + method + "Request"))
	if e != nil {
		return nil, domainError(1, "Unknown method", "")
	}
	m := mt.New().Interface()
	if e = protojson.Unmarshal(raw, m); e != nil {
		return nil, domainError(1, "Invalid request", "")
	}
	r, e := s.Invoke(ctx, method, m)
	if e != nil {
		return nil, e
	}
	return protojson.Marshal(r)
}
