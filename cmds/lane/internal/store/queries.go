// SPDX-License-Identifier: Apache-2.0
package store

import (
	"context"
	"database/sql"
	"encoding/base64"
	"encoding/json"
	lane "github.com/delinoio/apps/protos/lane/v1"
	"golang.org/x/text/cases"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	"slices"
	"strings"
)

type cursor struct{ Scope, Last string }

func pageStart(p *lane.Page, scope string) (int, string, error) {
	n := 100
	if p.GetSize() > 1000 {
		return 0, "", domainError(1, "Page size exceeds 1000", "")
	}
	if p.GetSize() != 0 {
		n = int(p.Size)
	}
	if p.GetCursor() == "" {
		return n, "", nil
	}
	b, e := base64.RawURLEncoding.DecodeString(p.Cursor)
	var c cursor
	if e != nil || json.Unmarshal(b, &c) != nil || c.Scope != scope {
		return 0, "", domainError(1, "Cursor does not match query", "")
	}
	return n, c.Last, nil
}
func nextCursor(scope, last string) string {
	b, _ := json.Marshal(cursor{scope, last})
	return base64.RawURLEncoding.EncodeToString(b)
}
func (s *Store) query(ctx context.Context, method string, m proto.Message) (out proto.Message, err error) {
	err = s.WithRead(ctx, func(c *sql.Conn) error {
		switch r := m.(type) {
		case *lane.GetTaskRequest:
			h, e := heads(ctx, c, r.Id)
			if e != nil {
				return e
			}
			if h[0].Kind != 2 {
				return domainError(2, "Task does not exist", r.Id)
			}
			out = &lane.GetTaskResponse{Result: result(h)}
		case *lane.GetRepositoryRequest:
			id, e := canonicalRepo(ctx, c, r.Id)
			if e != nil {
				return e
			}
			h, e := heads(ctx, c, id)
			if e != nil {
				return e
			}
			if h[0].Kind != 1 {
				return domainError(2, "Repository does not exist", r.Id)
			}
			out = &lane.GetRepositoryResponse{Result: result(h)}
		case *lane.ListRepositoriesRequest:
			items, next, e := s.list(ctx, c, r.Page, m, 1, nil)
			if e != nil {
				return e
			}
			out = &lane.ListRepositoriesResponse{Items: items, NextCursor: next}
		case *lane.ListTasksRequest:
			for _, x := range r.Progress {
				if x < 1 || x > 5 {
					return domainError(1, "Invalid progress filter", "")
				}
			}
			for _, x := range r.Proposal {
				if x < 1 || x > 4 {
					return domainError(1, "Invalid proposal filter", "")
				}
			}
			items, next, e := s.list(ctx, c, r.Page, m, 2, r)
			if e != nil {
				return e
			}
			out = &lane.ListTasksResponse{Items: items, NextCursor: next}
		case *lane.ListHistoryRequest:
			if _, e := heads(ctx, c, r.Id); e != nil {
				return e
			}
			scope := s.space + ":" + method + ":" + r.Id
			if r.PromptsOnly {
				scope += ":prompts"
			}
			n, last, e := pageStart(r.Page, scope)
			if e != nil {
				return e
			}
			rows, e := c.QueryContext(ctx, "SELECT envelope_json,prompt_version FROM versions WHERE entity_id=? ORDER BY created_at_us,version_id", r.Id)
			if e != nil {
				return e
			}
			defer rows.Close()
			resp := &lane.ListHistoryResponse{}
			for rows.Next() {
				var b string
				var p bool
				if e = rows.Scan(&b, &p); e != nil {
					return e
				}
				v := new(lane.EntityVersion)
				if e = protojson.Unmarshal([]byte(b), v); e != nil {
					return e
				}
				key := v.CreatedAt.AsTime().Format("2006-01-02T15:04:05.000000Z") + ":" + v.VersionId
				if key <= last || v.Deleted || (r.PromptsOnly && !p) {
					continue
				}
				if len(resp.Versions) == n {
					prev := resp.Versions[n-1]
					resp.NextCursor = nextCursor(scope, prev.CreatedAt.AsTime().Format("2006-01-02T15:04:05.000000Z")+":"+prev.VersionId)
					break
				}
				resp.Versions = append(resp.Versions, v)
			}
			out = resp
			return rows.Err()
		case *lane.ListEventsRequest:
			copy := proto.Clone(r).(*lane.ListEventsRequest)
			copy.Page = nil
			fp, _ := fingerprint(copy)
			scope := s.space + method + fp
			n, last, e := pageStart(r.Page, scope)
			if e != nil {
				return e
			}
			rows, e := c.QueryContext(ctx, "SELECT body_json FROM remote_objects WHERE kind=2 AND deleted=0 AND id>? ORDER BY id", last)
			if e != nil {
				return e
			}
			defer rows.Close()
			resp := &lane.ListEventsResponse{}
			for rows.Next() {
				var b string
				if e = rows.Scan(&b); e != nil {
					return e
				}
				v := new(lane.WebhookEvent)
				if e = protojson.Unmarshal([]byte(b), v); e != nil {
					return e
				}
				if r.RepositoryId != nil && v.RepositoryId != *r.RepositoryId || len(r.States) > 0 && !slices.Contains(r.States, v.State) {
					continue
				}
				if len(resp.Items) == n {
					resp.NextCursor = nextCursor(scope, resp.Items[n-1].Id)
					break
				}
				resp.Items = append(resp.Items, v)
			}
			out = resp
			return rows.Err()
		case *lane.GetEventRequest:
			var b string
			if e := c.QueryRowContext(ctx, "SELECT body_json FROM remote_objects WHERE kind=2 AND id=? AND deleted=0", r.Id).Scan(&b); e != nil {
				return domainError(2, "Cached event does not exist", r.Id)
			}
			resp := &lane.GetEventResponse{Event: new(lane.WebhookEvent)}
			if e := protojson.Unmarshal([]byte(b), resp.Event); e != nil {
				return e
			}
			rows, e := c.QueryContext(ctx, "SELECT body_json FROM remote_objects WHERE kind=3 AND deleted=0 ORDER BY id")
			if e != nil {
				return e
			}
			defer rows.Close()
			for rows.Next() {
				if e = rows.Scan(&b); e != nil {
					return e
				}
				p := new(lane.ProcessingRecord)
				if e = protojson.Unmarshal([]byte(b), p); e != nil {
					return e
				}
				if p.EventId == r.Id {
					resp.Processing = append(resp.Processing, p)
				}
			}
			out = resp
			return rows.Err()
		default:
			return domainError(11, "This operation requires an explicit online target", "")
		}
		return nil
	})
	return
}
func (s *Store) list(ctx context.Context, c *sql.Conn, p *lane.Page, m proto.Message, kind int, filter *lane.ListTasksRequest) ([]*lane.EntityResult, string, error) {
	copy := proto.Clone(m)
	switch x := copy.(type) {
	case *lane.ListTasksRequest:
		x.Page = nil
	case *lane.ListRepositoriesRequest:
		x.Page = nil
	}
	fp, _ := fingerprint(copy)
	scope := s.space + string(m.ProtoReflect().Descriptor().FullName()) + fp
	n, last, e := pageStart(p, scope)
	if e != nil {
		return nil, "", e
	}
	rows, e := c.QueryContext(ctx, "SELECT id FROM entities WHERE kind=? AND deleted=0 AND id>? ORDER BY id", kind, last)
	if e != nil {
		return nil, "", e
	}
	var ids []string
	for rows.Next() {
		var id string
		if e = rows.Scan(&id); e != nil {
			rows.Close()
			return nil, "", e
		}
		ids = append(ids, id)
	}
	e = rows.Err()
	rows.Close()
	if e != nil {
		return nil, "", e
	}
	var out []*lane.EntityResult
	prev := ""
	for _, id := range ids {
		h, e := heads(ctx, c, id)
		if e != nil {
			return nil, "", e
		}
		matches := filter == nil
		if filter != nil {
			for _, v := range h {
				t := v.GetTask()
				if t == nil {
					continue
				}
				rid, e := canonicalRepo(ctx, c, t.RepositoryId)
				if e != nil {
					return nil, "", e
				}
				target := filter.GetRepositoryId()
				if target != "" {
					target, e = canonicalRepo(ctx, c, target)
					if e != nil {
						return nil, "", e
					}
				}
				if target != "" && rid != target || len(filter.Progress) > 0 && !slices.Contains(filter.Progress, t.Progress) || len(filter.Proposal) > 0 && !slices.Contains(filter.Proposal, t.Proposal) {
					continue
				}
				if !MatchesSearch(t.Title, t.Description, filter.Search) {
					continue
				}
				matches = true
			}
		}
		if !matches {
			continue
		}
		if len(out) == n {
			return out, nextCursor(scope, prev), nil
		}
		out = append(out, result(h))
		prev = id
	}
	return out, "", nil
}

func MatchesSearch(title, description, query string) bool {
	fold := cases.Fold()
	q := fold.String(query)
	return strings.Contains(fold.String(title), q) || strings.Contains(fold.String(description), q)
}
