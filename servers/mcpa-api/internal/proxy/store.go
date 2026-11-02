// SPDX-License-Identifier: FSL-1.1-ALv2

package proxy

import (
	"context"
	"crypto/sha256"
	"encoding/json"
	"log/slog"
	"sync"
	"time"

	coreauth "github.com/router-for-me/CLIProxyAPI/v6/sdk/cliproxy/auth"
)

// resourceStore is the coreauth.Store of one resource's auth manager. The
// manager calls Save after token refreshes and after every call result
// (conductor.go MarkResult and Update call persist). resourceStore forwards a
// credential's metadata to the TokenPersister only when it differs from the
// last known payload, and never blocks the manager: Save runs under the
// manager's lock, so the write happens on the engine's persister goroutine.
type resourceStore struct {
	resourceID string
	persister  *asyncPersister

	mu    sync.Mutex
	known map[string][sha256.Size]byte
}

var _ coreauth.Store = (*resourceStore)(nil)

func newResourceStore(resourceID string, persister *asyncPersister) *resourceStore {
	return &resourceStore{resourceID: resourceID, persister: persister, known: map[string][sha256.Size]byte{}}
}

// track records the payload a credential was registered with.
func (s *resourceStore) track(auth *coreauth.Auth) {
	if auth == nil || auth.Metadata == nil {
		return
	}
	payload, err := json.Marshal(auth.Metadata)
	if err != nil {
		return
	}
	s.mu.Lock()
	s.known[auth.ID] = sha256.Sum256(payload)
	s.mu.Unlock()
}

func (s *resourceStore) forget(credentialID string) {
	s.mu.Lock()
	delete(s.known, credentialID)
	s.mu.Unlock()
}

func (s *resourceStore) List(context.Context) ([]*coreauth.Auth, error) { return nil, nil }

func (s *resourceStore) Save(_ context.Context, auth *coreauth.Auth) (string, error) {
	if auth == nil || auth.Disabled || auth.Metadata == nil {
		return "", nil
	}
	if auth.Attributes[attrResourceID] != s.resourceID {
		return "", nil
	}
	payload, err := json.Marshal(auth.Metadata)
	if err != nil {
		return "", nil
	}
	sum := sha256.Sum256(payload)
	s.mu.Lock()
	previous, known := s.known[auth.ID]
	if !known || previous == sum {
		s.mu.Unlock()
		return "", nil
	}
	s.known[auth.ID] = sum
	s.mu.Unlock()
	s.persister.enqueue(s.resourceID, auth.ID, payload)
	return "", nil
}

func (s *resourceStore) Delete(context.Context, string) error { return nil }

// asyncPersister writes changed OAuth payloads to the TokenPersister in the
// background, keeping only the latest payload per credential.
type asyncPersister struct {
	persister TokenPersister
	logger    *slog.Logger
	timeout   time.Duration

	mu      sync.Mutex
	pending map[persistKey][]byte
	wake    chan struct{}
	stop    chan struct{}
	done    chan struct{}
}

type persistKey struct {
	resourceID   string
	credentialID string
}

func newAsyncPersister(persister TokenPersister, logger *slog.Logger, timeout time.Duration) *asyncPersister {
	p := &asyncPersister{
		persister: persister,
		logger:    logger,
		timeout:   timeout,
		pending:   map[persistKey][]byte{},
		wake:      make(chan struct{}, 1),
		stop:      make(chan struct{}),
		done:      make(chan struct{}),
	}
	go p.run()
	return p
}

func (p *asyncPersister) enqueue(resourceID, credentialID string, payload []byte) {
	if p == nil || p.persister == nil {
		return
	}
	p.mu.Lock()
	p.pending[persistKey{resourceID, credentialID}] = payload
	p.mu.Unlock()
	select {
	case p.wake <- struct{}{}:
	default:
	}
}

// drop discards a queued payload of a removed credential.
func (p *asyncPersister) drop(resourceID, credentialID string) {
	if p == nil {
		return
	}
	p.mu.Lock()
	delete(p.pending, persistKey{resourceID, credentialID})
	p.mu.Unlock()
}

func (p *asyncPersister) run() {
	defer close(p.done)
	retry := time.NewTimer(time.Hour)
	retry.Stop()
	for {
		select {
		case <-p.stop:
			p.flush()
			return
		case <-p.wake:
		case <-retry.C:
		}
		if failed := p.flush(); failed {
			retry.Reset(5 * time.Second)
		}
	}
}

// flush writes every queued payload and requeues failures. It reports
// whether any write failed.
func (p *asyncPersister) flush() bool {
	p.mu.Lock()
	batch := p.pending
	p.pending = map[persistKey][]byte{}
	p.mu.Unlock()
	failed := false
	for key, payload := range batch {
		ctx, cancel := context.WithTimeout(context.Background(), p.timeout)
		err := p.persister.PersistCredentialSecret(ctx, key.resourceID, key.credentialID, payload)
		cancel()
		if err == nil {
			continue
		}
		failed = true
		p.logger.Warn("persisting refreshed provider credential failed",
			slog.String("resource_id", key.resourceID),
			slog.String("credential_id", key.credentialID),
			slog.String("error", err.Error()))
		p.mu.Lock()
		if _, newer := p.pending[key]; !newer {
			p.pending[key] = payload
		}
		p.mu.Unlock()
	}
	return failed
}

func (p *asyncPersister) close() {
	if p == nil {
		return
	}
	select {
	case <-p.stop:
	default:
		close(p.stop)
	}
	<-p.done
}
