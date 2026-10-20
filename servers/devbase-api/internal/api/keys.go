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
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/timestamppb"
)

var (
	opIssueApiKey  = Op{Name: "IssueApiKey", Group: GroupKey, Mutation: true}
	opGetApiKey    = Op{Name: "GetApiKey", Group: GroupKey, Degraded: true}
	opListApiKeys  = Op{Name: "ListApiKeys", Group: GroupKey, Degraded: true}
	opRevokeApiKey = Op{Name: "RevokeApiKey", Group: GroupKey, Mutation: true, Degraded: true}
)

const keyColumns = "id::text,project_id::text,permission,created_at,expires_at,revoked_at"

func scanKey(row pgx.Row) (*pb.ApiKey, time.Time, error) {
	k := &pb.ApiKey{}
	var permission int16
	var created time.Time
	var expires, revoked *time.Time
	e := row.Scan(&k.Id, &k.ProjectId, &permission, &created, &expires, &revoked)
	k.Permission, k.CreateTime = pb.ApiKeyPermission(permission), timestamppb.New(created)
	if expires != nil {
		k.ExpireTime = timestamppb.New(*expires)
	}
	if revoked != nil {
		k.RevokeTime = timestamppb.New(*revoked)
	}
	return k, created, e
}

func foreignKeyViolation(e error) bool {
	var database *pgconn.PgError
	return errors.As(e, &database) && database.Code == "23503"
}

func (s *Server) IssueApiKey(ctx context.Context, r *connect.Request[pb.IssueApiKeyRequest]) (*connect.Response[pb.IssueApiKeyResponse], error) {
	q, out := r.Msg, &pb.IssueApiKeyResponse{}
	var expires *time.Time
	return handle(s, ctx, r, opIssueApiKey, out, projectTarget(q.ProjectId), func() error {
		if q.Permission != pb.ApiKeyPermission_API_KEY_PERMISSION_READ_ONLY && q.Permission != pb.ApiKeyPermission_API_KEY_PERMISSION_CRUD {
			return invalid("permission must be READ_ONLY or CRUD")
		}
		if q.ExpireTime != nil {
			if e := q.ExpireTime.CheckValid(); e != nil {
				return invalid("expire_time is invalid")
			}
			// Only the structure is checked here. "In the future" depends on the clock, so it
			// is checked after replay resolution; a retry of a successful issuance must
			// replay even after the requested expiry has passed.
			t := q.ExpireTime.AsTime().Truncate(time.Microsecond)
			expires = &t
		}
		return nil
	}, func(c *Call) error {
		if expires != nil && !expires.After(c.Now) {
			return invalid("expire_time must be in the future")
		}
		// Serialize issuers per project so the active-key count and insert are atomic.
		if _, e := c.DB.Exec(c.Ctx, "SELECT pg_advisory_xact_lock(hashtextextended($1,0))", "keys:"+q.ProjectId); e != nil {
			return e
		}
		var active int64
		if e := c.DB.QueryRow(c.Ctx, "SELECT count(*) FROM api_keys WHERE project_id=$1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>$2)", q.ProjectId, c.Now).Scan(&active); e != nil {
			return e
		}
		if active >= c.Config.MaxActiveKeysPerProject {
			return quota("the project reached its active key limit")
		}
		secret, hash, e := newSecret()
		if e != nil {
			return e
		}
		id := newID()
		k, _, e := scanKey(c.DB.QueryRow(c.Ctx, "INSERT INTO api_keys(id,project_id,permission,secret_hash,created_at,expires_at) VALUES($1,$2,$3,$4,$5,$6) RETURNING "+keyColumns, id, q.ProjectId, int16(q.Permission), hash, c.Now, expires))
		if foreignKeyViolation(e) {
			return notFound()
		}
		if e != nil {
			return e
		}
		out.ApiKey, out.Secret = k, FormatKey(id, secret)
		c.ProjectID = q.ProjectId
		// A replay returns the key ID and permission, never the secret.
		c.redact = func(m proto.Message) { m.(*pb.IssueApiKeyResponse).Secret = "" }
		return c.Audit("api_key.issue", id)
	})
}

func (s *Server) GetApiKey(ctx context.Context, r *connect.Request[pb.GetApiKeyRequest]) (*connect.Response[pb.GetApiKeyResponse], error) {
	q, out := r.Msg, &pb.GetApiKeyResponse{}
	return handle(s, ctx, r, opGetApiKey, out, projectTarget(q.ProjectId), func() error { return requireID(q.ApiKeyId, "api_key_id") }, func(c *Call) error {
		k, _, e := scanKey(c.DB.QueryRow(c.Ctx, "SELECT "+keyColumns+" FROM api_keys WHERE id=$1 AND project_id=$2", q.ApiKeyId, q.ProjectId))
		if errors.Is(e, pgx.ErrNoRows) {
			return notFound()
		}
		out.ApiKey = k
		return e
	})
}

func (s *Server) ListApiKeys(ctx context.Context, r *connect.Request[pb.ListApiKeysRequest]) (*connect.Response[pb.ListApiKeysResponse], error) {
	q, out := r.Msg, &pb.ListApiKeysResponse{}
	var size int
	var after *cursor
	return handle(s, ctx, r, opListApiKeys, out, projectTarget(q.ProjectId), func() (e error) {
		if size, e = pageSize(q.PageSize); e != nil {
			return e
		}
		after, e = decodeCursor(q.PageToken)
		return e
	}, func(c *Call) error {
		at, id := after.args()
		rows, e := c.DB.Query(c.Ctx, "SELECT "+keyColumns+" FROM api_keys WHERE project_id=$1 AND ($2::timestamptz IS NULL OR (created_at,id)>($2,$3::uuid)) ORDER BY created_at,id LIMIT $4", q.ProjectId, at, id, size+1)
		if e != nil {
			return e
		}
		defer rows.Close()
		var created []time.Time
		for rows.Next() {
			k, t, e := scanKey(rows)
			if e != nil {
				return e
			}
			out.ApiKeys, created = append(out.ApiKeys, k), append(created, t)
		}
		if e = rows.Err(); e != nil {
			return e
		}
		if len(out.ApiKeys) > size {
			out.ApiKeys = out.ApiKeys[:size]
			out.NextPageToken = encodeCursor(created[size-1], out.ApiKeys[size-1].Id)
		}
		if len(out.ApiKeys) == 0 {
			rows.Close()
			return requireProject(c, q.ProjectId)
		}
		return nil
	})
}

func (s *Server) RevokeApiKey(ctx context.Context, r *connect.Request[pb.RevokeApiKeyRequest]) (*connect.Response[pb.RevokeApiKeyResponse], error) {
	q, out := r.Msg, &pb.RevokeApiKeyResponse{}
	return handle(s, ctx, r, opRevokeApiKey, out, projectTarget(q.ProjectId), func() error { return requireID(q.ApiKeyId, "api_key_id") }, func(c *Call) error {
		k, _, e := scanKey(c.DB.QueryRow(c.Ctx, "UPDATE api_keys SET revoked_at=$3 WHERE id=$1 AND project_id=$2 AND revoked_at IS NULL RETURNING "+keyColumns, q.ApiKeyId, q.ProjectId, c.Now))
		if errors.Is(e, pgx.ErrNoRows) {
			// Revocation is idempotent in effect: an already revoked key is returned unchanged.
			k, _, e = scanKey(c.DB.QueryRow(c.Ctx, "SELECT "+keyColumns+" FROM api_keys WHERE id=$1 AND project_id=$2", q.ApiKeyId, q.ProjectId))
			if errors.Is(e, pgx.ErrNoRows) {
				return notFound()
			}
			out.ApiKey = k
			return e
		}
		if e != nil {
			return e
		}
		out.ApiKey = k
		return c.Audit("api_key.revoke", q.ApiKeyId)
	})
}
