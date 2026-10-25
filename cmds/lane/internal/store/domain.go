// SPDX-License-Identifier: Apache-2.0
package store

import (
	"context"
	"database/sql"
	"errors"
	lane "github.com/delinoio/apps/protos/lane/v1"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/timestamppb"
	"log/slog"
	"reflect"
	"slices"
	"strings"
	"time"
)

func heads(ctx context.Context, c *sql.Conn, id string) ([]*lane.EntityVersion, error) {
	rows, e := c.QueryContext(ctx, "SELECT v.envelope_json FROM heads h JOIN versions v ON v.version_id=h.version_id AND v.entity_id=h.entity_id WHERE h.entity_id=? ORDER BY v.version_id", id)
	if e != nil {
		return nil, e
	}
	defer rows.Close()
	var out []*lane.EntityVersion
	for rows.Next() {
		var b string
		if e = rows.Scan(&b); e != nil {
			return nil, e
		}
		v := new(lane.EntityVersion)
		if e = protojson.Unmarshal([]byte(b), v); e != nil {
			return nil, e
		}

		if v.FormatVersion != 1 {
			return nil, domainError(8, "Unsupported entity format", id)
		}
		if v.EntityId != id || !validID(v.VersionId) || v.Kind < 1 || v.Kind > 2 || v.CreatedAt == nil || v.CreatedAt.CheckValid() != nil {
			return nil, domainError(1, "Invalid stored version", id)
		}
		if !v.Deleted {
			if v.Kind == 1 && v.GetRepository() == nil || v.Kind == 2 && v.GetTask() == nil {
				return nil, domainError(1, "Invalid stored body", id)
			}
			if t := v.GetTask(); t != nil && (t.Progress < 1 || t.Progress > 5 || t.Proposal < 1 || t.Proposal > 4) {
				return nil, domainError(1, "Invalid stored task", id)
			}
		}
		out = append(out, v)
	}
	if e = rows.Err(); e != nil {
		return nil, e
	}
	if len(out) == 0 {
		return nil, domainError(2, "Entity does not exist", id)
	}
	return out, nil
}
func result(v []*lane.EntityVersion) *lane.EntityResult {
	r := &lane.EntityResult{Result: lane.ResultKind_RESULT_KIND_APPLIED, Version: v[0]}
	if v[0].Deleted {
		r.Result = lane.ResultKind_RESULT_KIND_DELETED
	} else if len(v) > 1 {
		r.Result = lane.ResultKind_RESULT_KIND_CONFLICT
		r.Alternatives = v
		r.Version = nil
	}
	return r
}
func headIDs(v []*lane.EntityVersion) []string {
	ids := make([]string, len(v))
	for i, x := range v {
		ids[i] = x.VersionId
	}
	slices.Sort(ids)
	return ids
}
func canonicalRepo(ctx context.Context, c *sql.Conn, id string) (string, error) {
	seen := map[string]bool{}
	for !seen[id] {
		seen[id] = true
		var next string
		e := c.QueryRowContext(ctx, "SELECT canonical_id FROM repository_mappings WHERE local_id=?", id).Scan(&next)
		if errors.Is(e, sql.ErrNoRows) {
			return id, nil
		}
		if e != nil {
			return "", e
		}
		id = next
	}
	return "", domainError(1, "Invalid repository mapping", id)
}
func validateTask(ctx context.Context, c *sql.Conn, t *lane.Task) error {
	if t == nil || strings.TrimSpace(t.Title) == "" || t.Progress < 1 || t.Progress > 5 || t.Proposal < 1 || t.Proposal > 4 || !validID(t.RepositoryId) {
		return domainError(1, "Invalid task snapshot", "")
	}
	id, e := canonicalRepo(ctx, c, t.RepositoryId)
	if e != nil {
		return e
	}
	h, e := heads(ctx, c, id)
	if e != nil {
		return e
	}
	if h[0].Kind != 1 || h[0].Deleted {
		return domainError(7, "Repository is deleted or invalid", id)
	}
	if len(h) > 1 {
		return domainError(4, "Repository has unresolved conflicts", id)
	}
	return nil
}
func reserveURL(ctx context.Context, c *sql.Conn, id, url string) error {
	rows, e := c.QueryContext(ctx, "SELECT v.envelope_json FROM heads h JOIN versions v ON h.version_id=v.version_id JOIN entities e ON e.id=h.entity_id WHERE e.kind=1 AND e.deleted=0 AND e.id<>?", id)
	if e != nil {
		return e
	}
	defer rows.Close()
	for rows.Next() {
		var b string
		if e = rows.Scan(&b); e != nil {
			return e
		}
		v := new(lane.EntityVersion)
		if e = protojson.Unmarshal([]byte(b), v); e != nil {
			return e
		}
		if v.GetRepository().GetNormalizedUrl() == url {
			return domainError(1, "Repository URL is already registered", id)
		}
	}
	return rows.Err()
}
func newVersion(id string, kind lane.EntityKind, meta *lane.MutationMeta, parents []string) *lane.EntityVersion {
	return &lane.EntityVersion{FormatVersion: 1, EntityId: id, Kind: kind, VersionId: NewID(), ParentVersionIds: parents, CreatedAt: timestamppb.New(time.Now().UTC().Truncate(time.Microsecond)), Origin: proto.Clone(meta.Origin).(*lane.Origin)}
}
func (s *Store) persist(ctx context.Context, c *sql.Conn, v *lane.EntityVersion, meta *lane.MutationMeta, kind lane.MutationKind, conflict, prompt bool) error {
	var rid, url any
	if t := v.GetTask(); t != nil {
		var e error
		rid, e = canonicalRepo(ctx, c, t.RepositoryId)
		if e != nil {
			return e
		}
	}
	if r := v.GetRepository(); r != nil {
		url = r.NormalizedUrl
		if e := reserveURL(ctx, c, v.EntityId, r.NormalizedUrl); e != nil {
			return e
		}
	}
	if _, e := c.ExecContext(ctx, "INSERT OR IGNORE INTO entities(id,kind) VALUES(?,?)", v.EntityId, v.Kind); e != nil {
		return e
	}
	if !conflict {
		if _, e := c.ExecContext(ctx, "UPDATE entities SET repository_id=?,normalized_url=?,deleted=? WHERE id=?", rid, url, v.Deleted, v.EntityId); e != nil {
			return e
		}
		if _, e := c.ExecContext(ctx, "DELETE FROM heads WHERE entity_id=?", v.EntityId); e != nil {
			return e
		}
	}
	if _, e := c.ExecContext(ctx, "INSERT INTO versions(version_id,entity_id,envelope_json,created_at_us,prompt_version) VALUES(?,?,?,?,?)", v.VersionId, v.EntityId, marshal(v), v.CreatedAt.AsTime().UnixMicro(), prompt); e != nil {
		return e
	}
	if _, e := c.ExecContext(ctx, "INSERT INTO heads(entity_id,version_id) VALUES(?,?)", v.EntityId, v.VersionId); e != nil {
		return e
	}
	m := &lane.SyncMutation{Meta: proto.Clone(meta).(*lane.MutationMeta), Version: v, ExpectedHeadIds: v.ParentVersionIds, Kind: kind}
	m.Meta.RequestId = v.VersionId
	if _, e := c.ExecContext(ctx, "INSERT INTO outbox(request_id,entity_id,request_json,created_at_us) VALUES(?,?,?,?)", m.Meta.RequestId, v.EntityId, marshal(m), v.CreatedAt.AsTime().UnixMicro()); e != nil {
		return e
	}
	if t := v.GetTask(); t != nil {
		if kind == 1 && t.Proposal == 2 && meta.Origin.Kind == 3 {
			if _, e := c.ExecContext(ctx, "INSERT OR IGNORE INTO notification_receipts(proposal_id,device_id,state) VALUES(?,?,1)", v.EntityId, s.device); e != nil {
				return e
			}
		}
		if t.Proposal == 3 || t.Proposal == 4 {
			if _, e := c.ExecContext(ctx, "UPDATE notification_receipts SET state=3 WHERE proposal_id=? AND state=1", v.EntityId); e != nil {
				return e
			}
		}
	}
	_, e := c.ExecContext(ctx, "INSERT INTO changes(entity_id,change_kind) VALUES(?,1)", v.EntityId)
	return e
}
func (s *Store) erase(ctx context.Context, c *sql.Conn, id string, meta *lane.MutationMeta) (*lane.EntityResult, error) {
	h, e := heads(ctx, c, id)
	if e != nil {
		return nil, e
	}
	if h[0].Deleted {
		return result(h), nil
	}
	if h[0].Kind == 1 {
		rows, e := c.QueryContext(ctx, "SELECT id FROM entities WHERE repository_id=? AND deleted=0", id)
		if e != nil {
			return nil, e
		}
		var ids []string
		for rows.Next() {
			var x string
			if e = rows.Scan(&x); e != nil {
				rows.Close()
				return nil, e
			}
			ids = append(ids, x)
		}
		e = rows.Err()
		rows.Close()
		if e != nil {
			return nil, e
		}
		for _, x := range ids {
			if _, e = s.erase(ctx, c, x, meta); e != nil {
				return nil, e
			}
		}
	}
	v := newVersion(id, h[0].Kind, meta, headIDs(h))
	v.Deleted = true
	for _, q := range []string{"DELETE FROM heads WHERE entity_id=?", "DELETE FROM versions WHERE entity_id=?", "DELETE FROM outbox WHERE entity_id=?", "UPDATE notification_receipts SET state=3 WHERE proposal_id=? AND state=1"} {
		if _, e = c.ExecContext(ctx, q, id); e != nil {
			return nil, e
		}
	}
	if e = s.persist(ctx, c, v, meta, 4, false, false); e != nil {
		return nil, e
	}
	r := result([]*lane.EntityVersion{v})
	// Receipts retain fingerprints and a bodyless identity after permanent deletion.
	if _, e = c.ExecContext(ctx, "UPDATE receipts SET result_json=? WHERE entity_id=?", marshal(r), id); e != nil {
		return nil, e
	}
	if e = redactCaches(ctx, c, id, h[0].Kind); e != nil {
		return nil, e
	}
	return r, nil
}
func redactCaches(ctx context.Context, c *sql.Conn, id string, kind lane.EntityKind) error {
	rows, e := c.QueryContext(ctx, "SELECT kind,id,body_json FROM remote_objects WHERE deleted=0")
	if e != nil {
		return e
	}
	type item struct {
		k     int
		id, b string
	}
	var items []item
	for rows.Next() {
		var x item
		if e = rows.Scan(&x.k, &x.id, &x.b); e != nil {
			rows.Close()
			return e
		}
		items = append(items, x)
	}
	e = rows.Err()
	rows.Close()
	if e != nil {
		return e
	}
	events := map[string]bool{}

	if kind == lane.EntityKind_ENTITY_KIND_TASK {
		for _, x := range items {
			if x.k == 3 {
				p := new(lane.ProcessingRecord)
				if e = protojson.Unmarshal([]byte(x.b), p); e != nil {
					return e
				}
				if slices.Contains(p.TaskIds, id) {
					events[p.EventId] = true
				}
			}
		}
	}
	for _, x := range items {
		if x.k == 2 {
			v := new(lane.WebhookEvent)
			if e = protojson.Unmarshal([]byte(x.b), v); e != nil {
				return e
			}
			if kind == 1 && v.RepositoryId == id {
				events[x.id] = true
			}
		}
	}
	for _, x := range items {
		del := events[x.id]
		if x.k == 3 {
			v := new(lane.ProcessingRecord)
			if e = protojson.Unmarshal([]byte(x.b), v); e != nil {
				return e
			}
			del = events[v.EventId]
		}
		if del {
			if _, e = c.ExecContext(ctx, "UPDATE remote_objects SET body_json=NULL,deleted=1 WHERE kind=? AND id=?", x.k, x.id); e != nil {
				return e
			}
		}
	}
	return nil
}
func getMeta(m proto.Message) *lane.MutationMeta {
	v := reflect.ValueOf(m).Elem().FieldByName("Meta")
	if !v.IsValid() || v.IsNil() {
		return nil
	}
	return v.Interface().(*lane.MutationMeta)
}
func getID(m proto.Message) string {
	v := reflect.ValueOf(m).Elem().FieldByName("Id")
	if v.IsValid() {
		return v.String()
	}
	return ""
}
func expected(m proto.Message) string {
	v := reflect.ValueOf(m).Elem().FieldByName("ExpectedVersionId")
	if v.IsValid() {
		return v.String()
	}
	return ""
}
func normalizeRequest(m proto.Message) error {
	switch r := m.(type) {
	case *lane.CreateRepositoryRequest:
		u, e := NormalizeURL(r.Url)
		if e != nil {
			return e
		}
		r.Url = u
	case *lane.SaveRepositoryRequest:
		u, e := NormalizeURL(r.Url)
		if e != nil {
			return e
		}
		r.Url = u
	case *lane.CreateTaskRequest:
		if r.Progress == 0 {
			r.Progress = 1
		}
		if r.Proposal == 0 {
			r.Proposal = 1
		}
		if r.Repository != nil && r.Repository.GetUrl() != "" {
			u, e := NormalizeURL(r.Repository.GetUrl())
			if e != nil {
				return e
			}
			r.Repository.Target = &lane.RepositoryTarget_Url{Url: u}
		}
	case *lane.ResolveConflictRequest:
		slices.Sort(r.ExpectedHeadIds)
	}
	return nil
}
func (s *Store) mutate(ctx context.Context, method string, input proto.Message) (r, repo *lane.EntityResult, err error) {

	started := time.Now()
	defer func() {
		slog.DebugContext(ctx, "local domain operation", "method", method, "success", err == nil, "duration_ms", time.Since(started).Milliseconds())
	}()
	if input == nil || !input.ProtoReflect().IsValid() {
		err = domainError(1, "Request is required", "")
		return
	}
	m := proto.Clone(input)
	if err = normalizeRequest(m); err != nil {
		return
	}
	meta := getMeta(m)
	if meta == nil || !validID(meta.RequestId) || meta.Origin == nil || meta.Origin.Kind < 1 || meta.Origin.Kind > 3 || meta.Origin.DeviceId != s.device {
		err = domainError(1, "Valid request and local origin identities are required", "")
		return
	}
	fp, e := fingerprint(m)
	if e != nil {
		err = e
		return
	}
	var committedError error
	err = s.WithWrite(ctx, func(c *sql.Conn) error {

		identity, e := s.Identity(ctx)
		if e != nil {
			return e
		}
		if meta.Origin.AccountId != identity.AccountID {
			return domainError(1, "Origin account does not match selected space", "")
		}
		var old, b string
		e = c.QueryRowContext(ctx, "SELECT fingerprint,result_json FROM receipts WHERE method=? AND request_id=?", method, meta.RequestId).Scan(&old, &b)
		if e == nil {
			if old != fp {
				return domainError(6, "Request ID was reused with different content", getID(m))
			}
			r = new(lane.EntityResult)
			if e = protojson.Unmarshal([]byte(b), r); e != nil {
				return e
			}

			if r.Result == lane.ResultKind_RESULT_KIND_CONFLICT {
				committedError = domainError(3, "Version changed; submitted alternative was preserved", getID(m), headIDs(r.Alternatives)...)
			} else if r.Result != lane.ResultKind_RESULT_KIND_DELETED {
				r.Result = lane.ResultKind_RESULT_KIND_REPLAYED
			}
			if method == "CreateTask" && r.Version.GetTask() != nil {
				rh, e := heads(ctx, c, r.Version.GetTask().RepositoryId)
				if e == nil {
					repo = result(rh)
				}
			}
			return nil
		}
		if !errors.Is(e, sql.ErrNoRows) {
			return e
		}
		id := getID(m)
		if id == "" && (method == "CreateTask" || method == "CreateRepository") {
			id = NewID()
		}
		if !validID(id) {
			return domainError(1, "Invalid entity ID", id)
		}
		create := method == "CreateTask" || method == "CreateRepository"
		var h []*lane.EntityVersion
		if !create {
			h, e = heads(ctx, c, id)
			if e != nil {
				return e
			}
			if h[0].Deleted {
				r = result(h)
				return saveReceipt(ctx, c, method, meta.RequestId, fp, id, r)
			}
		} else {
			var n int
			if e = c.QueryRowContext(ctx, "SELECT count(*) FROM entities WHERE id=?", id).Scan(&n); e != nil {
				return e
			}

			if n != 0 {
				h, e = heads(ctx, c, id)
				if e != nil {
					return e
				}
				if h[0].Deleted {
					r = result(h)
					return saveReceipt(ctx, c, method, meta.RequestId, fp, id, r)
				}
				return domainError(1, "Entity ID already exists", id)
			}
		}
		if method == "DeleteTask" || method == "DeleteRepository" {
			want := lane.EntityKind_ENTITY_KIND_TASK
			if method == "DeleteRepository" {
				want = 1
			}
			if h[0].Kind != want {
				return domainError(1, "Wrong entity kind", id)
			}
			r, e = s.erase(ctx, c, id, meta)
			if e != nil {
				return e
			}
			return saveReceipt(ctx, c, method, meta.RequestId, fp, id, r)
		}
		if !create && len(h) > 1 && method != "ResolveConflict" {
			return domainError(4, "Resolve the conflict before editing", id, headIDs(h)...)
		}
		var v *lane.EntityVersion
		kind := lane.MutationKind_MUTATION_KIND_SAVE
		conflict, prompt := false, false
		if create {
			kind = 1
			v = newVersion(id, 1, meta, nil)
		} else {
			v = newVersion(id, h[0].Kind, meta, []string{expected(m)})
		}
		switch x := m.(type) {
		case *lane.CreateRepositoryRequest:
			v.Body = &lane.EntityVersion_Repository{Repository: &lane.Repository{NormalizedUrl: x.Url, DisplayName: x.DisplayName}}
		case *lane.SaveRepositoryRequest:
			if h[0].Kind != 1 {
				return domainError(1, "Wrong entity kind", id)
			}
			v.Body = &lane.EntityVersion_Repository{Repository: &lane.Repository{NormalizedUrl: x.Url, DisplayName: x.DisplayName}}
		case *lane.CreateTaskRequest:
			if x.Repository == nil {
				return domainError(1, "Repository is required", id)
			}
			rid := x.Repository.GetId()
			if u := x.Repository.GetUrl(); u != "" {
				e = c.QueryRowContext(ctx, "SELECT id FROM entities WHERE kind=1 AND normalized_url=? AND deleted=0", u).Scan(&rid)
				if errors.Is(e, sql.ErrNoRows) {
					rid = NewID()
					rv := newVersion(rid, 1, meta, nil)
					rv.Body = &lane.EntityVersion_Repository{Repository: &lane.Repository{NormalizedUrl: u}}
					if e = s.persist(ctx, c, rv, meta, 1, false, false); e != nil {
						return e
					}
					repo = result([]*lane.EntityVersion{rv})
				} else if e != nil {
					return e
				} else {
					rh, e := heads(ctx, c, rid)
					if e != nil {
						return e
					}
					repo = result(rh)
				}
			}
			if x.Proposal != 1 && x.Proposal != 2 {
				return domainError(1, "Creation requires regular or pending proposal", id)
			}
			v.Kind = 2
			v.Body = &lane.EntityVersion_Task{Task: &lane.Task{RepositoryId: rid, Title: x.Title, Description: x.Description, Prompt: x.Prompt, Progress: x.Progress, Proposal: x.Proposal}}
			prompt = x.Prompt != nil
		case *lane.SaveTaskRequest:
			if h[0].GetTask() == nil || x.Task == nil {
				return domainError(1, "Task is required", id)
			}
			if x.Task.Proposal != h[0].GetTask().Proposal {
				return domainError(1, "Use proposal disposition operation", id)
			}
			v.Body = &lane.EntityVersion_Task{Task: proto.Clone(x.Task).(*lane.Task)}
		case *lane.SetProgressRequest:
			if h[0].GetTask() == nil {
				return domainError(1, "Wrong entity kind", id)
			}
			t := proto.Clone(h[0].GetTask()).(*lane.Task)
			t.Progress = x.Progress
			v.Body = &lane.EntityVersion_Task{Task: t}
		case *lane.SetProposalRequest:
			if h[0].GetTask() == nil {
				return domainError(1, "Wrong entity kind", id)
			}
			t := proto.Clone(h[0].GetTask()).(*lane.Task)
			if (t.Proposal != 2 && t.Proposal != 4) || (x.Proposal != 3 && x.Proposal != 4) {
				return domainError(1, "Invalid proposal disposition", id)
			}
			t.Proposal = x.Proposal
			v.Body = &lane.EntityVersion_Task{Task: t}
		case *lane.RestorePromptRequest:
			v.PromptRestoration = true
			if h[0].GetTask() == nil {
				return domainError(1, "Wrong entity kind", id)
			}
			var b string
			if e = c.QueryRowContext(ctx, "SELECT envelope_json FROM versions WHERE entity_id=? AND version_id=?", id, x.SourceVersionId).Scan(&b); e != nil {
				return domainError(2, "Prompt source does not exist", id)
			}
			src := new(lane.EntityVersion)
			if e = protojson.Unmarshal([]byte(b), src); e != nil {
				return e
			}
			if src.GetTask() == nil {
				return domainError(1, "Invalid prompt source", id)
			}
			t := proto.Clone(h[0].GetTask()).(*lane.Task)
			t.Prompt = src.GetTask().Prompt
			v.Body = &lane.EntityVersion_Task{Task: t}
			prompt = true
		case *lane.ResolveConflictRequest:
			if len(h) < 2 || !slices.Equal(headIDs(h), x.ExpectedHeadIds) {
				return domainError(5, "Conflict heads changed", id, headIDs(h)...)
			}
			if x.Resolved == nil || x.Resolved.Kind != h[0].Kind || x.Resolved.Deleted || x.Resolved.EntityId != id || x.Resolved.FormatVersion != 1 {
				return domainError(1, "Invalid resolved snapshot", id)
			}
			v.Body = proto.Clone(x.Resolved).(*lane.EntityVersion).Body
			v.ParentVersionIds = headIDs(h)
			kind = 3
		default:
			return domainError(1, "Unknown mutation", id)
		}
		if !create && method != "ResolveConflict" {
			ex := expected(m)
			if !validID(ex) {
				return domainError(1, "Expected version is required", id)
			}
			if ex != h[0].VersionId {
				var n int
				if e = c.QueryRowContext(ctx, "SELECT count(*) FROM versions WHERE entity_id=? AND version_id=?", id, ex).Scan(&n); e != nil {
					return e
				}
				if n == 0 {
					return domainError(1, "Expected version does not belong to entity", id)
				}
				conflict = true
			}
		}
		if t := v.GetTask(); t != nil {
			if e = validateTask(ctx, c, t); e != nil {
				return e
			}
			if !create {
				old := h[0].GetTask()
				prompt = prompt || !reflect.DeepEqual(old.Prompt, t.Prompt)
				for _, alternative := range h {
					if alternative.GetTask() != nil && !reflect.DeepEqual(alternative.GetTask().Prompt, t.Prompt) {
						prompt = true
					}
				}
			}

		} else if r := v.GetRepository(); r != nil {
			normalized, e := NormalizeURL("https://" + r.NormalizedUrl + "/lane-validation")
			if e != nil || normalized != r.NormalizedUrl+"/lane-validation" {
				return domainError(1, "Invalid normalized repository URL", id)
			}
		} else if v.GetRepository() == nil {
			return domainError(1, "Snapshot body is missing", id)
		}
		if e = s.persist(ctx, c, v, meta, kind, conflict, prompt); e != nil {
			return e
		}
		all, e := heads(ctx, c, id)
		if e != nil {
			return e
		}
		r = result(all)
		if conflict {
			committedError = domainError(3, "Version changed; submitted alternative was preserved", id, headIDs(all)...)
		}
		return saveReceipt(ctx, c, method, meta.RequestId, fp, id, r)
	})
	if err == nil {
		err = committedError
	}
	return
}
func saveReceipt(ctx context.Context, c *sql.Conn, method, request, fp, id string, r *lane.EntityResult) error {
	_, e := c.ExecContext(ctx, "INSERT INTO receipts(method,request_id,fingerprint,entity_id,result_json) VALUES(?,?,?,?,?)", method, request, fp, id, marshal(r))
	return e
}
