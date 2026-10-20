// SPDX-License-Identifier: FSL-1.1-ALv2

package api

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"errors"
	"net/http"
	"strings"
	"sync"
	"time"

	pb "github.com/delinoio/apps/protos/devbase/v1"
	"github.com/delinoio/apps/servers/devbase-api/internal/delcore"
	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
)

const keyPrefix = "dbk_"

// credential is either a login token (forwarded to Delcore) or a project API key.
type credential struct {
	authorization string
	keyID         string
	secret        string
}

// identity is the verified caller and the tenant and project it may act on.
type identity struct {
	actor     string
	principal string
	keyID     string
	tenantID  string
	projectID string
}

func parseCredential(h http.Header) (credential, error) {
	parts := strings.Fields(h.Get("Authorization"))
	if len(parts) != 2 || !strings.EqualFold(parts[0], "Bearer") {
		return credential{}, unauthenticated()
	}
	token := parts[1]
	if !strings.HasPrefix(token, keyPrefix) {
		return credential{authorization: "Bearer " + token}, nil
	}
	id, secret, ok := ParseKey(token)
	if !ok {
		return credential{}, unauthenticated()
	}
	return credential{keyID: id, secret: secret}, nil
}

// FormatKey builds the raw API key from its ID and secret.
func FormatKey(id, secret string) string {
	return keyPrefix + strings.ReplaceAll(id, "-", "") + "_" + secret
}

// ParseKey splits a raw API key. It validates only the format.
func ParseKey(raw string) (id, secret string, ok bool) {
	rest, found := strings.CutPrefix(raw, keyPrefix)
	if !found || len(rest) != 32+1+43 || rest[32] != '_' {
		return "", "", false
	}
	u, e := uuid.Parse(rest[:32])
	if e != nil || u.Version() != 7 {
		return "", "", false
	}
	secret = rest[33:]
	if b, e := base64.RawURLEncoding.DecodeString(secret); e != nil || len(b) != 32 {
		return "", "", false
	}
	return u.String(), secret, true
}

// newSecret returns a random 256-bit secret and its storage hash.
func newSecret() (secret string, hash []byte, e error) {
	b := make([]byte, 32)
	if _, e = rand.Read(b); e != nil {
		return "", nil, e
	}
	secret = base64.RawURLEncoding.EncodeToString(b)
	return secret, hashSecret(secret), nil
}

func hashSecret(secret string) []byte {
	sum := sha256.Sum256([]byte(secret))
	return sum[:]
}

type keyRow struct {
	projectID, tenantID string
	permission          pb.ApiKeyPermission
	hash                []byte
	expires, revoked    *time.Time
}

func (k keyRow) usable(now time.Time) bool {
	return k.revoked == nil && (k.expires == nil || k.expires.After(now))
}

func (s *Server) loadKey(ctx context.Context, q Querier, id string, lock string) (keyRow, error) {
	var k keyRow
	var permission int16
	e := q.QueryRow(ctx, `SELECT k.project_id::text,p.tenant_id::text,k.permission,k.secret_hash,k.expires_at,k.revoked_at
 FROM api_keys k JOIN projects p ON p.id=k.project_id WHERE k.id=$1`+lock, id).Scan(&k.projectID, &k.tenantID, &permission, &k.hash, &k.expires, &k.revoked)
	k.permission = pb.ApiKeyPermission(permission)
	return k, e
}

func (s *Server) authenticate(ctx context.Context, cred credential, op Op, t Target, idempotency string) (identity, error) {
	if cred.keyID != "" {
		return s.authenticateKey(ctx, cred, op, t)
	}
	return s.authenticateLogin(ctx, cred, op, t, idempotency)
}

func (s *Server) authenticateKey(ctx context.Context, cred credential, op Op, t Target) (identity, error) {
	k, e := s.loadKey(ctx, s.Pool, cred.keyID, "")
	if errors.Is(e, pgx.ErrNoRows) {
		return identity{}, unauthenticated()
	}
	if e != nil {
		return identity{}, e
	}
	if subtle.ConstantTimeCompare(hashSecret(cred.secret), k.hash) != 1 || !k.usable(s.now()) {
		return identity{}, unauthenticated()
	}
	switch {
	case op.Group.management():
		return identity{}, denied("project API keys cannot manage projects or keys")
	case op.Group == GroupUserWrite && k.permission != pb.ApiKeyPermission_API_KEY_PERMISSION_CRUD:
		return identity{}, denied("the key permission does not allow changes")
	}
	if t.ProjectID != k.projectID {
		return identity{}, notFound()
	}
	state, e := s.tenantState(ctx, k.tenantID)
	if e != nil {
		return identity{}, e
	}
	if state != delcore.StateActive {
		return identity{}, denied("the tenant state does not permit project key access")
	}
	return identity{actor: "key:" + cred.keyID, principal: "key:" + cred.keyID, keyID: cred.keyID, tenantID: k.tenantID, projectID: k.projectID}, nil
}

func (s *Server) tenantState(ctx context.Context, tenant string) (delcore.State, error) {
	states, e := s.Authority.TenantStates(ctx, []string{tenant})
	if e != nil {
		return delcore.StateUnknown, unavailable()
	}
	return states[tenant], nil
}

// admitLoginState applies the tenant-state policy for Owners and Admins.
func admitLoginState(state delcore.State, op Op) error {
	switch state {
	case delcore.StateActive:
		return nil
	case delcore.StateBlocked, delcore.StateDeleting:
		if op.Degraded {
			return nil
		}
	}
	return denied("the tenant state does not permit this operation")
}

func (s *Server) authenticateLogin(ctx context.Context, cred credential, op Op, t Target, idempotency string) (identity, error) {
	tenant := t.TenantID
	user := ""
	if tenant == "" {
		var e error
		tenant, e = s.projectTenant(ctx, t.ProjectID)
		if errors.Is(e, pgx.ErrNoRows) {
			// The project is gone. A repeated delete may still replay, but only for
			// the same authenticated caller; everything else is not found. The caller's
			// record is found by caller and key alone, so a key already used for another
			// project or operation still ends in a conflict.
			if user, e = s.identify(ctx, cred); e != nil {
				return identity{}, e
			}
			if !op.Mutation {
				return identity{}, notFound()
			}
			if tenant, e = s.replayTenant(ctx, "user:"+user, idempotency); e != nil {
				return identity{}, e
			}
		} else if e != nil {
			return identity{}, e
		}
	}
	var (
		wg               sync.WaitGroup
		uerr, merr, serr error
		states           map[string]delcore.State
	)
	if user == "" {
		wg.Add(1)
		go func() { defer wg.Done(); user, uerr = s.Authority.Identify(ctx, cred.authorization) }()
	}
	wg.Add(2)
	go func() { defer wg.Done(); merr = s.Authority.RequireManager(ctx, cred.authorization, tenant) }()
	go func() { defer wg.Done(); states, serr = s.Authority.TenantStates(ctx, []string{tenant}) }()
	wg.Wait()
	for _, e := range []error{uerr, merr} {
		if errors.Is(e, delcore.ErrUnauthenticated) {
			return identity{}, unauthenticated()
		}
	}
	if errors.Is(merr, delcore.ErrForbidden) {
		return identity{}, denied("tenant owner or administrator authority is required")
	}
	if uerr != nil || merr != nil || serr != nil {
		return identity{}, unavailable()
	}
	if e := admitLoginState(states[tenant], op); e != nil {
		return identity{}, e
	}
	return identity{actor: "user:" + user, principal: "user:" + user, tenantID: tenant, projectID: t.ProjectID}, nil
}

func (s *Server) identify(ctx context.Context, cred credential) (string, error) {
	user, e := s.Authority.Identify(ctx, cred.authorization)
	switch {
	case errors.Is(e, delcore.ErrUnauthenticated):
		return "", unauthenticated()
	case e != nil:
		return "", unavailable()
	}
	return user, nil
}

func (s *Server) projectTenant(ctx context.Context, project string) (string, error) {
	var tenant string
	e := s.Pool.QueryRow(ctx, "SELECT tenant_id::text FROM projects WHERE id=$1", project).Scan(&tenant)
	return tenant, e
}

// replayTenant finds the tenant of a deleted project from the caller's own replay
// record. It lets a repeated delete re-check current authority instead of trusting the record.
// The lookup is by caller and key only: filtering by project would hide the record when the key
// was used for another project, and the uniqueness rule requires that case to conflict.
func (s *Server) replayTenant(ctx context.Context, principal, key string) (string, error) {
	var tenant string
	e := s.Pool.QueryRow(ctx, "SELECT tenant_id::text FROM replays WHERE principal=$1 AND idempotency_key=$2 AND expires_at>$3", principal, key, s.now()).Scan(&tenant)
	if errors.Is(e, pgx.ErrNoRows) {
		return "", notFound()
	}
	return tenant, e
}
