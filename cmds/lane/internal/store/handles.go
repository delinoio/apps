// SPDX-License-Identifier: Apache-2.0
package store

import (
	"connectrpc.com/connect"
	"context"
	"errors"
	lane "github.com/delinoio/apps/protos/lane/v1"
	"os"
	"path/filepath"
	"time"
)

func (s *Store) lockPath() string { return filepath.Join(s.root, "locks", s.space+".handles.lock") }
func (s *Store) acquireHandles(ctx context.Context, exclusive bool) (*os.File, error) {
	dir := filepath.Join(s.root, "locks")
	if e := os.MkdirAll(dir, 0700); e != nil {
		return nil, e
	}
	if e := protectPath(dir, true); e != nil {
		return nil, e
	}
	f, e := os.OpenFile(s.lockPath(), os.O_CREATE|os.O_RDWR, 0600)
	if e != nil {
		return nil, e
	}
	if e = protectPath(s.lockPath(), false); e != nil {
		f.Close()
		return nil, e
	}
	deadline := time.NewTimer(5 * time.Second)
	defer deadline.Stop()
	for {
		ok, e := tryHandleLock(f, exclusive)
		if e != nil {
			f.Close()
			return nil, e
		}
		if ok {
			return f, nil
		}
		select {
		case <-ctx.Done():
			f.Close()
			return nil, ctx.Err()
		case <-deadline.C:
			f.Close()
			return nil, domainError(9, "Space handles are busy; close other clients and retry", s.space)
		case <-time.After(10 * time.Millisecond):
		}
	}
}
func releaseHandles(f *os.File) error {
	if f == nil {
		return nil
	}
	return errors.Join(unlockHandle(f), f.Close())
}

// The caller holds lifeMu exclusively, after all callback-owned connections return.
func (s *Store) closePhysical() error {
	if s.physicalClosed {
		return nil
	}
	s.physicalClosed = true
	var e error
	if s.db != nil {
		e = s.db.Close()
	}
	if e != nil {
		return e
	}
	e = releaseHandles(s.handles)
	s.handles = nil
	return e
}
func unavailableError(e error) bool {
	var ce *connect.Error
	if errors.As(e, &ce) {
		for _, d := range ce.Details() {
			v, _ := d.Value()
			if detail, ok := v.(*lane.ErrorDetail); ok && detail.Reason == lane.ErrorReason_ERROR_REASON_DELETED {
				return true
			}
		}
	}
	return false
}
func (s *Store) registryUnavailable(ctx context.Context) (bool, error) {
	var n int
	e := s.registry.QueryRowContext(ctx, "SELECT count(*) FROM registry_meta WHERE key=? AND value='1'", "unavailable:"+s.space).Scan(&n)
	return n > 0, e
}
