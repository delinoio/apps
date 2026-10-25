// SPDX-License-Identifier: Apache-2.0
//go:build !darwin && !linux && !windows

package syncclient

import "os"

func tryCredentialLock(*os.File) (bool, error) { return false, ErrCredentials }
func unlockCredential(*os.File)                {}
