// SPDX-License-Identifier: FSL-1.1-ALv2
package api

import (
	"connectrpc.com/connect"
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	core "github.com/delinoio/apps/protos/delcore/v1"
	pb "github.com/delinoio/apps/protos/delitools/v1"
	"github.com/jackc/pgx/v5"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/types/known/timestamppb"
	"strings"
	"time"
)

func (s *Server) keyManager(ctx context.Context, header, tenant string, kind pb.KeyKind, id string) (string, error) {
	if !validID(tenant) || (kind != pb.KeyKind_KEY_KIND_PERSONAL && kind != pb.KeyKind_KEY_KIND_SERVICE) {
		return "", fault(pb.ErrorReason_ERROR_REASON_INVALID_INPUT, id)
	}
	user, e := s.Auth.User(ctx, header)
	if e != nil {
		return "", authFault(e, id)
	}
	mode := core.ConsumerAuthorization_CONSUMER_AUTHORIZATION_PERSONAL_KEYS
	if kind == pb.KeyKind_KEY_KIND_SERVICE {
		mode = core.ConsumerAuthorization_CONSUMER_AUTHORIZATION_SERVICE_KEYS
	}
	if e = s.Auth.Check(ctx, tenant, user, pb.Capability_CAPABILITY_UNSPECIFIED, mode, false); e != nil {
		return "", authFault(e, id)
	}
	return user, nil
}
func (s *Server) CreateKey(ctx context.Context, r *connect.Request[pb.CreateKeyRequest]) (*connect.Response[pb.CreateKeyResponse], error) {
	id := ID()
	q := r.Msg
	user, e := s.keyManager(ctx, r.Header().Get("Authorization"), q.TenantId, q.Kind, id)
	if e != nil {
		return nil, e
	}
	if len(q.Capabilities) == 0 || len(q.Capabilities) > 2 {
		return nil, fault(pb.ErrorReason_ERROR_REASON_INVALID_INPUT, id)
	}
	caps := []int32{}
	seen := map[pb.Capability]bool{}
	for _, cap := range q.Capabilities {
		if capability(cap) == "" || seen[cap] {
			return nil, fault(pb.ErrorReason_ERROR_REASON_INVALID_INPUT, id)
		}
		seen[cap] = true
		caps = append(caps, int32(cap))
	}
	now := s.now()
	var expiry any
	if q.ExpiresAt != nil {
		if q.ExpiresAt.CheckValid() != nil || !q.ExpiresAt.AsTime().After(now) {
			return nil, fault(pb.ErrorReason_ERROR_REASON_INVALID_INPUT, id)
		}
		expiry = q.ExpiresAt.AsTime()
	}
	if q.Kind == pb.KeyKind_KEY_KIND_SERVICE {
		user = ""
	}
	entropy := make([]byte, 32)
	if _, e = rand.Read(entropy); e != nil {
		return nil, fault(pb.ErrorReason_ERROR_REASON_INTERNAL, id)
	}
	secret := "dt_" + base64.RawURLEncoding.EncodeToString(entropy)
	sum := sha256.Sum256([]byte(secret))
	keyID := ID()
	if _, e = s.DB.Pool.Exec(ctx, "INSERT INTO api_keys(id,tenant_id,user_id,kind,capabilities,secret_hash,created_at,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8)", keyID, q.TenantId, user, q.Kind, caps, sum[:], now, expiry); e != nil {
		return nil, fault(pb.ErrorReason_ERROR_REASON_INTERNAL, id)
	}
	return connect.NewResponse(&pb.CreateKeyResponse{RequestId: id, Secret: secret, Key: &pb.ApiKey{Id: keyID, TenantId: q.TenantId, UserId: user, Kind: q.Kind, Capabilities: q.Capabilities, CreatedAt: timestamppb.New(now), ExpiresAt: q.ExpiresAt}}), nil
}
func (s *Server) ListKeys(ctx context.Context, r *connect.Request[pb.ListKeysRequest]) (*connect.Response[pb.ListKeysResponse], error) {
	id := ID()
	q := r.Msg
	user, e := s.keyManager(ctx, r.Header().Get("Authorization"), q.TenantId, q.Kind, id)
	if e != nil {
		return nil, e
	}
	after := q.AfterId
	if after == "" {
		after = "00000000-0000-0000-0000-000000000000"
	} else if !validID(after) {
		return nil, fault(pb.ErrorReason_ERROR_REASON_INVALID_INPUT, id)
	}
	// Pagination bounds are transport safeguards, independent of paid execution limits.
	size := q.PageSize
	if size == 0 {
		size = 50
	}
	if size > 100 {
		return nil, fault(pb.ErrorReason_ERROR_REASON_INVALID_INPUT, id)
	}
	rows, e := s.DB.Pool.Query(ctx, "SELECT id::text,user_id,kind,capabilities,created_at,expires_at,revoked FROM api_keys WHERE tenant_id=$1 AND kind=$2 AND ($2=2 OR user_id=$3) AND id>$4 ORDER BY id LIMIT $5", q.TenantId, q.Kind, user, after, size+1)
	if e != nil {
		return nil, fault(pb.ErrorReason_ERROR_REASON_INTERNAL, id)
	}
	defer rows.Close()
	out := &pb.ListKeysResponse{RequestId: id}
	for rows.Next() {
		k := &pb.ApiKey{TenantId: q.TenantId}
		var caps []int32
		var created time.Time
		var expiry *time.Time
		if e = rows.Scan(&k.Id, &k.UserId, &k.Kind, &caps, &created, &expiry, &k.Revoked); e != nil {
			return nil, fault(pb.ErrorReason_ERROR_REASON_INTERNAL, id)
		}
		k.CreatedAt = timestamppb.New(created)
		if expiry != nil {
			k.ExpiresAt = timestamppb.New(*expiry)
		}
		for _, cap := range caps {
			k.Capabilities = append(k.Capabilities, pb.Capability(cap))
		}
		out.Keys = append(out.Keys, k)
	}
	if rows.Err() != nil {
		return nil, fault(pb.ErrorReason_ERROR_REASON_INTERNAL, id)
	}
	if len(out.Keys) > int(size) {
		out.Keys = out.Keys[:size]
		out.NextAfterId = out.Keys[size-1].Id
	}
	return connect.NewResponse(out), nil
}
func (s *Server) RevokeKey(ctx context.Context, r *connect.Request[pb.RevokeKeyRequest]) (*connect.Response[pb.RevokeKeyResponse], error) {
	id := ID()
	q := r.Msg
	if !validID(q.TenantId) || !validID(q.KeyId) {
		return nil, fault(pb.ErrorReason_ERROR_REASON_INVALID_INPUT, id)
	}
	// User authentication precedes reading key ownership; errors reveal no key metadata.
	user, e := s.Auth.User(ctx, r.Header().Get("Authorization"))
	if e != nil {
		return nil, authFault(e, id)
	}
	var kind pb.KeyKind
	var owner string
	if e = s.DB.Pool.QueryRow(ctx, "SELECT kind,user_id FROM api_keys WHERE tenant_id=$1 AND id=$2", q.TenantId, q.KeyId).Scan(&kind, &owner); e != nil {
		return nil, fault(pb.ErrorReason_ERROR_REASON_FORBIDDEN, id)
	}
	if kind == pb.KeyKind_KEY_KIND_PERSONAL && owner != user {
		return nil, fault(pb.ErrorReason_ERROR_REASON_FORBIDDEN, id)
	}
	if _, e = s.keyManager(ctx, r.Header().Get("Authorization"), q.TenantId, kind, id); e != nil {
		return nil, e
	}
	if _, e = s.DB.Pool.Exec(ctx, "UPDATE api_keys SET revoked=true WHERE id=$1 AND tenant_id=$2", q.KeyId, q.TenantId); e != nil {
		return nil, fault(pb.ErrorReason_ERROR_REASON_INTERNAL, id)
	}
	return connect.NewResponse(&pb.RevokeKeyResponse{RequestId: id}), nil
}
func (s *Server) GetSettings(ctx context.Context, r *connect.Request[pb.GetSettingsRequest]) (*connect.Response[pb.GetSettingsResponse], error) {
	id := ID()
	if _, e := s.Auth.Operator(ctx, r.Header().Get("Authorization")); e != nil {
		return nil, authFault(e, id)
	}
	v, e := s.DB.Settings(ctx)
	if e != nil {
		return nil, fault(pb.ErrorReason_ERROR_REASON_INTERNAL, id)
	}
	return connect.NewResponse(&pb.GetSettingsResponse{RequestId: id, Settings: v}), nil
}
func (s *Server) UpdateSettings(ctx context.Context, r *connect.Request[pb.UpdateSettingsRequest]) (*connect.Response[pb.UpdateSettingsResponse], error) {
	id := ID()
	user, e := s.Auth.Operator(ctx, r.Header().Get("Authorization"))
	if e != nil {
		return nil, authFault(e, id)
	}
	if ValidateSettings(r.Msg.Settings) != nil || len(strings.TrimSpace(r.Msg.Reason)) < 3 || len(r.Msg.Reason) > 1000 {
		return nil, fault(pb.ErrorReason_ERROR_REASON_INVALID_INPUT, id)
	}
	raw, _ := protojson.Marshal(r.Msg.Settings)
	e = begin(ctx, s.DB, func(tx pgx.Tx) error {
		if _, e := tx.Exec(ctx, "UPDATE settings SET value=$1 WHERE singleton", raw); e != nil {
			return e
		}
		_, e := tx.Exec(ctx, "INSERT INTO audit VALUES($1,$2,$3,$4,$5,$6)", ID(), user, "update_settings", r.Msg.Reason, id, s.now())
		return e
	})
	if e != nil {
		return nil, fault(pb.ErrorReason_ERROR_REASON_INTERNAL, id)
	}
	return connect.NewResponse(&pb.UpdateSettingsResponse{RequestId: id}), nil
}
func (s *Server) InspectRequest(ctx context.Context, r *connect.Request[pb.InspectRequestRequest]) (*connect.Response[pb.InspectRequestResponse], error) {
	id := ID()
	if _, e := s.Auth.Operator(ctx, r.Header().Get("Authorization")); e != nil {
		return nil, authFault(e, id)
	}
	if !validID(r.Msg.RequestId) {
		return nil, fault(pb.ErrorReason_ERROR_REASON_INVALID_INPUT, id)
	}
	out := &pb.InspectRequestResponse{RequestId: id, InspectedRequestId: r.Msg.RequestId}
	var created time.Time
	if e := s.DB.Pool.QueryRow(ctx, "SELECT tenant_id::text,capability,state,model,created_at FROM requests WHERE id=$1 AND created_at>$2", r.Msg.RequestId, s.now().Add(-Retention)).Scan(&out.TenantId, &out.Capability, &out.State, &out.Model, &created); e != nil {
		return nil, fault(pb.ErrorReason_ERROR_REASON_INVALID_INPUT, id)
	}
	out.CreatedAt = timestamppb.New(created)
	rows, e := s.DB.Pool.Query(ctx, "SELECT id::text,generation_id,usage IS NOT NULL,delivered,created_at FROM attempts WHERE request_id=$1 AND created_at>$2 ORDER BY created_at,id", r.Msg.RequestId, s.now().Add(-Retention))
	if e != nil {
		return nil, fault(pb.ErrorReason_ERROR_REASON_INTERNAL, id)
	}
	defer rows.Close()
	for rows.Next() {
		a := &pb.Attempt{}
		var at time.Time
		if e = rows.Scan(&a.Id, &a.GenerationId, &a.UsageKnown, &a.Delivered, &at); e != nil {
			return nil, fault(pb.ErrorReason_ERROR_REASON_INTERNAL, id)
		}
		a.CreatedAt = timestamppb.New(at)
		out.Attempts = append(out.Attempts, a)
	}
	if rows.Err() != nil {
		return nil, fault(pb.ErrorReason_ERROR_REASON_INTERNAL, id)
	}
	return connect.NewResponse(out), nil
}
