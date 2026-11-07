// SPDX-License-Identifier: FSL-1.1-ALv2

package service

import (
	"context"
	"testing"

	"connectrpc.com/connect"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
)

// Base URL validation runs before any authorization or database access, so a
// zero Service is enough to observe the rejection.
func TestRegisterApiKeyCredentialRejectsEmptyQueryOrFragmentBaseURL(t *testing.T) {
	ctx := auth.WithPrincipal(context.Background(), auth.NewUserPrincipal(mcpav1.ActorKind_ACTOR_KIND_CONSOLE, "u1", "sub-u1", nil))
	for _, base := range []string{"https://x.example/v1?", "https://x.example/v1#", "https://x.example/v1?#"} {
		_, err := (&Service{}).RegisterApiKeyCredential(ctx, connect.NewRequest(&mcpav1.RegisterApiKeyCredentialRequest{
			Provider: mcpav1.Provider_PROVIDER_OPENAI_COMPATIBLE,
			Name:     "n",
			ApiKey:   "sk-1234567890abcd",
			BaseUrl:  base,
			Models:   []string{"m"},
		}))
		if err == nil || apperr.As(err).Reason != mcpav1.ErrorReason_ERROR_REASON_INVALID_INPUT {
			t.Errorf("%s: got %v, want INVALID_INPUT", base, err)
		}
	}
}
