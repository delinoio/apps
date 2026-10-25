// SPDX-License-Identifier: Apache-2.0
package locallog

import (
	"context"
	"errors"
	"golang.org/x/sys/windows"
	"os"
	"time"
)

func acquire(ctx context.Context, f *os.File) error {
	ctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	for {
		e := windows.LockFileEx(windows.Handle(f.Fd()), windows.LOCKFILE_EXCLUSIVE_LOCK|windows.LOCKFILE_FAIL_IMMEDIATELY, 0, 1, 0, &windows.Overlapped{})
		if e == nil {
			return nil
		}
		if !errors.Is(e, windows.ERROR_LOCK_VIOLATION) {
			return e
		}
		if wait(ctx) != nil {
			return errBusy
		}
	}
}
func release(f *os.File) {
	_ = windows.UnlockFileEx(windows.Handle(f.Fd()), 0, 1, 0, &windows.Overlapped{})
}
