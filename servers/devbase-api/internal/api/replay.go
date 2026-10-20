// SPDX-License-Identifier: FSL-1.1-ALv2

package api

import (
	"context"
	"crypto/sha256"
	"errors"
	"time"

	"github.com/jackc/pgx/v5"
	"google.golang.org/protobuf/proto"
)

// ReplayRetention is how long idempotency records are kept.
const ReplayRetention = 24 * time.Hour

// fingerprint hashes the operation and the request without its idempotency key.
func fingerprint(op Op, msg proto.Message) ([]byte, error) {
	m := proto.Clone(msg)
	if fd := m.ProtoReflect().Descriptor().Fields().ByName("idempotency_key"); fd != nil {
		m.ProtoReflect().Clear(fd)
	}
	raw, e := proto.MarshalOptions{Deterministic: true}.Marshal(m)
	if e != nil {
		return nil, e
	}
	sum := sha256.Sum256(append([]byte(op.Name+"\x00"), raw...))
	return sum[:], nil
}

// mutate runs a mutation in one transaction. The advisory lock on the caller and
// key serializes retries so exactly one request executes; the others replay.
func (s *Server) mutate(ctx context.Context, c *Call, op Op, who identity, msg, out proto.Message, key string, run func(*Call) error) error {
	print, e := fingerprint(op, msg)
	if e != nil {
		return invalid("invalid request")
	}
	tx, e := s.Pool.BeginTx(ctx, pgx.TxOptions{IsoLevel: pgx.ReadCommitted})
	if e != nil {
		return e
	}
	defer tx.Rollback(ctx)
	c.DB = tx
	if _, e = tx.Exec(ctx, "SELECT pg_advisory_xact_lock(hashtextextended($1,0))", who.principal+"|"+key); e != nil {
		return e
	}
	if who.keyID != "" {
		// Lock order is project, then key, then children, the same direction as DeleteProject
		// and tenant cleanup. Taking the key lock first can deadlock with their cascade.
		var project string
		e := tx.QueryRow(ctx, "SELECT id::text FROM projects WHERE id=$1 FOR KEY SHARE", who.projectID).Scan(&project)
		if errors.Is(e, pgx.ErrNoRows) {
			return unauthenticated()
		}
		if e != nil {
			return e
		}
		// A shared row lock makes revocation wait for in-flight work and makes later
		// work observe a committed revocation.
		k, e := s.loadKey(ctx, tx, who.keyID, " FOR SHARE OF k")
		if errors.Is(e, pgx.ErrNoRows) || (e == nil && !k.usable(c.Now)) {
			return unauthenticated()
		}
		if e != nil {
			return e
		}
	}
	if _, e = tx.Exec(ctx, "DELETE FROM replays WHERE principal=$1 AND idempotency_key=$2 AND expires_at<=$3", who.principal, key, c.Now); e != nil {
		return e
	}
	var (
		operation, tenant string
		recorded          *string
		stored            []byte
		response          []byte
		scrubbed          bool
	)
	e = tx.QueryRow(ctx, "SELECT operation,tenant_id::text,project_id::text,fingerprint,response,scrubbed FROM replays WHERE principal=$1 AND idempotency_key=$2", who.principal, key).Scan(&operation, &tenant, &recorded, &stored, &response, &scrubbed)
	switch {
	case e == nil:
		// A scrubbed record has no fingerprint, which is where the project of the original
		// request is otherwise compared, so compare the retained project here. Project
		// creation has no project yet, so its identity carries none.
		if operation != op.Name || tenant != who.tenantID || (recorded != nil && who.projectID != "" && *recorded != who.projectID) {
			return idempotencyConflict()
		}
		if scrubbed {
			// The data this record produced was deleted. Never restore or return it.
			return notFound()
		}
		if string(stored) != string(print) {
			return idempotencyConflict()
		}
		return proto.Unmarshal(response, out)
	case !errors.Is(e, pgx.ErrNoRows):
		return e
	}
	if e = run(c); e != nil {
		return e
	}
	snapshot := proto.Clone(out)
	if fd := snapshot.ProtoReflect().Descriptor().Fields().ByName("metadata"); fd != nil {
		snapshot.ProtoReflect().Clear(fd)
	}
	if c.redact != nil {
		c.redact(snapshot)
	}
	body, e := proto.Marshal(snapshot)
	if e != nil {
		return e
	}
	var project, user any
	if c.ProjectID != "" {
		project = c.ProjectID
	}
	if c.replayUser != "" {
		user = c.replayUser
	}
	if _, e = tx.Exec(ctx, `INSERT INTO replays(principal,idempotency_key,operation,tenant_id,project_id,target_user_id,fingerprint,response,created_at,expires_at)
 VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)`, who.principal, key, op.Name, c.TenantID, project, user, print, body, c.Now, c.Now.Add(ReplayRetention)); e != nil {
		return e
	}
	return tx.Commit(ctx)
}

// scrubReplays removes stored responses for deleted data. Delete operations
// carry no personal data and keep their record so a repeated delete stays idempotent.
func scrubReplays(ctx context.Context, q Querier, where string, arg any) error {
	_, e := q.Exec(ctx, "UPDATE replays SET scrubbed=true,response=NULL,fingerprint='\\x'::bytea WHERE "+where+" AND NOT scrubbed AND operation NOT LIKE 'Delete%'", arg)
	return e
}

// ScrubProject scrubs every replay record of a project that is being deleted.
func ScrubProject(ctx context.Context, q Querier, project string) error {
	return scrubReplays(ctx, q, "project_id=$1", project)
}

// ScrubUser scrubs every replay record that refers to a deleted user.
func ScrubUser(ctx context.Context, q Querier, user string) error {
	return scrubReplays(ctx, q, "target_user_id=$1", user)
}

// ScrubProjects scrubs every replay record of the given projects, for tenant cleanup. It selects
// by project ID so the update uses the replays_project index; the table has no tenant index,
// and filtering by tenant would scan every replay record once per deleted tenant.
func ScrubProjects(ctx context.Context, q Querier, projects []string) error {
	return scrubReplays(ctx, q, "project_id=ANY($1::uuid[])", projects)
}
