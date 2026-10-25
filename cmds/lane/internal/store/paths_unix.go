// SPDX-License-Identifier: Apache-2.0
//go:build !windows

package store

import (
	"errors"
	"os"
	"path/filepath"
	"runtime"
)

func DefaultRoot() (string, error) {
	home, e := os.UserHomeDir()
	if e != nil {
		return "", e
	}
	return dataRoot(runtime.GOOS, home, os.Getenv("XDG_DATA_HOME"))
}
func dataRoot(platform, home, xdg string) (string, error) {
	switch platform {
	case "darwin":
		return filepath.Join(home, "Library", "Application Support", "io.delino.lane"), nil
	default:
		base := xdg
		if base == "" {
			base = filepath.Join(home, ".local", "share")
		}
		if !filepath.IsAbs(base) {
			return "", errors.New("data directory must be absolute")
		}
		return filepath.Join(base, "io.delino.lane"), nil
	}
}
func protectPath(path string, directory bool) error {
	mode := os.FileMode(0600)
	if directory {
		mode = 0700
	}
	return os.Chmod(path, mode)
}
