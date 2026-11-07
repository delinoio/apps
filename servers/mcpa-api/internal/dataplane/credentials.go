// SPDX-License-Identifier: FSL-1.1-ALv2

package dataplane

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"strings"
	"sync"
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
	db   *store.Store
	keys *crypto.Keyring
	log  *slog.Logger
	// notify queues the credential change notification of a resource in
	// the transaction of the change.
	notify func(ctx context.Context, db store.DB, resourceID uuid.UUID) error
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
// recreated. Other replicas are notified in the same transaction so they
// reload the refreshed token and never reuse a rotated refresh token; on
// failure nothing is stored and the engine retries.
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
	err = c.db.InTx(ctx, func(tx pgx.Tx) error {
		if err := store.UpdateCredentialSecret(ctx, tx, cid, stored.AccountLabel, sealed); err != nil {
			return err
		}
		if c.notify != nil {
			return c.notify(ctx, tx, rid)
		}
		return nil
	})
	if errors.Is(err, store.ErrNotFound) {
		return nil
	}
	return err
}

// credentialChannel is the PostgreSQL notification channel of credential
// changes. Payloads are "<instance>:<resource_id>".
const credentialChannel = "mcpa_credential_changes"

// CredentialSync applies credential changes to the engine immediately and
// propagates them to other replicas with PostgreSQL notifications. It
// implements service.CredentialListener.
//
// Each notification is queued with pg_notify in the transaction of the
// change. PostgreSQL delivers it to every listening replica when, and only
// when, that transaction commits, so a committed change cannot miss its
// notification and a rolled-back change sends none. A replica that is not
// listening at that moment drops all runtimes when its listener
// reconnects.
//
// Engine updates of a resource are serialized with lockResource.
// CredentialRegistered applies a snapshot that it reads from PostgreSQL;
// holding the resource lock from that read until the engine update makes
// every later removal or invalidation of the resource, whether local or
// from a notification, apply after the snapshot. Without the lock, a
// concurrent removal could finish first and the stale snapshot would then
// restore the removed credential.
type CredentialSync struct {
	db       *store.Store
	keys     *crypto.Keyring
	log      *slog.Logger
	engine   *proxy.Engine
	instance string

	// all is held shared by every resource lock and exclusively while all
	// runtimes are dropped.
	all     sync.RWMutex
	locksMu sync.Mutex
	locks   map[uuid.UUID]*resourceLock
	// afterLoad, when set by tests, runs in CredentialRegistered between
	// reading the credential and applying it to the engine.
	afterLoad func()
}

// resourceLock serializes the engine updates of one resource. refs counts
// holders and waiters so unused locks are deleted.
type resourceLock struct {
	mu   sync.Mutex
	refs int
}

// lockResource locks the engine state of a resource against concurrent
// credential synchronization and returns the unlock function.
func (s *CredentialSync) lockResource(resourceID uuid.UUID) func() {
	s.all.RLock()
	s.locksMu.Lock()
	if s.locks == nil {
		s.locks = map[uuid.UUID]*resourceLock{}
	}
	l := s.locks[resourceID]
	if l == nil {
		l = &resourceLock{}
		s.locks[resourceID] = l
	}
	l.refs++
	s.locksMu.Unlock()
	l.mu.Lock()
	return func() {
		l.mu.Unlock()
		s.locksMu.Lock()
		if l.refs--; l.refs == 0 {
			delete(s.locks, resourceID)
		}
		s.locksMu.Unlock()
		s.all.RUnlock()
	}
}

// invalidateResource drops the runtime of a resource after any engine
// update of the resource in progress.
func (s *CredentialSync) invalidateResource(resourceID uuid.UUID) {
	defer s.lockResource(resourceID)()
	s.engine.InvalidateResource(resourceID.String())
}

// invalidateAll drops every runtime after the engine updates in progress.
func (s *CredentialSync) invalidateAll() {
	s.all.Lock()
	defer s.all.Unlock()
	s.engine.InvalidateAll()
}

var _ service.CredentialListener = (*CredentialSync)(nil)

// CredentialsChanging queues the change notification of a resource in the
// transaction of a credential change.
func (s *CredentialSync) CredentialsChanging(ctx context.Context, tx pgx.Tx, resourceID uuid.UUID) error {
	return s.notify(ctx, tx, resourceID)
}

// CredentialRegistered registers a new credential with the engine.
func (s *CredentialSync) CredentialRegistered(ctx context.Context, resourceID, credentialID uuid.UUID) {
	defer s.lockResource(resourceID)()
	stored, sealed, err := store.GetCredentialSealed(ctx, s.db.Pool, credentialID)
	if s.afterLoad != nil {
		s.afterLoad()
	}
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
	defer s.lockResource(resourceID)()
	if err := s.engine.RemoveCredential(ctx, resourceID.String(), credentialID.String()); err != nil {
		// Removal is safety-critical: drop the runtime so the next call
		// reloads the remaining credentials from PostgreSQL.
		s.log.Warn("removing a credential from the engine failed; dropping the resource runtime", "resource_id", resourceID, "credential_id", credentialID, "error", err)
		s.engine.InvalidateResource(resourceID.String())
	}
}

// ResourceDeleted unregisters every credential of a deleted resource.
func (s *CredentialSync) ResourceDeleted(ctx context.Context, resourceID uuid.UUID, credentialIDs []uuid.UUID) {
	defer s.lockResource(resourceID)()
	for _, id := range credentialIDs {
		if err := s.engine.RemoveCredential(ctx, resourceID.String(), id.String()); err != nil {
			s.log.Warn("removing a credential of a deleted resource failed", "resource_id", resourceID, "credential_id", id, "error", err)
		}
	}
	s.engine.InvalidateResource(resourceID.String())
}

// notify queues a notification that a resource's credentials changed.
// db must be the transaction of the change: PostgreSQL delivers the
// notification on commit. An error must abort that transaction.
func (s *CredentialSync) notify(ctx context.Context, db store.DB, resourceID uuid.UUID) error {
	if _, err := db.Exec(ctx, `SELECT pg_notify($1, $2)`, credentialChannel, s.instance+":"+resourceID.String()); err != nil {
		s.log.Warn("queueing a credential change notification failed; the change is rolled back", "resource_id", resourceID, "error", err)
		return fmt.Errorf("queue credential change notification: %w", err)
	}
	return nil
}

// Listen applies credential changes of other replicas until ctx ends. On
// every (re)connection it drops all loaded runtimes, because changes made
// while it was not listening are unknown; calls then reload from
// PostgreSQL.
func (s *CredentialSync) Listen(ctx context.Context) error {
	return runListenLoop(ctx, s.log, s.listenOnce, sleepContext)
}

// listenBackoffInitial and listenBackoffMax bound the reconnect delay.
const (
	listenBackoffInitial = time.Second
	listenBackoffMax     = time.Minute
)

// runListenLoop calls listen until ctx ends. listen reports whether it
// reached LISTEN before returning its error. A connection that reached LISTEN
// resets the reconnect delay to listenBackoffInitial, so a healthy connection
// that drops is not treated as another consecutive failure; consecutive
// failures before LISTEN double the delay up to listenBackoffMax. wait blocks
// for the delay and returns false when ctx ended first.
func runListenLoop(
	ctx context.Context,
	log *slog.Logger,
	listen func(context.Context) (bool, error),
	wait func(context.Context, time.Duration) bool,
) error {
	backoff := listenBackoffInitial
	for ctx.Err() == nil {
		connected, err := listen(ctx)
		if ctx.Err() != nil {
			return nil
		}
		if connected {
			backoff = listenBackoffInitial
		}
		log.Warn("credential change listener disconnected", "error", err, "connected", connected, "retry_in", backoff.String())
		if !wait(ctx, backoff) {
			return nil
		}
		backoff = min(backoff*2, listenBackoffMax)
	}
	return nil
}

func sleepContext(ctx context.Context, d time.Duration) bool {
	t := time.NewTimer(d)
	defer t.Stop()
	select {
	case <-ctx.Done():
		return false
	case <-t.C:
		return true
	}
}

// listenOnce runs one listener connection. The first result reports whether
// LISTEN succeeded.
func (s *CredentialSync) listenOnce(ctx context.Context) (bool, error) {
	conn, err := s.db.Pool.Acquire(ctx)
	if err != nil {
		return false, err
	}
	// The connection carries LISTEN state; never return it to the pool.
	raw := conn.Hijack()
	defer func() { _ = raw.Close(context.Background()) }()
	if _, err := raw.Exec(ctx, "LISTEN "+pgx.Identifier{credentialChannel}.Sanitize()); err != nil {
		return false, err
	}
	s.invalidateAll()
	for {
		n, err := raw.WaitForNotification(ctx)
		if err != nil {
			return true, err
		}
		instance, resourceID, ok := strings.Cut(n.Payload, ":")
		if !ok || instance == s.instance {
			continue
		}
		id, err := uuid.Parse(resourceID)
		if err != nil {
			continue
		}
		s.invalidateResource(id)
	}
}
