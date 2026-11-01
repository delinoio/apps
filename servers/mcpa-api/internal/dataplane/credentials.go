// SPDX-License-Identifier: FSL-1.1-ALv2

package dataplane

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/credential"
	"github.com/delinoio/apps/servers/mcpa-api/internal/crypto"
	"github.com/delinoio/apps/servers/mcpa-api/internal/proxy"
	"github.com/delinoio/apps/servers/mcpa-api/internal/service"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

// Credentials loads decrypted credentials for the engine and re-encrypts
// refreshed OAuth tokens.
type Credentials struct {
	db     *store.Store
	keys   *crypto.Keyring
	log    *slog.Logger
	notify func(ctx context.Context, resourceID uuid.UUID)
}

var (
	_ proxy.CredentialSource = (*Credentials)(nil)
	_ proxy.TokenPersister   = (*Credentials)(nil)
)

// toEngine decrypts a stored credential into the engine form.
func toEngine(keys *crypto.Keyring, c store.Credential, sealed crypto.Sealed) (proxy.Credential, error) {
	p, err := credential.Open(keys, c, sealed)
	if err != nil {
		return proxy.Credential{}, err
	}
	out := proxy.Credential{
		ID: c.ID.String(), ResourceID: c.ResourceID.String(), Provider: c.Provider, Kind: c.Kind,
		Label: c.AccountLabel, BaseURL: c.BaseURL, Models: c.Models,
	}
	switch {
	case c.Kind == mcpav1.CredentialKind_CREDENTIAL_KIND_API_KEY && p.Kind == credential.KindAPIKey:
		out.Secret = []byte(p.APIKey)
	case c.Kind == mcpav1.CredentialKind_CREDENTIAL_KIND_OAUTH && p.Kind == credential.KindOAuth:
		out.Secret = append([]byte(nil), p.OAuth...)
	default:
		return proxy.Credential{}, errors.New("credential payload kind does not match the credential")
	}
	return out, nil
}

// LoadResourceCredentials returns the decrypted credentials of a live
// resource. A credential that cannot be decrypted is skipped and logged.
func (c *Credentials) LoadResourceCredentials(ctx context.Context, resourceID string) ([]proxy.Credential, error) {
	id, err := uuid.Parse(resourceID)
	if err != nil {
		return nil, nil
	}
	stored, sealed, err := store.ListCredentialsSealed(ctx, c.db.Pool, id)
	if err != nil {
		return nil, err
	}
	out := make([]proxy.Credential, 0, len(stored))
	for i, sc := range stored {
		cred, err := toEngine(c.keys, sc, sealed[i])
		if err != nil {
			c.log.Error("credential could not be decrypted", "alert", true, "resource_id", resourceID, "credential_id", sc.ID, "key_id", sealed[i].KeyID, "error", err)
			continue
		}
		out = append(out, cred)
	}
	return out, nil
}

// PersistCredentialSecret stores a refreshed OAuth payload, re-encrypted
// under the active master key. A credential removed in the meantime is not
// recreated.
func (c *Credentials) PersistCredentialSecret(ctx context.Context, resourceID, credentialID string, secret []byte) error {
	rid, err := uuid.Parse(resourceID)
	if err != nil {
		return err
	}
	cid, err := uuid.Parse(credentialID)
	if err != nil {
		return err
	}
	stored, _, err := store.GetCredentialSealed(ctx, c.db.Pool, cid)
	if errors.Is(err, store.ErrNotFound) {
		return nil
	}
	if err != nil {
		return err
	}
	if stored.ResourceID != rid || stored.Kind != mcpav1.CredentialKind_CREDENTIAL_KIND_OAUTH {
		return fmt.Errorf("credential %s is not an OAuth credential of resource %s", cid, rid)
	}
	sealed, err := credential.Seal(c.keys, stored, credential.Payload{Kind: credential.KindOAuth, OAuth: secret})
	if err != nil {
		return err
	}
	if err := store.UpdateCredentialSecret(ctx, c.db.Pool, cid, stored.AccountLabel, sealed); err != nil {
		if errors.Is(err, store.ErrNotFound) {
			return nil
		}
		return err
	}
	if c.notify != nil {
		// Other replicas reload the refreshed token so a rotated refresh
		// token is not reused.
		c.notify(ctx, rid)
	}
	return nil
}

// credentialChannel is the PostgreSQL notification channel of credential
// changes. Payloads are "<instance>:<resource_id>".
const credentialChannel = "mcpa_credential_changes"

// CredentialSync applies credential changes to the engine immediately and
// propagates them to other replicas with PostgreSQL notifications. It
// implements service.CredentialListener.
type CredentialSync struct {
	db       *store.Store
	keys     *crypto.Keyring
	log      *slog.Logger
	engine   *proxy.Engine
	instance string
}

var _ service.CredentialListener = (*CredentialSync)(nil)

// CredentialRegistered registers a new credential with the engine.
func (s *CredentialSync) CredentialRegistered(ctx context.Context, resourceID, credentialID uuid.UUID) {
	defer s.Notify(ctx, resourceID)
	stored, sealed, err := store.GetCredentialSealed(ctx, s.db.Pool, credentialID)
	if err != nil {
		s.log.Warn("loading a registered credential failed; the resource reloads instead", "resource_id", resourceID, "credential_id", credentialID, "error", err)
		s.engine.InvalidateResource(resourceID.String())
		return
	}
	cred, err := toEngine(s.keys, stored, sealed)
	if err == nil {
		err = s.engine.UpsertCredential(ctx, cred)
	}
	if err != nil {
		s.log.Warn("registering a credential with the engine failed; the resource reloads instead", "resource_id", resourceID, "credential_id", credentialID, "error", err)
		s.engine.InvalidateResource(resourceID.String())
	}
}

// CredentialRemoved stops new calls from selecting the credential.
func (s *CredentialSync) CredentialRemoved(ctx context.Context, resourceID, credentialID uuid.UUID) {
	defer s.Notify(ctx, resourceID)
	if err := s.engine.RemoveCredential(ctx, resourceID.String(), credentialID.String()); err != nil {
		// Removal is safety-critical: drop the runtime so the next call
		// reloads the remaining credentials from PostgreSQL.
		s.log.Warn("removing a credential from the engine failed; dropping the resource runtime", "resource_id", resourceID, "credential_id", credentialID, "error", err)
		s.engine.InvalidateResource(resourceID.String())
	}
}

// ResourceDeleted unregisters every credential of a deleted resource.
func (s *CredentialSync) ResourceDeleted(ctx context.Context, resourceID uuid.UUID, credentialIDs []uuid.UUID) {
	defer s.Notify(ctx, resourceID)
	for _, id := range credentialIDs {
		if err := s.engine.RemoveCredential(ctx, resourceID.String(), id.String()); err != nil {
			s.log.Warn("removing a credential of a deleted resource failed", "resource_id", resourceID, "credential_id", id, "error", err)
		}
	}
	s.engine.InvalidateResource(resourceID.String())
}

// Notify tells other replicas that a resource's credentials changed.
// Failures are logged; replicas also resynchronize when their listener
// reconnects.
func (s *CredentialSync) Notify(ctx context.Context, resourceID uuid.UUID) {
	ctx, cancel := context.WithTimeout(context.WithoutCancel(ctx), 5*time.Second)
	defer cancel()
	if _, err := s.db.Pool.Exec(ctx, `SELECT pg_notify($1, $2)`, credentialChannel, s.instance+":"+resourceID.String()); err != nil {
		s.log.Warn("credential change notification failed", "resource_id", resourceID, "error", err)
	}
}

// Listen applies credential changes of other replicas until ctx ends. On
// every (re)connection it drops all loaded runtimes, because changes made
// while it was not listening are unknown; calls then reload from
// PostgreSQL.
func (s *CredentialSync) Listen(ctx context.Context) error {
	backoff := time.Second
	for ctx.Err() == nil {
		err := s.listenOnce(ctx)
		if ctx.Err() != nil {
			return nil
		}
		s.log.Warn("credential change listener disconnected", "error", err, "retry_in", backoff.String())
		select {
		case <-ctx.Done():
			return nil
		case <-time.After(backoff):
		}
		backoff = min(backoff*2, time.Minute)
	}
	return nil
}

func (s *CredentialSync) listenOnce(ctx context.Context) error {
	conn, err := s.db.Pool.Acquire(ctx)
	if err != nil {
		return err
	}
	// The connection carries LISTEN state; never return it to the pool.
	raw := conn.Hijack()
	defer func() { _ = raw.Close(context.Background()) }()
	if _, err := raw.Exec(ctx, "LISTEN "+pgx.Identifier{credentialChannel}.Sanitize()); err != nil {
		return err
	}
	s.engine.InvalidateAll()
	for {
		n, err := raw.WaitForNotification(ctx)
		if err != nil {
			return err
		}
		instance, resourceID, ok := strings.Cut(n.Payload, ":")
		if !ok || instance == s.instance {
			continue
		}
		if _, err := uuid.Parse(resourceID); err != nil {
			continue
		}
		s.engine.InvalidateResource(resourceID)
	}
}
