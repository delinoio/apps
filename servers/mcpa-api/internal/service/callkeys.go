// SPDX-License-Identifier: FSL-1.1-ALv2

package service

import (
	"bytes"
	"context"
	"errors"
	"time"

	"connectrpc.com/connect"
	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/authz"
	"github.com/delinoio/apps/servers/mcpa-api/internal/callkey"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

func displayPrefix(id uuid.UUID) string { return callkey.DisplayPrefix(id) }

// IssueCallKey issues a call key. The secret is returned once, by the first
// execution of the request key.
func (s *Service) IssueCallKey(ctx context.Context, req *connect.Request[mcpav1.IssueCallKeyRequest]) (*connect.Response[mcpav1.IssueCallKeyResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	m := req.Msg
	requestKey, err := parseRequestKey(m.GetRequestKey())
	if err != nil {
		return nil, err
	}
	if err := validateName("name", m.GetName()); err != nil {
		return nil, err
	}
	var expire *time.Time
	expireField := ""
	if m.GetExpireTime() != nil {
		if err := m.GetExpireTime().CheckValid(); err != nil {
			return nil, apperr.InvalidInput("expire_time is not a valid timestamp")
		}
		t := m.GetExpireTime().AsTime()
		if !t.After(s.now()) {
			return nil, apperr.InvalidInput("expire_time must be in the future")
		}
		expire = &t
		expireField = t.UTC().Format(time.RFC3339Nano)
	}
	acc, err := s.authz.Resource(ctx, p, m.GetResourceId(), roleInvoke, authz.AccessOptions{})
	if err != nil {
		return nil, err
	}
	key, err := callkey.New()
	if err != nil {
		return nil, apperr.Internal(err)
	}
	fp := fingerprint("IssueCallKey", acc.Resource.ID.String(), m.GetName(), expireField)
	var stored store.CallKey
	secret := key.Secret
	err = s.mutate(ctx, p, acc.Resource.ID, roleInvoke, func(tx pgx.Tx, r store.Resource, _ mcpav1.ResourceRole) error {
		rec, claimed, err := store.ClaimIdempotency(ctx, tx, "IssueCallKey", p.UserID, requestKey, fp, key.ID)
		if err != nil {
			return err
		}
		if !claimed {
			if !bytes.Equal(rec.Fingerprint, fp) {
				return apperr.IdempotencyConflict()
			}
			secret = ""
			stored, err = store.GetCallKey(ctx, tx, r.ID, rec.ResultID)
			if errors.Is(err, store.ErrNotFound) {
				return apperr.NotFound("call key not found")
			}
			return err
		}
		stored, err = store.InsertCallKey(ctx, tx, store.CallKey{
			ID: key.ID, ResourceID: r.ID, TenantID: r.TenantID, Name: m.GetName(), SecretHash: key.Hash,
			IssuerUserID: p.UserID, ExpireTime: expire,
		})
		if err != nil {
			return err
		}
		return WriteAudit(ctx, tx, p, AuditInput{
			TenantID: r.TenantID, ResourceID: r.ID, Action: mcpav1.AuditAction_AUDIT_ACTION_CALL_KEY_ISSUED, TargetID: key.ID.String(),
			Details: map[string]string{"display_prefix": callkey.DisplayPrefix(key.ID)},
		})
	})
	if err != nil {
		return nil, err
	}
	if secret != "" {
		s.log.InfoContext(ctx, "call key issued", "resource_id", stored.ResourceID, "call_key_id", stored.ID)
	}
	return connect.NewResponse(&mcpav1.IssueCallKeyResponse{Metadata: apperr.Metadata(ctx), CallKey: callKeyProto(stored), Secret: secret}), nil
}

// ListCallKeys lists every key for managers and the caller's own keys for
// invokers.
func (s *Service) ListCallKeys(ctx context.Context, req *connect.Request[mcpav1.ListCallKeysRequest]) (*connect.Response[mcpav1.ListCallKeysResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	page, err := ParsePage(req.Msg.GetPage())
	if err != nil {
		return nil, err
	}
	acc, err := s.authz.Resource(ctx, p, req.Msg.GetResourceId(), roleInvoke, authz.AccessOptions{})
	if err != nil {
		return nil, err
	}
	issuer := ""
	if !authz.Satisfies(acc.Role, roleManage) {
		issuer = p.UserID
	}
	keys, err := store.ListCallKeys(ctx, s.db.Pool, acc.Resource.ID, issuer, page.After, page.Size+1)
	if err != nil {
		return nil, dbErr(err)
	}
	out := &mcpav1.ListCallKeysResponse{Metadata: apperr.Metadata(ctx)}
	if len(keys) > page.Size {
		keys = keys[:page.Size]
		out.NextAfterId = keys[len(keys)-1].ID.String()
	}
	for _, k := range keys {
		out.CallKeys = append(out.CallKeys, callKeyProto(k))
	}
	return connect.NewResponse(out), nil
}

// RevokeCallKey revokes a key. Invokers may revoke only their own keys.
func (s *Service) RevokeCallKey(ctx context.Context, req *connect.Request[mcpav1.RevokeCallKeyRequest]) (*connect.Response[mcpav1.RevokeCallKeyResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	keyID, err := parseID(req.Msg.GetCallKeyId(), "call_key_id")
	if err != nil {
		return nil, err
	}
	acc, err := s.authz.Resource(ctx, p, req.Msg.GetResourceId(), roleInvoke, authz.AccessOptions{})
	if err != nil {
		return nil, err
	}
	var out store.CallKey
	err = s.mutate(ctx, p, acc.Resource.ID, roleInvoke, func(tx pgx.Tx, r store.Resource, role mcpav1.ResourceRole) error {
		k, err := store.GetCallKey(ctx, tx, r.ID, keyID)
		if errors.Is(err, store.ErrNotFound) {
			return apperr.NotFound("call key not found")
		}
		if err != nil {
			return err
		}
		if !authz.Satisfies(role, roleManage) && k.IssuerUserID != p.UserID {
			return apperr.NotFound("call key not found")
		}
		var changed bool
		if out, changed, err = store.RevokeCallKey(ctx, tx, r.ID, keyID); err != nil || !changed {
			return err
		}
		return WriteAudit(ctx, tx, p, AuditInput{
			TenantID: r.TenantID, ResourceID: r.ID, Action: mcpav1.AuditAction_AUDIT_ACTION_CALL_KEY_REVOKED, TargetID: keyID.String(),
			Details: map[string]string{"display_prefix": callkey.DisplayPrefix(keyID)},
		})
	})
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&mcpav1.RevokeCallKeyResponse{Metadata: apperr.Metadata(ctx), CallKey: callKeyProto(out)}), nil
}
