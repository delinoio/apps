// SPDX-License-Identifier: Apache-2.0
//go:build !windows

package store

import (
	"errors"
	"golang.org/x/sys/unix"
	"os"
)

func tryHandleLock(f *os.File, exclusive bool) (bool, error) {
	op := unix.LOCK_SH
	if exclusive {
		op = unix.LOCK_EX
	}
	e := unix.Flock(int(f.Fd()), op|unix.LOCK_NB)
	if errors.Is(e, unix.EWOULDBLOCK) || errors.Is(e, unix.EAGAIN) {
		return false, nil
	}
	return e == nil, e
}
func unlockHandle(f *os.File) error { return unix.Flock(int(f.Fd()), unix.LOCK_UN) }
