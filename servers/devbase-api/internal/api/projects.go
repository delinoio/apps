// SPDX-License-Identifier: FSL-1.1-ALv2

package api

import (
	"context"
	"errors"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/devbase/v1"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"google.golang.org/protobuf/types/known/timestamppb"
)

var (
	opCreateProject = Op{Name: "CreateProject", Group: GroupProject, Mutation: true}
	opGetProject    = Op{Name: "GetProject", Group: GroupProject, Degraded: true}
	opListProjects  = Op{Name: "ListProjects", Group: GroupProject, Degraded: true}
	opUpdateProject = Op{Name: "UpdateProject", Group: GroupProject, Mutation: true}
	opDeleteProject = Op{Name: "DeleteProject", Group: GroupProject, Mutation: true, Degraded: true}
)

const projectColumns = "id::text,tenant_id::text,name,version,created_at,updated_at"

func scanProject(row pgx.Row) (*pb.Project, time.Time, error) {
	p := &pb.Project{}
	var created, updated time.Time
	e := row.Scan(&p.Id, &p.TenantId, &p.Name, &p.Version, &created, &updated)
	p.CreateTime, p.UpdateTime = timestamppb.New(created), timestamppb.New(updated)
	return p, created, e
}

func uniqueViolation(e error, constraint string) bool {
	var database *pgconn.PgError
	return errors.As(e, &database) && database.Code == "23505" && database.ConstraintName == constraint
}

// tenantTarget and projectTarget validate the identifier a request addresses.
func tenantTarget(id string) func() (Target, error) {
	return func() (Target, error) { return Target{TenantID: id}, requireID(id, "tenant_id") }
}
func projectTarget(id string) func() (Target, error) {
	return func() (Target, error) { return Target{ProjectID: id}, requireID(id, "project_id") }
}

func (s *Server) CreateProject(ctx context.Context, r *connect.Request[pb.CreateProjectRequest]) (*connect.Response[pb.CreateProjectResponse], error) {
	q, out := r.Msg, &pb.CreateProjectResponse{}
	return handle(s, ctx, r, opCreateProject, out, tenantTarget(q.TenantId), func() error { return projectName(q.Name) }, func(c *Call) error {
		// A tenant-scoped lock makes the count and insert atomic for concurrent creators.
		if _, e := c.DB.Exec(c.Ctx, "SELECT pg_advisory_xact_lock(hashtextextended($1,0))", "projects:"+q.TenantId); e != nil {
			return e
		}
		var count int64
		if e := c.DB.QueryRow(c.Ctx, "SELECT count(*) FROM projects WHERE tenant_id=$1", q.TenantId).Scan(&count); e != nil {
			return e
		}
		if count >= c.Config.MaxProjectsPerTenant {
			return quota("the tenant reached its project limit")
		}
		id := newID()
		p, _, e := scanProject(c.DB.QueryRow(c.Ctx, "INSERT INTO projects(id,tenant_id,name,created_at,updated_at) VALUES($1,$2,$3,$4,$4) RETURNING "+projectColumns, id, q.TenantId, q.Name, c.Now))
		if uniqueViolation(e, "projects_tenant_name") {
			return exists("a project with this name already exists in the tenant")
		}
		if e != nil {
			return e
		}
		c.ProjectID = id
		out.Project = p
		return c.Audit("project.create", id)
	})
}

// requireProject reports a project that was deleted after the request was authorized.
// A list query returns no rows both for a live empty project and for one whose cascade
// already removed every child, so an empty page must check the parent before it succeeds.
func requireProject(c *Call, id string) error {
	var exists bool
	if e := c.DB.QueryRow(c.Ctx, "SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)", id).Scan(&exists); e != nil {
		return e
	}
	if !exists {
		return notFound()
	}
	return nil
}

func (s *Server) GetProject(ctx context.Context, r *connect.Request[pb.GetProjectRequest]) (*connect.Response[pb.GetProjectResponse], error) {
	q, out := r.Msg, &pb.GetProjectResponse{}
	return handle(s, ctx, r, opGetProject, out, projectTarget(q.ProjectId), func() error { return nil }, func(c *Call) error {
		p, _, e := scanProject(c.DB.QueryRow(c.Ctx, "SELECT "+projectColumns+" FROM projects WHERE id=$1", q.ProjectId))
		if errors.Is(e, pgx.ErrNoRows) {
			return notFound()
		}
		out.Project = p
		return e
	})
}

func (s *Server) ListProjects(ctx context.Context, r *connect.Request[pb.ListProjectsRequest]) (*connect.Response[pb.ListProjectsResponse], error) {
	q, out := r.Msg, &pb.ListProjectsResponse{}
	var size int
	var after *cursor
	return handle(s, ctx, r, opListProjects, out, tenantTarget(q.TenantId), func() (e error) {
		if size, e = pageSize(q.PageSize); e != nil {
			return e
		}
		after, e = decodeCursor(q.PageToken)
		return e
	}, func(c *Call) error {
		at, id := after.args()
		rows, e := c.DB.Query(c.Ctx, "SELECT "+projectColumns+" FROM projects WHERE tenant_id=$1 AND ($2::timestamptz IS NULL OR (created_at,id)>($2,$3::uuid)) ORDER BY created_at,id LIMIT $4", q.TenantId, at, id, size+1)
		if e != nil {
			return e
		}
		defer rows.Close()
		var created []time.Time
		for rows.Next() {
			p, t, e := scanProject(rows)
			if e != nil {
				return e
			}
			out.Projects, created = append(out.Projects, p), append(created, t)
		}
		if e = rows.Err(); e != nil {
			return e
		}
		if len(out.Projects) > size {
			out.Projects = out.Projects[:size]
			out.NextPageToken = encodeCursor(created[size-1], out.Projects[size-1].Id)
		}
		return nil
	})
}

func (s *Server) UpdateProject(ctx context.Context, r *connect.Request[pb.UpdateProjectRequest]) (*connect.Response[pb.UpdateProjectResponse], error) {
	q, out := r.Msg, &pb.UpdateProjectResponse{}
	return handle(s, ctx, r, opUpdateProject, out, projectTarget(q.ProjectId), func() error { return projectName(q.Name) }, func(c *Call) error {
		p, _, e := scanProject(c.DB.QueryRow(c.Ctx, "UPDATE projects SET name=$2,version=version+1,updated_at=$3 WHERE id=$1 AND version=$4 RETURNING "+projectColumns, q.ProjectId, q.Name, c.Now, q.Version))
		if uniqueViolation(e, "projects_tenant_name") {
			return exists("a project with this name already exists in the tenant")
		}
		if errors.Is(e, pgx.ErrNoRows) {
			return missingOrStale(c, "projects", q.ProjectId)
		}
		if e != nil {
			return e
		}
		out.Project = p
		return c.Audit("project.update", q.ProjectId)
	})
}

// missingOrStale distinguishes a stale version from a missing row after a conditional write matched nothing.
func missingOrStale(c *Call, table, id string) error {
	var found bool
	if e := c.DB.QueryRow(c.Ctx, "SELECT EXISTS(SELECT 1 FROM "+table+" WHERE id=$1)", id).Scan(&found); e != nil {
		return e
	}
	if found {
		return stale()
	}
	return notFound()
}

func (s *Server) DeleteProject(ctx context.Context, r *connect.Request[pb.DeleteProjectRequest]) (*connect.Response[pb.DeleteProjectResponse], error) {
	q, out := r.Msg, &pb.DeleteProjectResponse{}
	return handle(s, ctx, r, opDeleteProject, out, projectTarget(q.ProjectId), func() error { return nil }, func(c *Call) error {
		// Foreign keys delete the project's users and keys in this transaction.
		tag, e := c.DB.Exec(c.Ctx, "DELETE FROM projects WHERE id=$1 AND version=$2", q.ProjectId, q.Version)
		if e != nil {
			return e
		}
		if tag.RowsAffected() == 0 {
			return missingOrStale(c, "projects", q.ProjectId)
		}
		c.ProjectID = q.ProjectId
		if e = ScrubProject(c.Ctx, c.DB, q.ProjectId); e != nil {
			return e
		}
		return c.Audit("project.delete", q.ProjectId)
	})
}
