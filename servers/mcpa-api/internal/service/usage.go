// SPDX-License-Identifier: FSL-1.1-ALv2

package service

import (
	"context"
	"fmt"
	"sort"
	"unicode"
	"unicode/utf8"

	"connectrpc.com/connect"
	"github.com/google/uuid"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/authz"
	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
	"github.com/delinoio/apps/servers/mcpa-api/internal/pricing"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

// readableResources returns the tenant resources, including tombstones, on
// which the principal holds need, intersected with the filter.
func (s *Service) readableResources(ctx context.Context, p *auth.Principal, tenantID string, filter *mcpav1.CallFilter, need mcpav1.ResourceRole) ([]uuid.UUID, error) {
	var requested map[uuid.UUID]bool
	if ids := filter.GetResourceIds(); len(ids) > 0 {
		requested = map[uuid.UUID]bool{}
		for _, raw := range ids {
			id, err := parseID(raw, "filter.resource_ids")
			if err != nil {
				return nil, err
			}
			requested[id] = true
		}
	}
	if err := s.authz.Capability(ctx, p, tenantID, delcore.CapabilityAccessResources); err != nil {
		return nil, err
	}
	if err := s.authz.RequireGroups(ctx, p, tenantID); err != nil {
		return nil, err
	}
	groups, err := s.authz.UserGroups(ctx, tenantID, p.UserID)
	if err != nil {
		return nil, dbErr(err)
	}
	roles, err := store.ResourceIDsForPrincipal(ctx, s.db.Pool, tenantID, p.UserID, groups)
	if err != nil {
		return nil, dbErr(err)
	}
	var out []uuid.UUID
	for id, rs := range roles {
		if requested != nil && !requested[id] {
			continue
		}
		if authz.Satisfies(authz.Strongest(rs...), need) {
			out = append(out, id)
		}
	}
	sort.Slice(out, func(i, j int) bool { return out[i].String() < out[j].String() })
	return out, nil
}

const (
	maxFilterModels   = 100
	maxFilterModelLen = 200
)

// validateFilter rejects filters the store cannot represent. Models must fit
// the call_records.model constraint and PostgreSQL text rules, and enum values
// must be defined members because the store narrows them to int16.
func validateFilter(f *mcpav1.CallFilter) error {
	if f == nil {
		return nil
	}
	if err := validateFilterModels(f.GetModels()); err != nil {
		return err
	}
	for _, p := range f.GetProviders() {
		if _, ok := mcpav1.Provider_name[int32(p)]; !ok {
			return apperr.InvalidInput(fmt.Sprintf("filter.providers contains an unknown provider %d", int32(p)))
		}
	}
	for _, st := range f.GetStatuses() {
		if _, ok := mcpav1.CallStatus_name[int32(st)]; !ok {
			return apperr.InvalidInput(fmt.Sprintf("filter.statuses contains an unknown status %d", int32(st)))
		}
	}
	for _, id := range f.GetCallKeyIds() {
		if _, err := parseID(id, "filter.call_key_ids"); err != nil {
			return err
		}
	}
	if f.GetStartTime() != nil && f.GetStartTime().CheckValid() != nil {
		return apperr.InvalidInput("filter.start_time is not a valid timestamp")
	}
	if f.GetEndTime() != nil && f.GetEndTime().CheckValid() != nil {
		return apperr.InvalidInput("filter.end_time is not a valid timestamp")
	}
	if f.GetStartTime() != nil && f.GetEndTime() != nil && !f.GetStartTime().AsTime().Before(f.GetEndTime().AsTime()) {
		return apperr.InvalidInput("filter.start_time must be before filter.end_time")
	}
	return nil
}

func validateFilterModels(models []string) error {
	if len(models) > maxFilterModels {
		return apperr.InvalidInput(fmt.Sprintf("filter.models has at most %d entries", maxFilterModels))
	}
	for _, m := range models {
		if len(m) == 0 || len(m) > maxFilterModelLen {
			return apperr.InvalidInput(fmt.Sprintf("filter.models entries must be 1 to %d bytes", maxFilterModelLen))
		}
		if !utf8.ValidString(m) {
			return apperr.InvalidInput("filter.models entries must be valid UTF-8")
		}
		for _, r := range m {
			if unicode.IsControl(r) {
				return apperr.InvalidInput("filter.models entries must not contain control characters")
			}
		}
	}
	return nil
}

// GetUsage returns usage over resources where the caller holds at least
// INVOKE.
func (s *Service) GetUsage(ctx context.Context, req *connect.Request[mcpav1.GetUsageRequest]) (*connect.Response[mcpav1.GetUsageResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	page, err := ParsePage(req.Msg.GetPage())
	if err != nil {
		return nil, err
	}
	if err := validateFilter(req.Msg.GetFilter()); err != nil {
		return nil, err
	}
	ids, err := s.readableResources(ctx, p, req.Msg.GetTenantId(), req.Msg.GetFilter(), roleInvoke)
	if err != nil {
		return nil, err
	}
	if len(ids) == 0 {
		return connect.NewResponse(&mcpav1.GetUsageResponse{
			Metadata: apperr.Metadata(ctx),
			Summary:  &mcpav1.UsageSummary{TotalGib: pricing.GiB(0), EstimatedUsd: pricing.EstimateUSD(0)},
		}), nil
	}
	reader := s.getHooks().Usage
	if reader == nil {
		return nil, apperr.Unimplemented("usage views are not available on this server")
	}
	resp, err := reader.GetUsage(ctx, UsageQuery{TenantID: req.Msg.GetTenantId(), ResourceIDs: ids, Filter: req.Msg.GetFilter(), Page: page})
	if err != nil {
		return nil, err
	}
	resp.Metadata = apperr.Metadata(ctx)
	return connect.NewResponse(resp), nil
}

// ListCallRecords returns call records (usage rows and errors) of resources
// where the caller holds MANAGE.
func (s *Service) ListCallRecords(ctx context.Context, req *connect.Request[mcpav1.ListCallRecordsRequest]) (*connect.Response[mcpav1.ListCallRecordsResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	page, err := ParsePage(req.Msg.GetPage())
	if err != nil {
		return nil, err
	}
	if err := validateFilter(req.Msg.GetFilter()); err != nil {
		return nil, err
	}
	ids, err := s.readableResources(ctx, p, req.Msg.GetTenantId(), req.Msg.GetFilter(), roleManage)
	if err != nil {
		return nil, err
	}
	if len(ids) == 0 {
		return connect.NewResponse(&mcpav1.ListCallRecordsResponse{Metadata: apperr.Metadata(ctx)}), nil
	}
	reader := s.getHooks().Usage
	if reader == nil {
		return nil, apperr.Unimplemented("call records are not available on this server")
	}
	resp, err := reader.ListCallRecords(ctx, UsageQuery{TenantID: req.Msg.GetTenantId(), ResourceIDs: ids, Filter: req.Msg.GetFilter(), Page: page})
	if err != nil {
		return nil, err
	}
	resp.Metadata = apperr.Metadata(ctx)
	return connect.NewResponse(resp), nil
}
