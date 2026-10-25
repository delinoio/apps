// SPDX-License-Identifier: Apache-2.0
package store

import (
	"golang.org/x/sys/windows"
	"path/filepath"
)

func DefaultRoot() (string, error) {
	base, e := windows.KnownFolderPath(windows.FOLDERID_LocalAppData, 0)
	if e != nil {
		return "", e
	}
	return filepath.Join(base, "io.delino.lane"), nil
}

// Protect the root and inherited files with an explicit current-user-only DACL.
func protectPath(path string, directory bool) error {
	user, e := windows.GetCurrentProcessToken().GetTokenUser()
	if e != nil {
		return e
	}
	flags := ""
	if directory {
		flags = "OICI"
	}
	sd, e := windows.SecurityDescriptorFromString("D:P(A;" + flags + ";FA;;;" + user.User.Sid.String() + ")")
	if e != nil {
		return e
	}
	acl, _, e := sd.DACL()
	if e != nil {
		return e
	}
	return windows.SetNamedSecurityInfo(path, windows.SE_FILE_OBJECT, windows.DACL_SECURITY_INFORMATION|windows.PROTECTED_DACL_SECURITY_INFORMATION, nil, nil, acl, nil)
}
