// SPDX-License-Identifier: FSL-1.1-ALv2

package rpc

import (
	"errors"
	"fmt"
	"slices"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/delcore-api/internal/accounting"
	"github.com/delinoio/apps/servers/delcore-api/internal/features"
	"github.com/delinoio/apps/servers/delcore-api/internal/postgres"
	"github.com/jackc/pgx/v5"
	"google.golang.org/protobuf/types/known/timestamppb"
)

// featureQuery is a validated feature request of the calling service. A nil
// query means the request named no feature. An unknown feature has no ID.
type featureQuery struct {
	key       string
	id        string
	kind      pb.FeatureKind
	valueType pb.ConfigurationValueType
	meterID   string
	groupKey  string
}

// featureRequest validates the feature key and group values before any tenant
// data is read, so malformed requests fail the same way for every tenant.
// Feature definitions are service-scoped catalog data: a service can only
// authorize its own features.
func (c *call) featureRequest(q *pb.AuthorizeRequest) (*featureQuery, error) {
	if q.FeatureKey == "" {
		if len(q.GroupValues) != 0 {
			return nil, reasoned(connect.CodeInvalidArgument, pb.ErrorReason_ERROR_REASON_INVALID_ATTRIBUTE, "group values require a feature key")
		}
		return nil, nil
	}
	if !catalogKeyPattern.MatchString(q.FeatureKey) {
		return nil, invalid("invalid feature key")
	}
	values, err := attributes(q.GroupValues)
	if err != nil {
		return nil, err
	}
	f := &featureQuery{key: q.FeatureKey}
	var kind, valueType int32
	var meterKey string
	err = c.tx.QueryRow(c.ctx, "SELECT id::text,kind,value_type,meter_key FROM features WHERE service_id=$1 AND key=$2", c.service, q.FeatureKey).Scan(&f.id, &kind, &valueType, &meterKey)
	if errors.Is(err, pgx.ErrNoRows) {
		// An unknown feature is a typed block, not an error, so a consumer that
		// deploys ahead of its catalog fails closed with a diagnosable reason.
		return f, nil
	}
	if err != nil {
		return nil, err
	}
	f.kind, f.valueType = pb.FeatureKind(kind), pb.ConfigurationValueType(valueType)
	if f.kind != pb.FeatureKind_FEATURE_KIND_METERED {
		if len(values) != 0 {
			return nil, reasoned(connect.CodeInvalidArgument, pb.ErrorReason_ERROR_REASON_INVALID_ATTRIBUTE, "only metered features take group values")
		}
		return f, nil
	}
	var raw []byte
	if err = c.tx.QueryRow(c.ctx, `SELECT ce.id::text,cv.definition FROM catalog_entities ce LEFT JOIN catalog_versions cv ON cv.id=ce.active_version_id
 WHERE ce.kind='meter' AND ce.scope=$1 AND ce.key=$2`, c.service, meterKey).Scan(&f.meterID, &raw); err != nil {
		// Feature declarations require an existing meter entity and catalog
		// entities are never deleted, so a missing meter is inconsistent state.
		return nil, fmt.Errorf("metered feature meter is missing: %w", err)
	}
	definition, err := features.DecodeMeter(raw)
	if err != nil {
		return nil, err
	}
	if f.groupKey, err = features.GroupKey(definition.GroupBy, values); err != nil {
		return nil, reasoned(connect.CodeInvalidArgument, pb.ErrorReason_ERROR_REASON_INVALID_ATTRIBUTE, err.Error())
	}
	return f, nil
}

// authorizeFeature appends the feature result after the unchanged permission
// and billing checks. Grants come from the plan and add-on versions pinned for
// the tenant at the request time, so catalog edits never change existing
// selections. Metered usage is the last applied aggregate of the current
// billing period; the check never blocks because processing lags and never
// reserves capacity.
func (c *call) authorizeFeature(f *featureQuery, tenant string, o *pb.AuthorizeResponse) error {
	if f == nil {
		return nil
	}
	if f.id == "" {
		o.Reasons = append(o.Reasons, pb.BlockReason_BLOCK_REASON_FEATURE_UNKNOWN)
		c.s.Log.InfoContext(c.ctx, "feature authorization evaluated", "category", "feature_authorization", "correlation_id", c.correlation, "service_id", c.service, "feature_key", f.key, "reason", pb.BlockReason_BLOCK_REASON_FEATURE_UNKNOWN.String())
		return nil
	}
	rows, err := c.tx.Query(c.ctx, `SELECT cvf.value FROM tenant_catalog_selections cs JOIN catalog_version_features cvf ON cvf.version_id=cs.version_id
 WHERE cs.tenant_id=$1 AND cvf.feature_id=$2 AND cs.starts_at<=$3 AND (cs.ends_at IS NULL OR cs.ends_at>$3) ORDER BY cs.starts_at,cs.id`, tenant, f.id, c.now)
	if err != nil {
		return err
	}
	var raws [][]byte
	for rows.Next() {
		var raw []byte
		if err = rows.Scan(&raw); err != nil {
			rows.Close()
			return err
		}
		raws = append(raws, raw)
	}
	rows.Close()
	if err = rows.Err(); err != nil {
		return err
	}
	if len(raws) > 1 {
		// Subscription changes reject duplicate feature grants; this only
		// guards rows written before that rule and fails closed.
		c.s.Log.WarnContext(c.ctx, "conflicting pinned feature grants", "category", "feature_authorization", "correlation_id", c.correlation, "service_id", c.service, "feature_key", f.key, "grants", len(raws))
	}
	grants := make([]features.Grant, 0, len(raws))
	for _, raw := range raws {
		grant, err := features.DecodeGrant(f.kind, f.valueType, raw)
		if err != nil {
			return err
		}
		grants = append(grants, grant)
	}
	result := &pb.FeatureAuthorization{FeatureId: f.id, Kind: f.kind}
	applied := accounting.Zero()
	if len(grants) == 1 && f.kind == pb.FeatureKind_FEATURE_KIND_METERED {
		start, end, err := postgres.BillingPeriod(c.ctx, c.tx, tenant, c.now)
		if errors.Is(err, postgres.ErrPeriodUnavailable) {
			// A paid plan whose confirmed period ended before a renewal was
			// recorded has no current period, and ingestion rejects usage in it.
			// The limit cannot be measured, so the feature fails closed with the
			// renewal reason; a paid check that already reported it is unchanged.
			result.Entitled, result.Limit = true, grants[0].Limit.String()
			if !slices.Contains(o.Reasons, pb.BlockReason_BLOCK_REASON_RENEWAL) {
				o.Reasons = append(o.Reasons, pb.BlockReason_BLOCK_REASON_RENEWAL)
			}
			o.Feature = result
			c.s.Log.InfoContext(c.ctx, "feature authorization evaluated", "category", "feature_authorization", "correlation_id", c.correlation, "service_id", c.service, "feature_key", f.key, "feature_kind", f.kind.String(), "entitled", true, "reason", pb.BlockReason_BLOCK_REASON_RENEWAL.String(), "billing_period", "unavailable")
			return nil
		}
		if err != nil {
			return err
		}
		var text string
		err = c.tx.QueryRow(c.ctx, "SELECT value::text FROM metering_aggregates WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3 AND group_key=$4", tenant, f.meterID, start, f.groupKey).Scan(&text)
		switch {
		case errors.Is(err, pgx.ErrNoRows):
		case err != nil:
			return err
		default:
			if applied, err = accounting.Parse(text); err != nil {
				return fmt.Errorf("stored aggregate value: %w", err)
			}
		}
		result.Limit, result.Usage = grants[0].Limit.String(), applied.String()
		result.PeriodStart, result.PeriodEnd = timestamppb.New(start), timestamppb.New(end)
	}
	decision := features.Evaluate(grants, applied)
	result.Entitled = decision.Entitled
	if decision.Entitled && f.kind == pb.FeatureKind_FEATURE_KIND_CONFIGURATION {
		result.Configuration = grants[0].Configuration
	}
	if decision.Reason != pb.BlockReason_BLOCK_REASON_UNSPECIFIED {
		o.Reasons = append(o.Reasons, decision.Reason)
	}
	o.Feature = result
	c.s.Log.InfoContext(c.ctx, "feature authorization evaluated", "category", "feature_authorization", "correlation_id", c.correlation, "service_id", c.service, "feature_key", f.key, "feature_kind", f.kind.String(), "entitled", decision.Entitled, "reason", decision.Reason.String())
	return nil
}
