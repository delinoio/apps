// SPDX-License-Identifier: Apache-2.0
package syncclient

import (
	"context"
	"database/sql"
	"errors"
	"path/filepath"

	lanev1 "github.com/delinoio/apps/protos/lane/v1"
	"google.golang.org/protobuf/encoding/protojson"
)

// ImportStandalone copies the explicitly approved standalone dataset into an
// account space. It never imports a retained account space or rewrites origins.
// Source writes are paused only for local SQL; no network or credential work runs
// under either transaction. Repeat calls are idempotent, including after a crash.
func ImportStandalone(ctx context.Context, source, dest Store) error {
	if source.SpaceID() == dest.SpaceID() || source.Root() != dest.Root() || source.DeviceID() != dest.DeviceID() {
		return ErrAccountMismatch
	}
	registry, err := sql.Open("sqlite", filepath.Join(source.Root(), "registry.sqlite")+"?mode=ro")
	if err != nil {
		return err
	}
	defer registry.Close()
	var sourceKey, destKey string
	if err = registry.QueryRowContext(ctx, "SELECT account_key FROM spaces WHERE id=?", source.SpaceID()).Scan(&sourceKey); err != nil {
		return err
	}
	if err = registry.QueryRowContext(ctx, "SELECT account_key FROM spaces WHERE id=?", dest.SpaceID()).Scan(&destKey); err != nil {
		return err
	}
	if sourceKey != "local" || destKey == "local" || destKey == "" {
		return ErrAccountMismatch
	}
	return source.WithWrite(ctx, func(src *sql.Conn) error {
		return dest.WithWrite(ctx, func(dst *sql.Conn) error {
			// Merge URL identities before inserting projections. Immutable histories and
			// transport mutations retain their original IDs and use the mapping on reads.
			rows, err := src.QueryContext(ctx, "SELECT id,normalized_url FROM entities WHERE kind=1 AND deleted=0 AND normalized_url IS NOT NULL")
			if err != nil {
				return err
			}
			var mappings []*lanev1.RepositoryMapping
			for rows.Next() {
				var id, url string
				if err = rows.Scan(&id, &url); err != nil {
					rows.Close()
					return err
				}
				var canonical string
				e := dst.QueryRowContext(ctx, "SELECT id FROM entities WHERE kind=1 AND deleted=0 AND normalized_url=?", url).Scan(&canonical)
				if e != nil && !errors.Is(e, sql.ErrNoRows) {
					rows.Close()
					return e
				}
				if e == nil && canonical != id {
					mappings = append(mappings, &lanev1.RepositoryMapping{LocalId: id, CanonicalId: canonical})
				}
			}
			err = rows.Err()
			rows.Close()
			if err != nil {
				return err
			}
			if err = applyMappings(ctx, dst, mappings); err != nil {
				return err
			}
			rows, err = src.QueryContext(ctx, "SELECT id,kind,repository_id,normalized_url,deleted FROM entities ORDER BY kind,id")
			if err != nil {
				return err
			}
			for rows.Next() {
				var id string
				var kind, deleted int
				var repo, url sql.NullString
				if err = rows.Scan(&id, &kind, &repo, &url, &deleted); err != nil {
					rows.Close()
					return err
				}
				var mapped int
				if err = dst.QueryRowContext(ctx, "SELECT count(*) FROM repository_mappings WHERE local_id=?", id).Scan(&mapped); err != nil {
					rows.Close()
					return err
				}
				if mapped > 0 {
					url = sql.NullString{}
				}
				if _, err = dst.ExecContext(ctx, "INSERT OR IGNORE INTO entities(id,kind,repository_id,normalized_url,deleted) VALUES(?,?,?,?,?)", id, kind, repo, url, deleted); err != nil {
					rows.Close()
					return err
				}
			}
			err = rows.Err()
			rows.Close()
			if err != nil {
				return err
			}
			rows, err = src.QueryContext(ctx, "SELECT envelope_json,prompt_version FROM versions ORDER BY created_at_us,version_id")
			if err != nil {
				return err
			}
			for rows.Next() {
				var body string
				var prompt int
				if err = rows.Scan(&body, &prompt); err != nil {
					rows.Close()
					return err
				}
				v := new(lanev1.EntityVersion)
				if err = protojson.Unmarshal([]byte(body), v); err != nil {
					rows.Close()
					return err
				}
				var deleted int
				if err = dst.QueryRowContext(ctx, "SELECT deleted FROM entities WHERE id=?", v.EntityId).Scan(&deleted); err != nil {
					rows.Close()
					return err
				}
				if deleted != 0 && !v.Deleted {
					continue
				}
				if err = insertVersion(ctx, dst, v); err != nil {
					rows.Close()
					return err
				}
				if prompt != 0 {
					if _, err = dst.ExecContext(ctx, "UPDATE versions SET prompt_version=1 WHERE version_id=?", v.VersionId); err != nil {
						rows.Close()
						return err
					}
				}
			}
			err = rows.Err()
			rows.Close()
			if err != nil {
				return err
			}
			rows, err = src.QueryContext(ctx, "SELECT v.envelope_json FROM heads h JOIN versions v ON v.version_id=h.version_id ORDER BY v.entity_id")
			if err != nil {
				return err
			}
			for rows.Next() {
				var body string
				if err = rows.Scan(&body); err != nil {
					rows.Close()
					return err
				}
				v := new(lanev1.EntityVersion)
				if err = protojson.Unmarshal([]byte(body), v); err != nil {
					rows.Close()
					return err
				}
				if err = ApplySyncResult(ctx, dst, &lanev1.EntityResult{Version: v}); err != nil {
					rows.Close()
					return err
				}
			}
			err = rows.Err()
			rows.Close()
			if err != nil {
				return err
			}
			rows, err = src.QueryContext(ctx, "SELECT method,request_id,fingerprint,entity_id,result_json FROM receipts")
			if err != nil {
				return err
			}
			for rows.Next() {
				var method, id, fingerprint, result string
				var entity sql.NullString
				if err = rows.Scan(&method, &id, &fingerprint, &entity, &result); err != nil {
					rows.Close()
					return err
				}
				var existing string
				e := dst.QueryRowContext(ctx, "SELECT fingerprint FROM receipts WHERE method=? AND request_id=?", method, id).Scan(&existing)
				if e == nil && existing != fingerprint {
					rows.Close()
					return invalid("request ID reused during import")
				}
				if e != nil && !errors.Is(e, sql.ErrNoRows) {
					rows.Close()
					return e
				}
				var deleted int
				if entity.Valid {
					e = dst.QueryRowContext(ctx, "SELECT deleted FROM entities WHERE id=?", entity.String).Scan(&deleted)
					if e != nil && !errors.Is(e, sql.ErrNoRows) {
						rows.Close()
						return e
					}
				}
				if deleted != 0 {
					result = `{"result":"RESULT_KIND_DELETED"}`
				}
				if _, err = dst.ExecContext(ctx, "INSERT OR IGNORE INTO receipts(method,request_id,fingerprint,entity_id,result_json) VALUES(?,?,?,?,?)", method, id, fingerprint, entity, result); err != nil {
					rows.Close()
					return err
				}
			}
			err = rows.Err()
			rows.Close()
			if err != nil {
				return err
			}
			rows, err = src.QueryContext(ctx, "SELECT request_id,entity_id,request_json,state,error_reason,created_at_us FROM outbox ORDER BY created_at_us,rowid")
			if err != nil {
				return err
			}
			for rows.Next() {
				var id, entity, body string
				var state int
				var reason sql.NullString
				var created int64
				if err = rows.Scan(&id, &entity, &body, &state, &reason, &created); err != nil {
					rows.Close()
					return err
				}
				var deleted int
				if err = dst.QueryRowContext(ctx, "SELECT deleted FROM entities WHERE id=?", entity).Scan(&deleted); err != nil {
					rows.Close()
					return err
				}
				m := new(lanev1.SyncMutation)
				if err = protojson.Unmarshal([]byte(body), m); err != nil {
					rows.Close()
					return err
				}
				if deleted != 0 && !m.GetVersion().GetDeleted() {
					continue
				}
				var existing string
				e := dst.QueryRowContext(ctx, "SELECT request_json FROM outbox WHERE request_id=?", id).Scan(&existing)
				if e != nil && !errors.Is(e, sql.ErrNoRows) {
					rows.Close()
					return e
				}
				if e == nil && existing != body {
					rows.Close()
					return invalid("outbox identity reused during import")
				}
				if _, err = dst.ExecContext(ctx, "INSERT OR IGNORE INTO outbox(request_id,entity_id,request_json,state,error_reason,created_at_us) VALUES(?,?,?,?,?,?)", id, entity, body, state, reason, created); err != nil {
					rows.Close()
					return err
				}
			}
			err = rows.Err()
			rows.Close()
			return err
		})
	})
}
