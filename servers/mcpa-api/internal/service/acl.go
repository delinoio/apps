// SPDX-License-Identifier: FSL-1.1-ALv2

package service

import (
	"context"

	"connectrpc.com/connect"
	"github.com/jackc/pgx/v5"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/authz"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

func validatePrincipal(kind mcpav1.PrincipalKind, id string) error {
	switch kind {
	case mcpav1.PrincipalKind_PRINCIPAL_KIND_USER, mcpav1.PrincipalKind_PRINCIPAL_KIND_GROUP:
	default:
		return apperr.InvalidInput("principal_kind must be USER or GROUP")
	}
	_, err := parseID(id, "principal_id")
	return err
}

// ListAclEntries lists a resource's ACL. Managers can read the ACL of a
// deleted resource.
func (s *Service) ListAclEntries(ctx context.Context, req *connect.Request[mcpav1.ListAclEntriesRequest]) (*connect.Response[mcpav1.ListAclEntriesResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	acc, err := s.authz.Resource(ctx, p, req.Msg.GetResourceId(), roleManage, authz.AccessOptions{AllowDeleted: true})
	if err != nil {
		return nil, err
	}
	entries, err := store.ListACL(ctx, s.db.Pool, acc.Resource.ID)
	if err != nil {
		return nil, dbErr(err)
	}
	out := &mcpav1.ListAclEntriesResponse{Metadata: apperr.Metadata(ctx)}
	for _, e := range entries {
		out.Entries = append(out.Entries, aclProto(e))
	}
	return connect.NewResponse(out), nil
}

// PutAclEntry grants or changes a role. The last MANAGE holder cannot be
// downgraded.
func (s *Service) PutAclEntry(ctx context.Context, req *connect.Request[mcpav1.PutAclEntryRequest]) (*connect.Response[mcpav1.PutAclEntryResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	m := req.Msg
	if err := validatePrincipal(m.GetPrincipalKind(), m.GetPrincipalId()); err != nil {
		return nil, err
	}
	if m.GetRole() != roleManage && m.GetRole() != roleInvoke {
		return nil, apperr.InvalidInput("role must be MANAGE or INVOKE")
	}
	acc, err := s.authz.Resource(ctx, p, m.GetResourceId(), roleManage, authz.AccessOptions{})
	if err != nil {
		return nil, err
	}
	var entry store.ACLEntry
	err = s.mutate(ctx, acc.Resource.ID, func(tx pgx.Tx, r store.Resource) error {
		entries, err := store.ListACL(ctx, tx, r.ID)
		if err != nil {
			return err
		}
		if m.GetRole() != roleManage && !authz.HasOtherManager(entries, m.GetPrincipalKind(), m.GetPrincipalId()) {
			return apperr.FailedPrecondition("the resource needs at least one other MANAGE entry")
		}
		entry, err = store.PutACL(ctx, tx, store.ACLEntry{
			ResourceID: r.ID, PrincipalKind: m.GetPrincipalKind(), PrincipalID: m.GetPrincipalId(), Role: m.GetRole(), CreatedByUserID: p.UserID,
		})
		if err != nil {
			return err
		}
		return WriteAudit(ctx, tx, p, AuditInput{
			TenantID: r.TenantID, ResourceID: r.ID, Action: mcpav1.AuditAction_AUDIT_ACTION_ACL_CHANGED, TargetID: m.GetPrincipalId(),
			Details: map[string]string{"change": "put", "principal_kind": m.GetPrincipalKind().String(), "role": m.GetRole().String()},
		})
	})
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&mcpav1.PutAclEntryResponse{Metadata: apperr.Metadata(ctx), Entry: aclProto(entry)}), nil
}

// RemoveAclEntry removes a role. The last MANAGE holder cannot be removed.
func (s *Service) RemoveAclEntry(ctx context.Context, req *connect.Request[mcpav1.RemoveAclEntryRequest]) (*connect.Response[mcpav1.RemoveAclEntryResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	m := req.Msg
	if err := validatePrincipal(m.GetPrincipalKind(), m.GetPrincipalId()); err != nil {
		return nil, err
	}
	acc, err := s.authz.Resource(ctx, p, m.GetResourceId(), roleManage, authz.AccessOptions{})
	if err != nil {
		return nil, err
	}
	err = s.mutate(ctx, acc.Resource.ID, func(tx pgx.Tx, r store.Resource) error {
		entries, err := store.ListACL(ctx, tx, r.ID)
		if err != nil {
			return err
		}
		var existing *store.ACLEntry
		for i := range entries {
			if entries[i].PrincipalKind == m.GetPrincipalKind() && entries[i].PrincipalID == m.GetPrincipalId() {
				existing = &entries[i]
			}
		}
		if existing == nil {
			return apperr.NotFound("ACL entry not found")
		}
		if existing.Role == roleManage && !authz.HasOtherManager(entries, m.GetPrincipalKind(), m.GetPrincipalId()) {
			return apperr.FailedPrecondition("the last MANAGE entry cannot be removed")
		}
		if _, err := store.RemoveACL(ctx, tx, r.ID, m.GetPrincipalKind(), m.GetPrincipalId()); err != nil {
			return err
		}
		return WriteAudit(ctx, tx, p, AuditInput{
			TenantID: r.TenantID, ResourceID: r.ID, Action: mcpav1.AuditAction_AUDIT_ACTION_ACL_CHANGED, TargetID: m.GetPrincipalId(),
			Details: map[string]string{"change": "remove", "principal_kind": m.GetPrincipalKind().String(), "role": existing.Role.String()},
		})
	})
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&mcpav1.RemoveAclEntryResponse{Metadata: apperr.Metadata(ctx)}), nil
}
