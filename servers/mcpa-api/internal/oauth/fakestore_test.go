// SPDX-License-Identifier: FSL-1.1-ALv2

package oauth_test

import (
	"context"
	"sync"
	"time"

	"github.com/google/uuid"

	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

// fakeStore is an in-memory oauth.Store with the semantics of
// store.MCPOAuth. now replaces the database clock.
type fakeStore struct {
	mu       sync.Mutex
	now      func() time.Time
	requests map[string]store.MCPAuthRequest
	grants   map[uuid.UUID]store.MCPGrant
	codes    map[string]*fakeCode
	handles  map[string]*fakeHandle
	tokens   map[string]fakeToken
	// deleteErr, when set, fails DeleteGrant like an unavailable database.
	deleteErr error
	// updateErr, when set, fails the next UpdateGrant before it calls fn, like
	// a database that is unreachable when the transaction begins, and is then
	// cleared.
	updateErr error
	// insertAuthErr, when set, fails InsertAuthRequest like an unavailable
	// database.
	insertAuthErr error
	// insertAuthCalls counts InsertAuthRequest calls, failed ones included.
	insertAuthCalls int
}

type fakeCode struct {
	code store.MCPCode
	used bool
}

type fakeHandle struct {
	grant   uuid.UUID
	retired bool
}

type fakeToken struct {
	grant  uuid.UUID
	expire time.Time
}

func newFakeStore(now func() time.Time) *fakeStore {
	return &fakeStore{
		now: now, requests: map[string]store.MCPAuthRequest{}, grants: map[uuid.UUID]store.MCPGrant{},
		codes: map[string]*fakeCode{}, handles: map[string]*fakeHandle{}, tokens: map[string]fakeToken{},
	}
}

func (f *fakeStore) InsertAuthRequest(_ context.Context, r store.MCPAuthRequest) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.insertAuthCalls++
	if f.insertAuthErr != nil {
		return f.insertAuthErr
	}
	f.requests[string(r.StateHash)] = r
	return nil
}

func (f *fakeStore) TakeAuthRequest(_ context.Context, stateHash []byte) (store.MCPAuthRequest, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	r, ok := f.requests[string(stateHash)]
	delete(f.requests, string(stateHash))
	if !ok || !r.ExpireTime.After(f.now()) {
		return store.MCPAuthRequest{}, store.ErrNotFound
	}
	return r, nil
}

func (f *fakeStore) InsertGrant(_ context.Context, g store.MCPGrant, c store.MCPCode) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	g.CreateTime, g.UpdateTime = f.now(), f.now()
	f.grants[g.ID] = g
	c.GrantID = g.ID
	f.codes[string(c.CodeHash)] = &fakeCode{code: c}
	return nil
}

func (f *fakeStore) LookupCode(_ context.Context, codeHash []byte) (store.MCPCode, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	c, ok := f.codes[string(codeHash)]
	if !ok {
		return store.MCPCode{}, store.ErrNotFound
	}
	if c.used {
		return store.MCPCode{CodeHash: codeHash, GrantID: c.code.GrantID}, store.ErrCodeReused
	}
	if !c.code.ExpireTime.After(f.now()) {
		return store.MCPCode{}, store.ErrNotFound
	}
	return c.code, nil
}

func (f *fakeStore) RefreshHandleGrant(_ context.Context, handleHash []byte) (uuid.UUID, bool, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	h, ok := f.handles[string(handleHash)]
	if !ok {
		return uuid.Nil, false, store.ErrNotFound
	}
	return h.grant, h.retired, nil
}

func (f *fakeStore) liveGrant(id uuid.UUID) (store.MCPGrant, bool) {
	g, ok := f.grants[id]
	return g, ok && g.ExpireTime.After(f.now())
}

func (f *fakeStore) GetGrant(_ context.Context, id uuid.UUID) (store.MCPGrant, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	g, ok := f.liveGrant(id)
	if !ok {
		return store.MCPGrant{}, store.ErrNotFound
	}
	return g, nil
}

func (f *fakeStore) GrantByAccessToken(_ context.Context, tokenHash []byte) (store.MCPGrant, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	t, ok := f.tokens[string(tokenHash)]
	if !ok || !t.expire.After(f.now()) {
		return store.MCPGrant{}, store.ErrNotFound
	}
	g, ok := f.liveGrant(t.grant)
	if !ok {
		return store.MCPGrant{}, store.ErrNotFound
	}
	return g, nil
}

func (f *fakeStore) GrantIDByAccessToken(_ context.Context, tokenHash []byte) (uuid.UUID, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	t, ok := f.tokens[string(tokenHash)]
	if !ok {
		return uuid.Nil, store.ErrNotFound
	}
	return t.grant, nil
}

// UpdateGrant holds the store lock while fn runs, like the row lock.
func (f *fakeStore) UpdateGrant(ctx context.Context, id uuid.UUID, use store.MCPGrantUse, fn func(ctx context.Context, g store.MCPGrant) (store.MCPGrantChange, error)) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	if err := f.updateErr; err != nil {
		f.updateErr = nil
		return err
	}
	g, ok := f.liveGrant(id)
	if !ok {
		return store.ErrNotFound
	}
	var h *fakeHandle
	if use.Handle != nil {
		h = f.handles[string(use.Handle)]
		if h == nil || h.grant != id || h.retired {
			return store.ErrHandleRetired
		}
	}
	var c *fakeCode
	if use.Code != nil {
		c = f.codes[string(use.Code)]
		switch {
		case c == nil || c.code.GrantID != id:
			return store.ErrNotFound
		case c.used:
			return store.ErrCodeReused
		case !c.code.ExpireTime.After(f.now()):
			return store.ErrNotFound
		}
	}
	// Like the real store, fn runs detached from request cancellation.
	ch, err := fn(context.WithoutCancel(ctx), g)
	if ch.Delete && err != nil {
		f.deleteLocked(id)
		return err
	}
	if err != nil {
		// Only replaced tokens survive a failed fn; the handle stays active
		// and the code stays unused.
		if ch.Tokens != nil {
			g.Tokens = *ch.Tokens
			g.UpdateTime = f.now()
			f.grants[id] = g
		}
		return err
	}
	if ch.Delete {
		f.deleteLocked(id)
		return nil
	}
	if h != nil {
		h.retired = true
	}
	if c != nil {
		c.used = true
	}
	if ch.Tokens != nil {
		g.Tokens = *ch.Tokens
	}
	if !ch.ExpireTime.IsZero() {
		g.ExpireTime = ch.ExpireTime
	}
	g.UpdateTime = f.now()
	f.grants[id] = g
	if ch.NewHandle != nil {
		f.handles[string(ch.NewHandle)] = &fakeHandle{grant: id}
	}
	if ch.AccessToken != nil {
		f.tokens[string(ch.AccessToken)] = fakeToken{grant: id, expire: ch.AccessExpire}
	}
	return nil
}

func (f *fakeStore) deleteLocked(id uuid.UUID) {
	delete(f.grants, id)
	for k, c := range f.codes {
		if c.code.GrantID == id {
			delete(f.codes, k)
		}
	}
	for k, h := range f.handles {
		if h.grant == id {
			delete(f.handles, k)
		}
	}
	for k, t := range f.tokens {
		if t.grant == id {
			delete(f.tokens, k)
		}
	}
}

func (f *fakeStore) DeleteGrant(_ context.Context, id uuid.UUID) (store.MCPGrant, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.deleteErr != nil {
		return store.MCPGrant{}, f.deleteErr
	}
	g, ok := f.grants[id]
	if !ok {
		return store.MCPGrant{}, store.ErrNotFound
	}
	f.deleteLocked(id)
	return g, nil
}

func (f *fakeStore) DeleteExpired(context.Context) (int64, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	var n int64
	now := f.now()
	for k, r := range f.requests {
		if !r.ExpireTime.After(now) {
			delete(f.requests, k)
			n++
		}
	}
	for k, c := range f.codes {
		if !c.code.ExpireTime.After(now) {
			delete(f.codes, k)
			n++
		}
	}
	for k, t := range f.tokens {
		if !t.expire.After(now) {
			delete(f.tokens, k)
			n++
		}
	}
	for id, g := range f.grants {
		if !g.ExpireTime.After(now) {
			f.deleteLocked(id)
			n++
		}
	}
	return n, nil
}

func (f *fakeStore) setDeleteErr(err error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.deleteErr = err
}

func (f *fakeStore) grantCount() int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return len(f.grants)
}

func (f *fakeStore) setInsertAuthErr(err error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.insertAuthErr = err
}

func (f *fakeStore) authInsertCalls() int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.insertAuthCalls
}

func (f *fakeStore) failNextUpdate(err error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.updateErr = err
}
