// SPDX-License-Identifier: Apache-2.0
//go:build darwin || linux

package syncclient

import (
	"errors"
	"golang.org/x/sys/unix"
	"os"
)

func tryCredentialLock(f *os.File) (bool, error) {
	e := unix.Flock(int(f.Fd()), unix.LOCK_EX|unix.LOCK_NB)
	if errors.Is(e, unix.EWOULDBLOCK) || errors.Is(e, unix.EAGAIN) {
		return false, nil
	}
	return e == nil, e
}
func unlockCredential(f *os.File) { _ = unix.Flock(int(f.Fd()), unix.LOCK_UN) }
