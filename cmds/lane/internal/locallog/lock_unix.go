//go:build !windows

// SPDX-License-Identifier: Apache-2.0
package locallog

import (
	"context"
	"errors"
	"golang.org/x/sys/unix"
	"os"
	"time"
)

func acquire(ctx context.Context, f *os.File) error {
	ctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	for {
		e := unix.Flock(int(f.Fd()), unix.LOCK_EX|unix.LOCK_NB)
		if e == nil {
			return nil
		}
		if !errors.Is(e, unix.EWOULDBLOCK) {
			return e
		}
		if wait(ctx) != nil {
			return errBusy
		}
	}
}
func release(f *os.File) { _ = unix.Flock(int(f.Fd()), unix.LOCK_UN) }
