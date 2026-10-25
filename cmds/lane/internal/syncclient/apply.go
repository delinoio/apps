// SPDX-License-Identifier: Apache-2.0
package syncclient

import (
	"context"
	"database/sql"
	"errors"
	"github.com/google/uuid"

	lanev1 "github.com/delinoio/apps/protos/lane/v1"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
)

func applyMappings(ctx context.Context, c *sql.Conn, mappings []*lanev1.RepositoryMapping) error {
	for _, m := range mappings {
		if m.LocalId == "" || m.CanonicalId == "" || m.LocalId == m.CanonicalId {
			return invalid("repository mapping")
		}
		var old string
		err := c.QueryRowContext(ctx, "SELECT canonical_id FROM repository_mappings WHERE local_id=?", m.LocalId).Scan(&old)
		if err != nil && !errors.Is(err, sql.ErrNoRows) {
			return err
		}
		if old != "" && old != m.CanonicalId {
			return invalid("changed repository mapping")
		}
		if _, err = c.ExecContext(ctx, "INSERT OR IGNORE INTO repository_mappings(local_id,canonical_id) VALUES(?,?)", m.LocalId, m.CanonicalId); err != nil {
			return err
		}
		// Aliases retain immutable versions; only the canonical projection reserves the URL.
		if _, err = c.ExecContext(ctx, "UPDATE entities SET normalized_url=NULL WHERE id=?", m.LocalId); err != nil {
			return err
		}
		if _, err = c.ExecContext(ctx, "UPDATE entities SET repository_id=? WHERE repository_id=?", m.CanonicalId, m.LocalId); err != nil {
			return err
		}
	}
	return nil
}

// ApplySyncResult applies server state inside the caller's write transaction.
// Local descendants and concurrent heads remain intact until explicitly resolved.
func ApplySyncResult(ctx context.Context, c *sql.Conn, r *lanev1.EntityResult) error {
	if r == nil || r.Version == nil {
		return invalid("missing entity version")
	}
	v := r.Version
	if err := validateVersion(v); err != nil {
		return err
	}
	var deleted int
	err := c.QueryRowContext(ctx, "SELECT deleted FROM entities WHERE id=?", v.EntityId).Scan(&deleted)
	if err != nil && !errors.Is(err, sql.ErrNoRows) {
		return err
	}
	if deleted != 0 && !v.Deleted {
		return nil
	}
	if v.Deleted {
		return deleteEntity(ctx, c, v)
	}
	var repo, normalized any
	if v.GetTask() != nil {
		repo = v.GetTask().RepositoryId
		var canonical string
		if e := c.QueryRowContext(ctx, "SELECT canonical_id FROM repository_mappings WHERE local_id=?", repo).Scan(&canonical); e == nil {
			repo = canonical
		} else if !errors.Is(e, sql.ErrNoRows) {
			return e
		}
	}
	if v.GetRepository() != nil {
		normalized = v.GetRepository().NormalizedUrl
	}
	var mapped int
	if err = c.QueryRowContext(ctx, "SELECT count(*) FROM repository_mappings WHERE local_id=?", v.EntityId).Scan(&mapped); err != nil {
		return err
	}
	if mapped > 0 {
		normalized = nil
	}
	if _, err = c.ExecContext(ctx, "INSERT OR IGNORE INTO entities(id,kind,repository_id,normalized_url) VALUES(?,?,?,?)", v.EntityId, int(v.Kind), repo, normalized); err != nil {
		return err
	}
	versions := append([]*lanev1.EntityVersion{v}, r.Alternatives...)
	for _, version := range versions {
		if err = validateVersion(version); err != nil {
			return err
		}
		if version.EntityId != v.EntityId || version.Kind != v.Kind || version.Deleted {
			return invalid("invalid alternative")
		}
		if err = insertVersion(ctx, c, version); err != nil {
			return err
		}
		if _, err = c.ExecContext(ctx, "INSERT OR IGNORE INTO heads(entity_id,version_id) VALUES(?,?)", v.EntityId, version.VersionId); err != nil {
			return err
		}
	}
	// Remove heads that are ancestors of any other head. Arrival order and clocks
	// cannot select a winner. Traverse full history because local edits may race RPCs.
	rows, err := c.QueryContext(ctx, "SELECT version_id,envelope_json FROM versions WHERE entity_id=?", v.EntityId)
	if err != nil {
		return err
	}
	all := map[string]*lanev1.EntityVersion{}
	for rows.Next() {
		var id, body string
		if err = rows.Scan(&id, &body); err != nil {
			rows.Close()
			return err
		}
		vv := new(lanev1.EntityVersion)
		if err = protojson.Unmarshal([]byte(body), vv); err != nil {
			rows.Close()
			return err
		}
		all[id] = vv
	}
	err = rows.Err()
	rows.Close()
	if err != nil {
		return err
	}
	heads, err := stringRows(ctx, c, "SELECT version_id FROM heads WHERE entity_id=?", v.EntityId)
	if err != nil {
		return err
	}
	ancestors := map[string]bool{}
	var visit func(string)
	visit = func(id string) {
		if ancestors[id] {
			return
		}
		ancestors[id] = true
		for _, p := range all[id].GetParentVersionIds() {
			visit(p)
		}
	}
	for _, h := range heads {
		for _, p := range all[h].GetParentVersionIds() {
			visit(p)
		}
	}
	for _, h := range heads {
		if ancestors[h] {
			if _, err = c.ExecContext(ctx, "DELETE FROM heads WHERE entity_id=? AND version_id=?", v.EntityId, h); err != nil {
				return err
			}
		}
	}
	// The accepted server head supplies the projection only when still a head.
	var isHead int
	if err = c.QueryRowContext(ctx, "SELECT count(*) FROM heads WHERE entity_id=? AND version_id=?", v.EntityId, v.VersionId).Scan(&isHead); err != nil {
		return err
	}
	if isHead != 0 {
		if _, err = c.ExecContext(ctx, "UPDATE entities SET repository_id=?,normalized_url=? WHERE id=?", repo, normalized, v.EntityId); err != nil {
			return err
		}
	}
	_, err = c.ExecContext(ctx, "INSERT INTO changes(entity_id,change_kind) VALUES(?,1)", v.EntityId)
	return err
}
func validateVersion(v *lanev1.EntityVersion) error {
	if v == nil || v.FormatVersion != 1 {
		return ErrUnsupportedFormat
	}
	if v.EntityId == "" || v.VersionId == "" || (v.Kind != lanev1.EntityKind_ENTITY_KIND_TASK && v.Kind != lanev1.EntityKind_ENTITY_KIND_REPOSITORY) {
		return invalid("entity identity")
	}
	if v.Deleted {
		if v.Body != nil {
			return invalid("deleted body")
		}
		return nil
	}
	if v.CreatedAt == nil || v.CreatedAt.CheckValid() != nil {
		return invalid("entity timestamp")
	}
	if (v.Kind == lanev1.EntityKind_ENTITY_KIND_TASK && v.GetTask() == nil) || (v.Kind == lanev1.EntityKind_ENTITY_KIND_REPOSITORY && v.GetRepository() == nil) {
		return invalid("entity body")
	}
	return nil
}
func insertVersion(ctx context.Context, c *sql.Conn, v *lanev1.EntityVersion) error {
	body, err := protojson.Marshal(v)
	if err != nil {
		return err
	}
	var old string
	err = c.QueryRowContext(ctx, "SELECT envelope_json FROM versions WHERE version_id=?", v.VersionId).Scan(&old)
	if err == nil {
		previous := new(lanev1.EntityVersion)
		if protojson.Unmarshal([]byte(old), previous) != nil || !proto.Equal(previous, v) {
			return invalid("immutable version changed")
		}
		return nil
	}
	if !errors.Is(err, sql.ErrNoRows) {
		return err
	}
	prompt := 0
	if task := v.GetTask(); task != nil {
		if len(v.ParentVersionIds) == 0 {
			if task.Prompt != nil {
				prompt = 1
			}
		} else {
			for _, parent := range v.ParentVersionIds {
				var raw string
				if e := c.QueryRowContext(ctx, "SELECT envelope_json FROM versions WHERE version_id=? AND entity_id=?", parent, v.EntityId).Scan(&raw); e != nil {
					if errors.Is(e, sql.ErrNoRows) {
						prompt = 1
						continue
					}
					return e
				}
				p := new(lanev1.EntityVersion)
				if e := protojson.Unmarshal([]byte(raw), p); e != nil {
					return e
				}
				pt := p.GetTask()
				if pt == nil || (pt.Prompt == nil) != (task.Prompt == nil) || pt.GetPrompt() != task.GetPrompt() {
					prompt = 1
				}
			}
		}
	}
	created := int64(0)
	if v.CreatedAt != nil {
		created = v.CreatedAt.AsTime().UnixMicro()
	}
	_, err = c.ExecContext(ctx, "INSERT INTO versions(version_id,entity_id,envelope_json,created_at_us,prompt_version) VALUES(?,?,?,?,?)", v.VersionId, v.EntityId, string(body), created, prompt)
	return err
}
func stringRows(ctx context.Context, c *sql.Conn, query string, args ...any) ([]string, error) {
	rows, err := c.QueryContext(ctx, query, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []string
	for rows.Next() {
		var value string
		if err = rows.Scan(&value); err != nil {
			return nil, err
		}
		out = append(out, value)
	}
	return out, rows.Err()
}
func deleteEntity(ctx context.Context, c *sql.Conn, v *lanev1.EntityVersion) error {
	if v.Kind == lanev1.EntityKind_ENTITY_KIND_REPOSITORY {
		ids, err := stringRows(ctx, c, "SELECT id FROM entities WHERE kind=2 AND repository_id=? AND deleted=0", v.EntityId)
		if err != nil {
			return err
		}
		for _, id := range ids {
			deletionID, e := uuid.NewV7()
			if e != nil {
				return e
			}
			tomb := &lanev1.EntityVersion{FormatVersion: 1, EntityId: id, Kind: lanev1.EntityKind_ENTITY_KIND_TASK, VersionId: deletionID.String(), Deleted: true}
			if err = deleteEntity(ctx, c, tomb); err != nil {
				return err
			}
		}
		if err = deleteRemoteChildren(ctx, c, 0, v.EntityId); err != nil {
			return err
		}
	}
	statements := []struct {
		q    string
		args []any
	}{
		{"INSERT INTO entities(id,kind,deleted) VALUES(?,?,1) ON CONFLICT(id) DO UPDATE SET deleted=1,repository_id=NULL,normalized_url=NULL", []any{v.EntityId, int(v.Kind)}},
		{"DELETE FROM heads WHERE entity_id=?", []any{v.EntityId}},
		{"DELETE FROM versions WHERE entity_id=?", []any{v.EntityId}},
		{"DELETE FROM outbox WHERE entity_id=?", []any{v.EntityId}},
		{"UPDATE receipts SET result_json=? WHERE entity_id=?", []any{`{"result":"RESULT_KIND_DELETED"}`, v.EntityId}},
		{"UPDATE notification_receipts SET state=3 WHERE proposal_id=?", []any{v.EntityId}},
	}
	for _, s := range statements {
		if _, err := c.ExecContext(ctx, s.q, s.args...); err != nil {
			return err
		}
	}
	if err := insertVersion(ctx, c, v); err != nil {
		return err
	}
	if _, err := c.ExecContext(ctx, "INSERT INTO heads(entity_id,version_id) VALUES(?,?)", v.EntityId, v.VersionId); err != nil {
		return err
	}
	_, err := c.ExecContext(ctx, "INSERT INTO changes(entity_id,change_kind) VALUES(?,1)", v.EntityId)
	return err
}
func applyChange(ctx context.Context, c *sql.Conn, change *lanev1.Change) error {
	if change == nil {
		return ErrInvalidResponse
	}
	switch v := change.Change.(type) {
	case *lanev1.Change_Entity:
		return ApplySyncResult(ctx, c, v.Entity)
	case *lanev1.Change_Receiver:
		if v.Receiver == nil {
			return ErrInvalidResponse
		}
		if v.Receiver.Deleted {
			return deleteRemote(ctx, c, 1, v.Receiver.Id)
		}
		return putRemote(ctx, c, 1, v.Receiver.Id, v.Receiver)
	case *lanev1.Change_Event:
		if v.Event == nil {
			return ErrInvalidResponse
		}
		if v.Event.State == lanev1.EventState_EVENT_STATE_DELETED {
			return deleteRemote(ctx, c, 2, v.Event.Id)
		}
		return putRemote(ctx, c, 2, v.Event.Id, v.Event)
	case *lanev1.Change_Processing:
		if v.Processing == nil {
			return ErrInvalidResponse
		}
		return putRemote(ctx, c, 3, v.Processing.Id, v.Processing)
	case *lanev1.Change_Deletion:
		d := v.Deletion
		if d == nil {
			return ErrInvalidResponse
		}
		switch d.Kind {
		case lanev1.EntityKindOrRemoteKind_ENTITY_KIND_OR_REMOTE_KIND_REPOSITORY, lanev1.EntityKindOrRemoteKind_ENTITY_KIND_OR_REMOTE_KIND_TASK:
			return deleteEntity(ctx, c, &lanev1.EntityVersion{FormatVersion: 1, EntityId: d.Id, VersionId: d.DeletionVersionId, Kind: lanev1.EntityKind(d.Kind), Deleted: true})
		case lanev1.EntityKindOrRemoteKind_ENTITY_KIND_OR_REMOTE_KIND_RECEIVER:
			return deleteRemote(ctx, c, 1, d.Id)
		case lanev1.EntityKindOrRemoteKind_ENTITY_KIND_OR_REMOTE_KIND_EVENT:
			return deleteRemote(ctx, c, 2, d.Id)
		case lanev1.EntityKindOrRemoteKind_ENTITY_KIND_OR_REMOTE_KIND_ACCOUNT:
			for _, q := range []string{"DELETE FROM heads", "DELETE FROM versions", "DELETE FROM entities", "DELETE FROM outbox", "DELETE FROM remote_objects", "DELETE FROM receipts", "DELETE FROM repository_mappings", "DELETE FROM notification_receipts"} {
				if _, err := c.ExecContext(ctx, q); err != nil {
					return err
				}
			}
			return putMeta(ctx, c, "account_deleted", "1")
		}
	}
	return ErrInvalidResponse
}
func putRemote(ctx context.Context, c *sql.Conn, kind int, id string, message proto.Message) error {
	if id == "" {
		return ErrInvalidResponse
	}
	raw, err := protojson.Marshal(message)
	if err != nil {
		return err
	}
	_, err = c.ExecContext(ctx, "INSERT INTO remote_objects(kind,id,body_json,deleted) VALUES(?,?,?,0) ON CONFLICT(kind,id) DO UPDATE SET body_json=excluded.body_json WHERE remote_objects.deleted=0", kind, id, string(raw))
	if err != nil {
		return err
	}
	_, err = c.ExecContext(ctx, "INSERT INTO changes(entity_id,change_kind) VALUES(?,2)", id)
	return err
}
func deleteRemote(ctx context.Context, c *sql.Conn, kind int, id string) error {
	if kind == 2 {
		if err := deleteRemoteChildren(ctx, c, 3, id); err != nil {
			return err
		}
	}
	_, err := c.ExecContext(ctx, "INSERT INTO remote_objects(kind,id,body_json,deleted) VALUES(?,?,NULL,1) ON CONFLICT(kind,id) DO UPDATE SET body_json=NULL,deleted=1", kind, id)
	return err
}
func deleteRemoteChildren(ctx context.Context, c *sql.Conn, kind int, parent string) error {
	rows, err := c.QueryContext(ctx, "SELECT kind,id,body_json FROM remote_objects WHERE deleted=0")
	if err != nil {
		return err
	}
	type key struct {
		kind int
		id   string
	}
	var victims []key
	for rows.Next() {
		var k int
		var id, raw string
		if err = rows.Scan(&k, &id, &raw); err != nil {
			rows.Close()
			return err
		}
		match := false
		switch k {
		case 1:
			r := new(lanev1.WebhookReceiver)
			if protojson.Unmarshal([]byte(raw), r) != nil {
				rows.Close()
				return ErrInvalidResponse
			}
			match = kind == 0 && r.RepositoryId == parent
		case 2:
			e := new(lanev1.WebhookEvent)
			if protojson.Unmarshal([]byte(raw), e) != nil {
				rows.Close()
				return ErrInvalidResponse
			}
			match = kind == 0 && e.RepositoryId == parent
		case 3:
			p := new(lanev1.ProcessingRecord)
			if protojson.Unmarshal([]byte(raw), p) != nil {
				rows.Close()
				return ErrInvalidResponse
			}
			match = kind == 3 && p.EventId == parent
		}
		if match {
			victims = append(victims, key{k, id})
		}
	}
	err = rows.Err()
	rows.Close()
	if err != nil {
		return err
	}
	for _, v := range victims {
		if err = deleteRemote(ctx, c, v.kind, v.id); err != nil {
			return err
		}
	}
	return nil
}
