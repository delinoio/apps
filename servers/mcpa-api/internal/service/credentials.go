// SPDX-License-Identifier: FSL-1.1-ALv2

package service

import (
	"context"
	"errors"

	"connectrpc.com/connect"
	"github.com/jackc/pgx/v5"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/authz"
	"github.com/delinoio/apps/servers/mcpa-api/internal/credential"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

// ListCredentials lists a resource's credentials without secrets.
func (s *Service) ListCredentials(ctx context.Context, req *connect.Request[mcpav1.ListCredentialsRequest]) (*connect.Response[mcpav1.ListCredentialsResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	acc, err := s.authz.Resource(ctx, p, req.Msg.GetResourceId(), roleManage, authz.AccessOptions{})
	if err != nil {
		return nil, err
	}
	creds, err := store.ListCredentials(ctx, s.db.Pool, acc.Resource.ID)
	if err != nil {
		return nil, dbErr(err)
	}
	out := &mcpav1.ListCredentialsResponse{Metadata: apperr.Metadata(ctx)}
	for _, c := range creds {
		out.Credentials = append(out.Credentials, CredentialProto(c))
	}
	return connect.NewResponse(out), nil
}

// RegisterApiKeyCredential stores an encrypted provider API key.
func (s *Service) RegisterApiKeyCredential(ctx context.Context, req *connect.Request[mcpav1.RegisterApiKeyCredentialRequest]) (*connect.Response[mcpav1.RegisterApiKeyCredentialResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	m := req.Msg
	if !credential.SupportsAPIKey(m.GetProvider()) {
		return nil, apperr.InvalidInput("provider does not accept API keys")
	}
	if err := validateName("name", m.GetName()); err != nil {
		return nil, err
	}
	if err := credential.ValidateAPIKey(m.GetApiKey()); err != nil {
		return nil, apperr.InvalidInput(err.Error())
	}
	models, err := credential.NormalizeModels(m.GetModels(), "models")
	if err != nil {
		return nil, apperr.InvalidInput(err.Error())
	}
	if m.GetBaseUrl() != "" {
		if err := credential.ValidateUpstreamURL(m.GetBaseUrl()); err != nil {
			return nil, apperr.InvalidInput(err.Error())
		}
	}
	if m.GetProvider() == mcpav1.Provider_PROVIDER_OPENAI_COMPATIBLE {
		if m.GetBaseUrl() == "" {
			return nil, apperr.InvalidInput("base_url is required for OpenAI-compatible providers")
		}
		if len(models) == 0 {
			return nil, apperr.InvalidInput("models must list at least one model for OpenAI-compatible providers")
		}
	}
	acc, err := s.authz.Resource(ctx, p, m.GetResourceId(), roleManage, authz.AccessOptions{})
	if err != nil {
		return nil, err
	}
	c := store.Credential{
		ID: store.NewID(), ResourceID: acc.Resource.ID, TenantID: acc.Resource.TenantID, Provider: m.GetProvider(),
		Kind: mcpav1.CredentialKind_CREDENTIAL_KIND_API_KEY, Name: m.GetName(), SecretHint: credential.SecretHint(m.GetApiKey()),
		BaseURL: m.GetBaseUrl(), Models: models, CreatedByUserID: p.UserID,
	}
	sealed, err := credential.Seal(s.keys, c, credential.Payload{Kind: credential.KindAPIKey, APIKey: m.GetApiKey()})
	if err != nil {
		return nil, apperr.Internal(err)
	}
	err = s.mutate(ctx, p, acc.Resource.ID, roleManage, func(tx pgx.Tx, r store.Resource, _ mcpav1.ResourceRole) error {
		var err error
		if c, err = store.InsertCredential(ctx, tx, c, sealed); err != nil {
			return err
		}
		return WriteAudit(ctx, tx, p, AuditInput{
			TenantID: r.TenantID, ResourceID: r.ID, Action: mcpav1.AuditAction_AUDIT_ACTION_CREDENTIAL_REGISTERED, TargetID: c.ID.String(),
			Details: map[string]string{"provider": c.Provider.String(), "kind": c.Kind.String()},
		})
	})
	if err != nil {
		return nil, err
	}
	if l := s.getHooks().Credentials; l != nil {
		l.CredentialRegistered(ctx, c.ResourceID, c.ID)
	}
	s.log.InfoContext(ctx, "credential registered", "resource_id", c.ResourceID, "credential_id", c.ID, "provider", c.Provider.String())
	return connect.NewResponse(&mcpav1.RegisterApiKeyCredentialResponse{Metadata: apperr.Metadata(ctx), Credential: CredentialProto(c)}), nil
}

// RemoveCredential deletes a credential. The data plane stops using it for
// new calls immediately.
func (s *Service) RemoveCredential(ctx context.Context, req *connect.Request[mcpav1.RemoveCredentialRequest]) (*connect.Response[mcpav1.RemoveCredentialResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	credID, err := parseID(req.Msg.GetCredentialId(), "credential_id")
	if err != nil {
		return nil, err
	}
	acc, err := s.authz.Resource(ctx, p, req.Msg.GetResourceId(), roleManage, authz.AccessOptions{})
	if err != nil {
		return nil, err
	}
	var removed store.Credential
	err = s.mutate(ctx, p, acc.Resource.ID, roleManage, func(tx pgx.Tx, r store.Resource, _ mcpav1.ResourceRole) error {
		var err error
		removed, err = store.DeleteCredential(ctx, tx, r.ID, credID)
		if errors.Is(err, store.ErrNotFound) {
			return apperr.NotFound("credential not found")
		}
		if err != nil {
			return err
		}
		return WriteAudit(ctx, tx, p, AuditInput{
			TenantID: r.TenantID, ResourceID: r.ID, Action: mcpav1.AuditAction_AUDIT_ACTION_CREDENTIAL_REMOVED, TargetID: removed.ID.String(),
			Details: map[string]string{"provider": removed.Provider.String(), "kind": removed.Kind.String()},
		})
	})
	if err != nil {
		return nil, err
	}
	if l := s.getHooks().Credentials; l != nil {
		l.CredentialRemoved(ctx, removed.ResourceID, removed.ID)
	}
	s.log.InfoContext(ctx, "credential removed", "resource_id", removed.ResourceID, "credential_id", removed.ID)
	return connect.NewResponse(&mcpav1.RemoveCredentialResponse{Metadata: apperr.Metadata(ctx)}), nil
}

func (s *Service) oauthBroker(ctx context.Context, resourceID string) (OAuthBroker, *auth.Principal, authz.Access, error) {
	broker := s.getHooks().OAuth
	p, err := principal(ctx)
	if err != nil {
		return nil, nil, authz.Access{}, err
	}
	acc, err := s.authz.Resource(ctx, p, resourceID, roleManage, authz.AccessOptions{})
	if err != nil {
		return nil, nil, authz.Access{}, err
	}
	if broker == nil {
		return nil, nil, authz.Access{}, apperr.Unimplemented("provider OAuth is not available on this server")
	}
	return broker, p, acc, nil
}

// StartProviderOAuth starts a provider OAuth flow through the broker.
func (s *Service) StartProviderOAuth(ctx context.Context, req *connect.Request[mcpav1.StartProviderOAuthRequest]) (*connect.Response[mcpav1.StartProviderOAuthResponse], error) {
	if !credential.SupportsOAuth(req.Msg.GetProvider()) {
		return nil, apperr.InvalidInput("provider does not support OAuth accounts")
	}
	if err := validateName("name", req.Msg.GetName()); err != nil {
		return nil, err
	}
	broker, p, acc, err := s.oauthBroker(ctx, req.Msg.GetResourceId())
	if err != nil {
		return nil, err
	}
	flow, err := broker.StartProviderOAuth(ctx, p, acc.Resource, req.Msg)
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&mcpav1.StartProviderOAuthResponse{Metadata: apperr.Metadata(ctx), Flow: flow}), nil
}

// CompleteProviderOAuth completes an authorization-code flow.
func (s *Service) CompleteProviderOAuth(ctx context.Context, req *connect.Request[mcpav1.CompleteProviderOAuthRequest]) (*connect.Response[mcpav1.CompleteProviderOAuthResponse], error) {
	if _, err := parseID(req.Msg.GetFlowId(), "flow_id"); err != nil {
		return nil, err
	}
	broker, p, acc, err := s.oauthBroker(ctx, req.Msg.GetResourceId())
	if err != nil {
		return nil, err
	}
	flow, err := broker.CompleteProviderOAuth(ctx, p, acc.Resource, req.Msg)
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&mcpav1.CompleteProviderOAuthResponse{Metadata: apperr.Metadata(ctx), Flow: flow}), nil
}

// GetProviderOAuthFlow reads or polls a flow.
func (s *Service) GetProviderOAuthFlow(ctx context.Context, req *connect.Request[mcpav1.GetProviderOAuthFlowRequest]) (*connect.Response[mcpav1.GetProviderOAuthFlowResponse], error) {
	if _, err := parseID(req.Msg.GetFlowId(), "flow_id"); err != nil {
		return nil, err
	}
	broker, p, acc, err := s.oauthBroker(ctx, req.Msg.GetResourceId())
	if err != nil {
		return nil, err
	}
	flow, err := broker.GetProviderOAuthFlow(ctx, p, acc.Resource, req.Msg)
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&mcpav1.GetProviderOAuthFlowResponse{Metadata: apperr.Metadata(ctx), Flow: flow}), nil
}
