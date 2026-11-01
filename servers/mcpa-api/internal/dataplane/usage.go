// SPDX-License-Identifier: FSL-1.1-ALv2

package dataplane

import (
	"context"
	"errors"
	"time"

	"github.com/google/uuid"
	"google.golang.org/protobuf/types/known/timestamppb"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/pricing"
	"github.com/delinoio/apps/servers/mcpa-api/internal/service"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

// UsageReader serves usage, call-record and spend reads. The service has
// already authorized the resource IDs of every query.
type UsageReader struct {
	db *store.Store
}

var (
	_ service.UsageReader = (*UsageReader)(nil)
	_ service.SpendReader = (*UsageReader)(nil)
)

func usageFilter(q service.UsageQuery) store.UsageFilter {
	f := store.UsageFilter{TenantID: q.TenantID, ResourceIDs: q.ResourceIDs}
	if q.Filter == nil {
		return f
	}
	f.Models = q.Filter.GetModels()
	f.Providers = q.Filter.GetProviders()
	f.Statuses = q.Filter.GetStatuses()
	for _, raw := range q.Filter.GetCallKeyIds() {
		// The service validated every ID.
		if id, err := uuid.Parse(raw); err == nil {
			f.CallKeyIDs = append(f.CallKeyIDs, id)
		}
	}
	if t := q.Filter.GetStartTime(); t != nil {
		v := t.AsTime()
		f.Start = &v
	}
	if t := q.Filter.GetEndTime(); t != nil {
		v := t.AsTime()
		f.End = &v
	}
	return f
}

func storeErr(err error) error {
	if err == nil {
		return nil
	}
	if errors.Is(err, context.Canceled) {
		return apperr.As(err)
	}
	return apperr.DependencyUnavailable(err)
}

// GetUsage returns the summary over every matching daily aggregate and one
// page of aggregate rows, newest day first. Aggregates are per UTC day, so
// a time range selects every day that overlaps it.
func (u *UsageReader) GetUsage(ctx context.Context, q service.UsageQuery) (*mcpav1.GetUsageResponse, error) {
	f := usageFilter(q)
	totals, err := store.SumUsage(ctx, u.db.Pool, f)
	if err != nil {
		return nil, storeErr(err)
	}
	rows, err := store.ListUsageRows(ctx, u.db.Pool, f, q.Page.After, q.Page.Size+1)
	if err != nil {
		return nil, storeErr(err)
	}
	total := totals.RequestBytes + totals.ResponseBytes
	out := &mcpav1.GetUsageResponse{Summary: &mcpav1.UsageSummary{
		CallCount: totals.CallCount, RequestBytes: totals.RequestBytes, ResponseBytes: totals.ResponseBytes,
		TotalBytes: total, TotalGib: pricing.GiB(total), EstimatedUsd: pricing.EstimateUSD(total),
	}}
	if len(rows) > q.Page.Size {
		rows = rows[:q.Page.Size]
		out.NextAfterId = rows[len(rows)-1].ID.String()
	}
	for _, r := range rows {
		out.Rows = append(out.Rows, &mcpav1.UsageRow{
			Day: timestamppb.New(r.Day), ResourceId: r.ResourceID.String(), Model: r.Model, Provider: r.Provider,
			CallKeyId: r.CallKeyID.String(), Status: r.Status, CallCount: r.CallCount,
			RequestBytes: r.RequestBytes, ResponseBytes: r.ResponseBytes,
		})
	}
	return out, nil
}

// ListCallRecords returns one page of call records, newest first.
func (u *UsageReader) ListCallRecords(ctx context.Context, q service.UsageQuery) (*mcpav1.ListCallRecordsResponse, error) {
	records, err := store.ListCallRecords(ctx, u.db.Pool, usageFilter(q), q.Page.After, q.Page.Size+1)
	if err != nil {
		return nil, storeErr(err)
	}
	out := &mcpav1.ListCallRecordsResponse{}
	if len(records) > q.Page.Size {
		records = records[:q.Page.Size]
		out.NextAfterId = records[len(records)-1].ID.String()
	}
	for _, r := range records {
		out.Records = append(out.Records, &mcpav1.CallRecord{
			Id: r.ID.String(), ResourceId: r.ResourceID.String(), CallKeyId: r.CallKeyID.String(), Provider: r.Provider,
			Model: r.Model, Status: r.Status, ProviderStatusClass: r.ProviderStatusClass, ErrorClass: r.ErrorClass,
			RequestBytes: r.RequestBytes, ResponseBytes: r.ResponseBytes, LatencyMs: r.LatencyMS, RequestId: r.ID.String(),
			StartTime: timestamppb.New(r.StartTime), Format: r.Format, Streaming: r.Streaming,
		})
	}
	return out, nil
}

// ResourceBilledBytes returns the bytes committed for calls of a resource
// that started in [start, end).
func (u *UsageReader) ResourceBilledBytes(ctx context.Context, resourceID uuid.UUID, start, end time.Time) (int64, error) {
	res, err := store.GetResource(ctx, u.db.Pool, resourceID)
	if err != nil {
		return 0, err
	}
	return store.BilledBytes(ctx, u.db.Pool, res.TenantID, &resourceID, start, end)
}

// TenantBilledBytes returns the bytes committed for calls of a tenant that
// started in [start, end).
func (u *UsageReader) TenantBilledBytes(ctx context.Context, tenantID string, start, end time.Time) (int64, error) {
	return store.BilledBytes(ctx, u.db.Pool, tenantID, nil, start, end)
}
