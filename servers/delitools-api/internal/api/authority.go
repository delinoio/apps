// SPDX-License-Identifier: FSL-1.1-ALv2
package api

import (
	"connectrpc.com/connect"
	"context"
	"errors"
	"github.com/coreos/go-oidc/v3/oidc"
	core "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/protos/delcore/v1/delcorev1connect"
	pb "github.com/delinoio/apps/protos/delitools/v1"
	"golang.org/x/oauth2"
	"slices"
	"strings"
)

type Delcore struct {
	Client           delcorev1connect.DelcoreServiceClient
	Tokens           oauth2.TokenSource
	OperatorVerifier *oidc.IDTokenVerifier
	OperatorRole     string
}

func (d *Delcore) User(ctx context.Context, header string) (string, error) {
	if !strings.HasPrefix(header, "Bearer ") || strings.HasPrefix(header, "Bearer dt_") {
		return "", connect.NewError(connect.CodeUnauthenticated, errors.New("user token required"))
	}
	r := connect.NewRequest(&core.GetCurrentUserRequest{})
	r.Header().Set("Authorization", header)
	v, e := d.Client.GetCurrentUser(ctx, r)
	if e != nil {
		return "", e
	}
	if !validID(v.Msg.UserId) {
		return "", connect.NewError(connect.CodeUnauthenticated, errors.New("user identity unavailable"))
	}
	return v.Msg.UserId, nil
}
func (d *Delcore) Operator(ctx context.Context, header string) (string, error) {
	parts := strings.Fields(header)
	if len(parts) != 2 || parts[0] != "Bearer" || d.OperatorVerifier == nil {
		return "", connect.NewError(connect.CodeUnauthenticated, errors.New("operator token required"))
	}
	token, e := d.OperatorVerifier.Verify(ctx, parts[1])
	if e != nil {
		return "", connect.NewError(connect.CodeUnauthenticated, errors.New("invalid operator token"))
	}
	var claims struct {
		Roles    []string `json:"roles"`
		ClientID string   `json:"client_id"`
	}
	if e = token.Claims(&claims); e != nil || token.Subject == "" || token.Subject == claims.ClientID || !slices.Contains(claims.Roles, d.OperatorRole) {
		return "", connect.NewError(connect.CodePermissionDenied, errors.New("platform operator required"))
	}
	// Audit attribution uses the same delcore user identity as key management.
	return d.User(ctx, header)
}
func (d *Delcore) Check(ctx context.Context, tenant, user string, cap pb.Capability, mode core.ConsumerAuthorization, paid bool) error {
	token, e := d.Tokens.Token()
	if e != nil {
		return connect.NewError(connect.CodeUnavailable, errors.New("service credentials unavailable"))
	}
	r := connect.NewRequest(&core.AuthorizeRequest{TenantId: tenant, UserId: user, Capability: capability(cap), ConsumerAuthorization: mode, Paid: paid})
	r.Header().Set("Authorization", "Bearer "+token.AccessToken)
	v, e := d.Client.Authorize(ctx, r)
	if e != nil {
		return e
	}
	if !v.Msg.Allowed {
		code := connect.CodeFailedPrecondition
		if slices.Contains(v.Msg.Reasons, core.BlockReason_BLOCK_REASON_PERMISSION) {
			code = connect.CodePermissionDenied
		}
		return connect.NewError(code, errors.New("current authorization denied"))
	}
	return nil
}
func (d *Delcore) Report(ctx context.Context, u *core.Usage, id string) error {
	token, e := d.Tokens.Token()
	if e != nil {
		return e
	}
	r := connect.NewRequest(&core.ReportUsageRequest{RequestKey: id, Usage: u})
	r.Header().Set("Authorization", "Bearer "+token.AccessToken)
	_, e = d.Client.ReportUsage(ctx, r)
	return e
}
