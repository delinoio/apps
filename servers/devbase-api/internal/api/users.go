// SPDX-License-Identifier: FSL-1.1-ALv2

package api

import (
	"context"
	"errors"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/devbase/v1"
	"github.com/jackc/pgx/v5"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/types/known/structpb"
	"google.golang.org/protobuf/types/known/timestamppb"
)

var (
	opCreateEndUser = Op{Name: "CreateEndUser", Group: GroupUserWrite, Mutation: true}
	opGetEndUser    = Op{Name: "GetEndUser", Group: GroupUserRead}
	opLookupEndUser = Op{Name: "LookupEndUser", Group: GroupUserRead}
	opListEndUsers  = Op{Name: "ListEndUsers", Group: GroupUserRead}
	opUpdateEndUser = Op{Name: "UpdateEndUser", Group: GroupUserWrite, Mutation: true}
	opDeleteEndUser = Op{Name: "DeleteEndUser", Group: GroupUserWrite, Mutation: true, Degraded: true}
)

const userColumns = "id::text,project_id::text,external_id,COALESCE(name,''),COALESCE(email,''),metadata::text,version,created_at,updated_at"

func scanUser(row pgx.Row) (*pb.EndUser, time.Time, error) {
	u := &pb.EndUser{}
	var metadata string
	var created, updated time.Time
	if e := row.Scan(&u.Id, &u.ProjectId, &u.ExternalId, &u.Name, &u.Email, &metadata, &u.Version, &created, &updated); e != nil {
		return nil, created, e
	}
	u.Metadata = &structpb.Struct{}
	if e := protojson.Unmarshal([]byte(metadata), u.Metadata); e != nil {
		return nil, created, e
	}
	u.CreateTime, u.UpdateTime = timestamppb.New(created), timestamppb.New(updated)
	return u, created, nil
}

const externalIDConstraint = "end_users_project_id_external_id_key"

// nullable stores an empty optional string as NULL.
func nullable(s string) any {
	if s == "" {
		return nil
	}
	return s
}

func (s *Server) CreateEndUser(ctx context.Context, r *connect.Request[pb.CreateEndUserRequest]) (*connect.Response[pb.CreateEndUserResponse], error) {
	q, out := r.Msg, &pb.CreateEndUserResponse{}
	var metadata []byte
	return handle(s, ctx, r, opCreateEndUser, out, projectTarget(q.ProjectId), func() (e error) {
		for _, check := range []error{externalID(q.ExternalId), userName(q.Name), userEmail(q.Email)} {
			if check != nil {
				return check
			}
		}
		metadata, e = metadataJSON(q.Metadata)
		return e
	}, func(c *Call) error {
		// The atomic counter enforces the quota and locks the project row, which also
		// makes project deletion wait for, or fail, this creation.
		tag, e := c.DB.Exec(c.Ctx, "UPDATE projects SET user_count=user_count+1 WHERE id=$1 AND user_count<$2", q.ProjectId, c.Config.MaxUsersPerProject)
		if e != nil {
			return e
		}
		if tag.RowsAffected() == 0 {
			var found bool
			if e = c.DB.QueryRow(c.Ctx, "SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)", q.ProjectId).Scan(&found); e != nil {
				return e
			}
			if found {
				return quota("the project reached its end-user limit")
			}
			return notFound()
		}
		id := newID()
		u, _, e := scanUser(c.DB.QueryRow(c.Ctx, `INSERT INTO end_users(id,project_id,external_id,name,email,metadata,created_at,updated_at)
 VALUES($1,$2,$3,$4,$5,$6::jsonb,$7,$7) RETURNING `+userColumns, id, q.ProjectId, q.ExternalId, nullable(q.Name), nullable(q.Email), string(metadata), c.Now))
		if uniqueViolation(e, externalIDConstraint) {
			return exists("an end user with this external ID already exists in the project")
		}
		if e != nil {
			return e
		}
		out.EndUser, c.replayUser = u, id
		return c.Audit("end_user.create", id)
	})
}

func (s *Server) GetEndUser(ctx context.Context, r *connect.Request[pb.GetEndUserRequest]) (*connect.Response[pb.GetEndUserResponse], error) {
	q, out := r.Msg, &pb.GetEndUserResponse{}
	return handle(s, ctx, r, opGetEndUser, out, projectTarget(q.ProjectId), func() error { return requireID(q.EndUserId, "end_user_id") }, func(c *Call) error {
		u, _, e := scanUser(c.DB.QueryRow(c.Ctx, "SELECT "+userColumns+" FROM end_users WHERE id=$1 AND project_id=$2", q.EndUserId, q.ProjectId))
		if errors.Is(e, pgx.ErrNoRows) {
			return notFound()
		}
		out.EndUser = u
		return e
	})
}

func (s *Server) LookupEndUser(ctx context.Context, r *connect.Request[pb.LookupEndUserRequest]) (*connect.Response[pb.LookupEndUserResponse], error) {
	q, out := r.Msg, &pb.LookupEndUserResponse{}
	return handle(s, ctx, r, opLookupEndUser, out, projectTarget(q.ProjectId), func() error { return externalID(q.ExternalId) }, func(c *Call) error {
		u, _, e := scanUser(c.DB.QueryRow(c.Ctx, "SELECT "+userColumns+" FROM end_users WHERE project_id=$1 AND external_id=$2", q.ProjectId, q.ExternalId))
		if errors.Is(e, pgx.ErrNoRows) {
			return notFound()
		}
		out.EndUser = u
		return e
	})
}

func (s *Server) ListEndUsers(ctx context.Context, r *connect.Request[pb.ListEndUsersRequest]) (*connect.Response[pb.ListEndUsersResponse], error) {
	q, out := r.Msg, &pb.ListEndUsersResponse{}
	var size int
	var after *cursor
	return handle(s, ctx, r, opListEndUsers, out, projectTarget(q.ProjectId), func() (e error) {
		if size, e = pageSize(q.PageSize); e != nil {
			return e
		}
		after, e = decodeCursor(q.PageToken)
		return e
	}, func(c *Call) error {
		at, id := after.args()
		rows, e := c.DB.Query(c.Ctx, "SELECT "+userColumns+" FROM end_users WHERE project_id=$1 AND ($2::timestamptz IS NULL OR (created_at,id)>($2,$3::uuid)) ORDER BY created_at,id LIMIT $4", q.ProjectId, at, id, size+1)
		if e != nil {
			return e
		}
		defer rows.Close()
		var created []time.Time
		for rows.Next() {
			u, t, e := scanUser(rows)
			if e != nil {
				return e
			}
			out.EndUsers, created = append(out.EndUsers, u), append(created, t)
		}
		if e = rows.Err(); e != nil {
			return e
		}
		if len(out.EndUsers) > size {
			out.EndUsers = out.EndUsers[:size]
			out.NextPageToken = encodeCursor(created[size-1], out.EndUsers[size-1].Id)
		}
		if len(out.EndUsers) == 0 {
			// A project deleted after authorization cascades away every user, so an empty page
			// is also what a deleted project returns. Check the parent before reporting success.
			rows.Close()
			var exists bool
			if e = c.DB.QueryRow(c.Ctx, "SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)", q.ProjectId).Scan(&exists); e != nil {
				return e
			}
			if !exists {
				return notFound()
			}
		}
		return nil
	})
}

// maskable lists the fields an update may change.
var maskable = map[string]bool{"external_id": true, "name": true, "email": true, "metadata": true}

func (s *Server) UpdateEndUser(ctx context.Context, r *connect.Request[pb.UpdateEndUserRequest]) (*connect.Response[pb.UpdateEndUserResponse], error) {
	q, out := r.Msg, &pb.UpdateEndUserResponse{}
	mask := map[string]bool{}
	var metadata []byte
	return handle(s, ctx, r, opUpdateEndUser, out, projectTarget(q.ProjectId), func() (e error) {
		if e = requireID(q.EndUserId, "end_user_id"); e != nil {
			return e
		}
		paths := q.UpdateMask.GetPaths()
		if len(paths) == 0 {
			return invalid("update_mask must list at least one field")
		}
		for _, path := range paths {
			if !maskable[path] {
				return invalid("update_mask supports external_id, name, email and metadata")
			}
			mask[path] = true
		}
		if mask["external_id"] {
			if e = externalID(q.ExternalId); e != nil {
				return e
			}
		}
		if mask["name"] {
			if e = userName(q.Name); e != nil {
				return e
			}
		}
		if mask["email"] {
			if e = userEmail(q.Email); e != nil {
				return e
			}
		}
		if mask["metadata"] {
			metadata, e = metadataJSON(q.Metadata)
		}
		return e
	}, func(c *Call) error {
		// The row lock serializes this update with concurrent updates and deletes.
		current, _, e := scanUser(c.DB.QueryRow(c.Ctx, "SELECT "+userColumns+" FROM end_users WHERE id=$1 AND project_id=$2 FOR UPDATE", q.EndUserId, q.ProjectId))
		if errors.Is(e, pgx.ErrNoRows) {
			return notFound()
		}
		if e != nil {
			return e
		}
		if current.Version != q.Version {
			return stale()
		}
		external, name, email, meta := current.ExternalId, current.Name, current.Email, ""
		if mask["external_id"] {
			external = q.ExternalId
		}
		if mask["name"] {
			name = q.Name
		}
		if mask["email"] {
			email = q.Email
		}
		if mask["metadata"] {
			meta = string(metadata)
		} else if raw, e := protojson.Marshal(current.Metadata); e == nil {
			meta = string(raw)
		} else {
			return e
		}
		u, _, e := scanUser(c.DB.QueryRow(c.Ctx, `UPDATE end_users SET external_id=$3,name=$4,email=$5,metadata=$6::jsonb,version=version+1,updated_at=$7
 WHERE id=$1 AND project_id=$2 RETURNING `+userColumns, q.EndUserId, q.ProjectId, external, nullable(name), nullable(email), meta, c.Now))
		if uniqueViolation(e, externalIDConstraint) {
			return exists("an end user with this external ID already exists in the project")
		}
		if e != nil {
			return e
		}
		out.EndUser, c.replayUser = u, q.EndUserId
		return c.Audit("end_user.update", q.EndUserId)
	})
}

func (s *Server) DeleteEndUser(ctx context.Context, r *connect.Request[pb.DeleteEndUserRequest]) (*connect.Response[pb.DeleteEndUserResponse], error) {
	q, out := r.Msg, &pb.DeleteEndUserResponse{}
	return handle(s, ctx, r, opDeleteEndUser, out, projectTarget(q.ProjectId), func() error { return requireID(q.EndUserId, "end_user_id") }, func(c *Call) error {
		// Lock the project before the user. DeleteProject and tenant cleanup lock the parent
		// first and then cascade to the children, so any other order can deadlock.
		var locked string
		if e := c.DB.QueryRow(c.Ctx, "SELECT id::text FROM projects WHERE id=$1 FOR NO KEY UPDATE", q.ProjectId).Scan(&locked); errors.Is(e, pgx.ErrNoRows) {
			return notFound()
		} else if e != nil {
			return e
		}
		tag, e := c.DB.Exec(c.Ctx, "DELETE FROM end_users WHERE id=$1 AND project_id=$2 AND version=$3", q.EndUserId, q.ProjectId, q.Version)
		if e != nil {
			return e
		}
		if tag.RowsAffected() == 0 {
			var found bool
			if e = c.DB.QueryRow(c.Ctx, "SELECT EXISTS(SELECT 1 FROM end_users WHERE id=$1 AND project_id=$2)", q.EndUserId, q.ProjectId).Scan(&found); e != nil {
				return e
			}
			if found {
				return stale()
			}
			return notFound()
		}
		if _, e = c.DB.Exec(c.Ctx, "UPDATE projects SET user_count=user_count-1 WHERE id=$1", q.ProjectId); e != nil {
			return e
		}
		// Deleted personal data must not remain replayable.
		if e = ScrubUser(c.Ctx, c.DB, q.EndUserId); e != nil {
			return e
		}
		return c.Audit("end_user.delete", q.EndUserId)
	})
}
