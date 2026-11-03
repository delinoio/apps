// SPDX-License-Identifier: FSL-1.1-ALv2

package service

import (
	"context"
	"errors"

	"connectrpc.com/connect"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
)

// userTokenErr maps a failure to obtain or use a Delcore user token.
func userTokenErr(err error) error {
	var ae *apperr.Error
	if errors.As(err, &ae) {
		return ae
	}
	if errors.Is(err, auth.ErrNoUserToken) || delcore.IsUnauthenticated(err) {
		return apperr.Unauthenticated("the Delcore session expired; sign in again")
	}
	return apperr.DependencyUnavailable(err)
}

// GetViewer returns the caller and the tenants it can act in.
func (s *Service) GetViewer(ctx context.Context, _ *connect.Request[mcpav1.GetViewerRequest]) (*connect.Response[mcpav1.GetViewerResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	viewer := &mcpav1.Viewer{UserId: p.UserID, ActorKind: p.Kind}
	if p.IsPersonalKey() {
		// A personal key acts in exactly one tenant. Without a user token mcpa
		// cannot read the tenant name.
		viewer.Tenants = []*mcpav1.Tenant{{Id: p.KeyTenantID}}
		return connect.NewResponse(&mcpav1.GetViewerResponse{Metadata: apperr.Metadata(ctx), Viewer: viewer}), nil
	}
	token, err := p.DelcoreToken(ctx)
	if err != nil {
		return nil, userTokenErr(err)
	}
	user, err := s.dc.GetCurrentUser(ctx, token)
	if err != nil {
		return nil, userTokenErr(err)
	}
	if user.UserID != p.UserID {
		// The Delcore token no longer belongs to the session user.
		return nil, apperr.Unauthenticated("the session no longer matches the Delcore user; sign in again")
	}
	for _, t := range user.Tenants {
		viewer.Tenants = append(viewer.Tenants, &mcpav1.Tenant{Id: t.GetId(), Name: t.GetName(), Personal: t.GetPersonal()})
	}
	return connect.NewResponse(&mcpav1.GetViewerResponse{Metadata: apperr.Metadata(ctx), Viewer: viewer}), nil
}

// ListTenantGroups lists the tenant's Delcore groups for ACL editing. User
// sessions refresh the group snapshot from Delcore at most once per
// MCPA_AUTHORIZE_CACHE_TTL for the tenant and read it; personal keys read the
// latest snapshot.
func (s *Service) ListTenantGroups(ctx context.Context, req *connect.Request[mcpav1.ListTenantGroupsRequest]) (*connect.Response[mcpav1.ListTenantGroupsResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	tenant := req.Msg.GetTenantId()
	if err := s.authz.Capability(ctx, p, tenant, delcore.CapabilityAccessResources); err != nil {
		return nil, err
	}
	groups, err := s.authz.SnapshotGroups(ctx, p, tenant)
	if err != nil {
		return nil, userTokenErr(err)
	}
	out := &mcpav1.ListTenantGroupsResponse{Metadata: apperr.Metadata(ctx)}
	for _, g := range groups {
		out.Groups = append(out.Groups, &mcpav1.Group{Id: g.ID, Name: g.Name})
	}
	return connect.NewResponse(out), nil
}
