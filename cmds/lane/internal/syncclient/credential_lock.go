// SPDX-License-Identifier: Apache-2.0
package syncclient

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"os"
	"path/filepath"
	"time"
)

// Credential locks serialize refresh-token rotation across CLI and worker
// processes. They contain no credentials and never lock the SQLite data store.
func (a Auth) lockCredential(ctx context.Context, key string) (func(), error) {
	root := a.CredentialLockRoot
	if root == "" {
		base, e := os.UserCacheDir()
		if e != nil {
			return nil, ErrCredentials
		}
		root = filepath.Join(base, "io.delino.lane", "credential-locks")
	}
	if e := os.MkdirAll(root, 0700); e != nil {
		return nil, ErrCredentials
	}
	digest := sha256.Sum256([]byte(key))
	f, e := os.OpenFile(filepath.Join(root, hex.EncodeToString(digest[:])+".lock"), os.O_CREATE|os.O_RDWR, 0600)
	if e != nil {
		return nil, ErrCredentials
	}
	for {
		ok, e := tryCredentialLock(f)
		if e != nil {
			f.Close()
			return nil, ErrCredentials
		}
		if ok {
			return func() { unlockCredential(f); f.Close() }, nil
		}
		timer := time.NewTimer(25 * time.Millisecond)
		select {
		case <-ctx.Done():
			timer.Stop()
			f.Close()
			return nil, ctx.Err()
		case <-timer.C:
		}
	}
}
