// SPDX-License-Identifier: FSL-1.1-ALv2

package service

import (
	"bytes"
	"context"
	"errors"
	"strconv"

	"connectrpc.com/connect"
	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/authz"
	"github.com/delinoio/apps/servers/mcpa-api/internal/credential"
	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

const (
	roleManage = mcpav1.ResourceRole_RESOURCE_ROLE_MANAGE
	roleInvoke = mcpav1.ResourceRole_RESOURCE_ROLE_INVOKE
)

func nameConflict(err error) error {
	if store.IsUniqueViolation(err, store.ResourceNameIndex) {
		return apperr.AlreadyExists("a resource with this name already exists in the tenant")
	}
	return err
}

// CreateResource creates a resource and grants the creator MANAGE.
func (s *Service) CreateResource(ctx context.Context, req *connect.Request[mcpav1.CreateResourceRequest]) (*connect.Response[mcpav1.CreateResourceResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	m := req.Msg
	requestKey, err := parseRequestKey(m.GetRequestKey())
	if err != nil {
		return nil, err
	}
	if m.GetTenantId() == "" {
		return nil, apperr.InvalidInput("tenant_id is required")
	}
	if err := validateName("name", m.GetName()); err != nil {
		return nil, err
	}
	if err := validateDescription(m.GetDescription()); err != nil {
		return nil, err
	}
	if err := s.authz.Capability(ctx, p, m.GetTenantId(), delcore.CapabilityCreateResources); err != nil {
		return nil, err
	}
	fp := fingerprint("CreateResource", m.GetTenantId(), m.GetName(), m.GetDescription())
	var res store.Resource
	replay := false
	err = s.db.InTx(ctx, func(tx pgx.Tx) error {
		rec, claimed, err := store.ClaimIdempotency(ctx, tx, "CreateResource", p.UserID, requestKey, fp, store.NewID())
		if err != nil {
			return err
		}
		if !claimed {
			if !bytes.Equal(rec.Fingerprint, fp) {
				return apperr.IdempotencyConflict()
			}
			replay = true
			res, err = store.GetResource(ctx, tx, rec.ResultID)
			if errors.Is(err, store.ErrNotFound) {
				return apperr.NotFound("resource not found")
			}
			return err
		}
		res, err = store.InsertResource(ctx, tx, store.Resource{
			ID: rec.ResultID, TenantID: m.GetTenantId(), Name: m.GetName(), Description: m.GetDescription(),
			CreatedByUserID: p.UserID, LoadBalancing: mcpav1.LoadBalancing_LOAD_BALANCING_ROUND_ROBIN,
		})
		if err != nil {
			return nameConflict(err)
		}
		if _, err := store.PutACL(ctx, tx, store.ACLEntry{
			ResourceID: res.ID, PrincipalKind: mcpav1.PrincipalKind_PRINCIPAL_KIND_USER, PrincipalID: p.UserID,
			Role: roleManage, CreatedByUserID: p.UserID,
		}); err != nil {
			return err
		}
		return WriteAudit(ctx, tx, p, AuditInput{
			TenantID: res.TenantID, ResourceID: res.ID, Action: mcpav1.AuditAction_AUDIT_ACTION_RESOURCE_CREATED, TargetID: res.ID.String(),
		})
	})
	if err != nil {
		return nil, dbErr(err)
	}
	role := roleManage
	if replay {
		if role, err = s.authz.EffectiveRole(ctx, s.db.Pool, res.TenantID, res.ID, p.UserID); err != nil {
			return nil, dbErr(err)
		}
		// A replay returns the resource's current state. The request key is
		// not a read grant: a creator who has since lost every role gets the
		// same NOT_FOUND as GetResource.
		if role == mcpav1.ResourceRole_RESOURCE_ROLE_UNSPECIFIED {
			return nil, apperr.NotFound("resource not found")
		}
	} else {
		s.log.InfoContext(ctx, "resource created", "resource_id", res.ID, "actor_kind", p.Kind.String())
	}
	return connect.NewResponse(&mcpav1.CreateResourceResponse{Metadata: apperr.Metadata(ctx), Resource: s.resourceProto(res, role)}), nil
}

// GetResource returns one resource.
func (s *Service) GetResource(ctx context.Context, req *connect.Request[mcpav1.GetResourceRequest]) (*connect.Response[mcpav1.GetResourceResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	acc, err := s.authz.Resource(ctx, p, req.Msg.GetResourceId(), roleInvoke, authz.AccessOptions{AllowDeleted: true})
	if err != nil {
		return nil, err
	}
	if acc.Resource.Deleted() {
		return nil, apperr.NotFound("resource not found")
	}
	return connect.NewResponse(&mcpav1.GetResourceResponse{Metadata: apperr.Metadata(ctx), Resource: s.resourceProto(acc.Resource, acc.Role)}), nil
}

// ListResources lists live resources on which the caller has a role.
func (s *Service) ListResources(ctx context.Context, req *connect.Request[mcpav1.ListResourcesRequest]) (*connect.Response[mcpav1.ListResourcesResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	tenant := req.Msg.GetTenantId()
	page, err := ParsePage(req.Msg.GetPage())
	if err != nil {
		return nil, err
	}
	if err := s.authz.Capability(ctx, p, tenant, delcore.CapabilityAccessResources); err != nil {
		return nil, err
	}
	if err := s.authz.RequireGroups(ctx, p, tenant); err != nil {
		return nil, err
	}
	groups, err := s.authz.UserGroups(ctx, tenant, p.UserID)
	if err != nil {
		return nil, dbErr(err)
	}
	rows, err := store.ListResourcesForPrincipal(ctx, s.db.Pool, tenant, p.UserID, groups, page.After, page.Size+1)
	if err != nil {
		return nil, dbErr(err)
	}
	out := &mcpav1.ListResourcesResponse{Metadata: apperr.Metadata(ctx)}
	if len(rows) > page.Size {
		rows = rows[:page.Size]
		out.NextAfterId = rows[len(rows)-1].ID.String()
	}
	for _, r := range rows {
		out.Resources = append(out.Resources, s.resourceProto(r.Resource, authz.Strongest(r.Roles...)))
	}
	return connect.NewResponse(out), nil
}

// UpdateResource changes the name and description.
func (s *Service) UpdateResource(ctx context.Context, req *connect.Request[mcpav1.UpdateResourceRequest]) (*connect.Response[mcpav1.UpdateResourceResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	m := req.Msg
	if err := validateName("name", m.GetName()); err != nil {
		return nil, err
	}
	if err := validateDescription(m.GetDescription()); err != nil {
		return nil, err
	}
	acc, err := s.authz.Resource(ctx, p, m.GetResourceId(), roleManage, authz.AccessOptions{})
	if err != nil {
		return nil, err
	}
	var out store.Resource
	err = s.mutate(ctx, p, acc.Resource.ID, roleManage, func(tx pgx.Tx, r store.Resource, _ mcpav1.ResourceRole) error {
		if err := store.UpdateResource(ctx, tx, r.ID, m.GetName(), m.GetDescription()); err != nil {
			return nameConflict(err)
		}
		details := map[string]string{}
		if r.Name != m.GetName() {
			details["name_changed"] = "true"
		}
		if r.Description != m.GetDescription() {
			details["description_changed"] = "true"
		}
		if err := WriteAudit(ctx, tx, p, AuditInput{
			TenantID: r.TenantID, ResourceID: r.ID, Action: mcpav1.AuditAction_AUDIT_ACTION_RESOURCE_UPDATED, TargetID: r.ID.String(), Details: details,
		}); err != nil {
			return err
		}
		out, err = store.GetResource(ctx, tx, r.ID)
		return err
	})
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&mcpav1.UpdateResourceResponse{Metadata: apperr.Metadata(ctx), Resource: s.resourceProto(out, acc.Role)}), nil
}

// DeleteResource turns a resource into a read-only tombstone. Credentials,
// settings, OAuth flows and call keys are deleted immediately.
func (s *Service) DeleteResource(ctx context.Context, req *connect.Request[mcpav1.DeleteResourceRequest]) (*connect.Response[mcpav1.DeleteResourceResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	acc, err := s.authz.Resource(ctx, p, req.Msg.GetResourceId(), roleManage, authz.AccessOptions{})
	if err != nil {
		return nil, err
	}
	var deleted store.DeletedResource
	err = s.mutate(ctx, p, acc.Resource.ID, roleManage, func(tx pgx.Tx, r store.Resource, _ mcpav1.ResourceRole) error {
		if req.Msg.GetConfirmName() != r.Name {
			return apperr.FailedPrecondition("confirm_name must equal the resource name")
		}
		var err error
		if deleted, err = store.DeleteResource(ctx, tx, r.ID); err != nil {
			return err
		}
		if err := s.credentialsChanging(ctx, tx, r.ID); err != nil {
			return err
		}
		return WriteAudit(ctx, tx, p, AuditInput{
			TenantID: r.TenantID, ResourceID: r.ID, Action: mcpav1.AuditAction_AUDIT_ACTION_RESOURCE_DELETED, TargetID: r.ID.String(),
			Details: map[string]string{
				"credentials_deleted": strconv.Itoa(len(deleted.CredentialIDs)),
				"call_keys_deleted":   strconv.Itoa(len(deleted.CallKeyIDs)),
			},
		})
	})
	if err != nil {
		return nil, err
	}
	if l := s.getHooks().Credentials; l != nil {
		l.ResourceDeleted(ctx, acc.Resource.ID, deleted.CredentialIDs)
	}
	s.log.InfoContext(ctx, "resource deleted", "resource_id", acc.Resource.ID, "credentials", len(deleted.CredentialIDs), "call_keys", len(deleted.CallKeyIDs))
	return connect.NewResponse(&mcpav1.DeleteResourceResponse{Metadata: apperr.Metadata(ctx)}), nil
}

// UpdateRouting replaces the routing settings.
func (s *Service) UpdateRouting(ctx context.Context, req *connect.Request[mcpav1.UpdateRoutingRequest]) (*connect.Response[mcpav1.UpdateRoutingResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	routing := req.Msg.GetRouting()
	if routing == nil {
		return nil, apperr.InvalidInput("routing is required")
	}
	lb := routing.GetLoadBalancing()
	switch lb {
	case mcpav1.LoadBalancing_LOAD_BALANCING_UNSPECIFIED:
		lb = mcpav1.LoadBalancing_LOAD_BALANCING_ROUND_ROBIN
	case mcpav1.LoadBalancing_LOAD_BALANCING_ROUND_ROBIN:
	default:
		return nil, apperr.InvalidInput("routing.load_balancing is not supported")
	}
	if len(routing.GetModelAllowlist()) > maxAllowlist {
		return nil, apperr.InvalidInput("routing.model_allowlist may contain at most 200 entries")
	}
	allowlist, err := credential.NormalizeModels(routing.GetModelAllowlist(), "routing.model_allowlist")
	if err != nil {
		return nil, apperr.InvalidInput(err.Error())
	}
	acc, err := s.authz.Resource(ctx, p, req.Msg.GetResourceId(), roleManage, authz.AccessOptions{})
	if err != nil {
		return nil, err
	}
	var out store.Resource
	err = s.mutate(ctx, p, acc.Resource.ID, roleManage, func(tx pgx.Tx, r store.Resource, _ mcpav1.ResourceRole) error {
		if err := store.UpdateRouting(ctx, tx, r.ID, lb, allowlist); err != nil {
			return err
		}
		if err := WriteAudit(ctx, tx, p, AuditInput{
			TenantID: r.TenantID, ResourceID: r.ID, Action: mcpav1.AuditAction_AUDIT_ACTION_ROUTING_CHANGED, TargetID: r.ID.String(),
			Details: map[string]string{"load_balancing": lb.String(), "model_allowlist_count": strconv.Itoa(len(allowlist))},
		}); err != nil {
			return err
		}
		var err error
		out, err = store.GetResource(ctx, tx, r.ID)
		return err
	})
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&mcpav1.UpdateRoutingResponse{Metadata: apperr.Metadata(ctx), Resource: s.resourceProto(out, acc.Role)}), nil
}

// ListAuditEvents lists a resource's audit events, newest first. Managers
// can read the audit log of a deleted resource.
func (s *Service) ListAuditEvents(ctx context.Context, req *connect.Request[mcpav1.ListAuditEventsRequest]) (*connect.Response[mcpav1.ListAuditEventsResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	page, err := ParsePage(req.Msg.GetPage())
	if err != nil {
		return nil, err
	}
	acc, err := s.authz.Resource(ctx, p, req.Msg.GetResourceId(), roleManage, authz.AccessOptions{AllowDeleted: true})
	if err != nil {
		return nil, err
	}
	events, err := store.ListAudit(ctx, s.db.Pool, acc.Resource.ID, page.After, page.Size+1)
	if err != nil {
		return nil, dbErr(err)
	}
	out := &mcpav1.ListAuditEventsResponse{Metadata: apperr.Metadata(ctx)}
	if len(events) > page.Size {
		events = events[:page.Size]
		out.NextAfterId = events[len(events)-1].ID.String()
	}
	for _, e := range events {
		out.Events = append(out.Events, auditProto(e))
	}
	return connect.NewResponse(out), nil
}

// ListTenantAuditEvents lists a tenant's tenant-level audit events (tenant
// budget changes), newest first. They have no owning resource, so the tenant
// budget capability governs them and no resource access is granted.
func (s *Service) ListTenantAuditEvents(ctx context.Context, req *connect.Request[mcpav1.ListTenantAuditEventsRequest]) (*connect.Response[mcpav1.ListTenantAuditEventsResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	page, err := ParsePage(req.Msg.GetPage())
	if err != nil {
		return nil, err
	}
	tenant := req.Msg.GetTenantId()
	if err := s.authz.Capability(ctx, p, tenant, delcore.CapabilityManageTenantBudget); err != nil {
		return nil, err
	}
	events, err := store.ListTenantAudit(ctx, s.db.Pool, tenant, page.After, page.Size+1)
	if err != nil {
		return nil, dbErr(err)
	}
	out := &mcpav1.ListTenantAuditEventsResponse{Metadata: apperr.Metadata(ctx)}
	if len(events) > page.Size {
		events = events[:page.Size]
		out.NextAfterId = events[len(events)-1].ID.String()
	}
	for _, e := range events {
		out.Events = append(out.Events, auditProto(e))
	}
	return connect.NewResponse(out), nil
}

// parseOptionalID parses a canonical UUID field.
func parseID(v, field string) (uuid.UUID, error) { return authz.ParseID(v, field) }
